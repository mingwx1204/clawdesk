//! Plan-and-Execute 规划器 —— 复杂任务先规划再执行。
//!
//! 流程：
//! 1. 将用户请求交给 LLM 生成结构化计划（步骤列表）；
//! 2. 把计划注入 system 提示，随后进入执行循环（runner）；
//! 3. 执行中如工具失败，反思信息随 tool 消息回传，模型可修正计划。
//!
//! 说明：规划本身依赖 LLM（经 `ChatProvider`），本模块负责
//! 计划文本的解析与校验（离线可测），不直接调网络。

/// 从模型文本中提取计划步骤（`- 步骤` 或 `1. 步骤` 形式）。
///
/// 解析规则：
/// - 逐行扫描，匹配 `1. xxx`、`- xxx`、`* xxx`；
/// - 忽略空行与标题（如 `## 计划`）。
/// 无法识别时返回空 Vec（调用方可回退为直接执行）。
pub fn parse_plan(text: &str) -> Vec<String> {
    let mut steps = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // 编号列表：`1. step` / `1) step`
        if let Some(rest) = trimmed
            .strip_prefix(|c: char| c.is_ascii_digit())
            .and_then(|r| r.strip_prefix(['.', ')']))
            .map(|r| r.trim())
        {
            if !rest.is_empty() {
                steps.push(rest.to_string());
                continue;
            }
        }
        // 符号列表：`- step` / `* step`
        if let Some(rest) = trimmed
            .strip_prefix(['-', '*'])
            .map(|r| r.trim())
        {
            if !rest.is_empty() {
                steps.push(rest.to_string());
            }
        }
    }
    steps
}

/// 构造规划提示：请求 LLM 输出步骤计划（而非直接执行）。
pub fn build_plan_prompt(user_request: &str) -> String {
    format!(
        "请为以下任务制定一个执行计划，只输出步骤列表（每行一条，用 `1.` 编号），\
         不要执行、不要解释，计划中的每一步都应可独立完成：\n\n{}",
        user_request
    )
}

/// 将计划注入 system 提示（追加到已有系统提示之后）。
pub fn inject_plan_into_system(system: &str, plan_steps: &[String]) -> String {
    if plan_steps.is_empty() {
        return system.to_string();
    }
    let plan_text = plan_steps
        .iter()
        .enumerate()
        .map(|(i, s)| format!("{}. {}", i + 1, s))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "{}\n\n【执行计划】请按以下步骤逐一执行：\n{}\n（完成所有步骤后输出总结）",
        system, plan_text
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_numbered_plan() {
        let text = "## 计划\n1. 获取当前时间\n2. 计算 1+2\n3. 汇总结果";
        let steps = parse_plan(text);
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0], "获取当前时间");
        assert_eq!(steps[2], "汇总结果");
    }

    #[test]
    fn parse_bullet_plan() {
        let text = "- 第一步\n- 第二步\n\n* 第三步";
        let steps = parse_plan(text);
        assert_eq!(steps, vec!["第一步", "第二步", "第三步"]);
    }

    #[test]
    fn parse_empty_or_unstructured() {
        assert!(parse_plan("").is_empty());
        assert!(parse_plan("我直接执行，不需要计划").is_empty());
    }

    #[test]
    fn inject_plan_appends_numbered_steps() {
        let system = "你是助手。";
        let plan = vec!["取时间".to_string(), "汇总".to_string()];
        let out = inject_plan_into_system(system, &plan);
        assert!(out.contains("【执行计划】"));
        assert!(out.contains("1. 取时间"));
        assert!(out.contains("2. 汇总"));
    }

    #[test]
    fn inject_empty_plan_returns_original() {
        let system = "你是助手。";
        assert_eq!(inject_plan_into_system(system, &[]), system);
    }
}
