//! `builtin:calculate` —— 安全四则运算计算器。
//!
//! 设计说明：
//! - 采用自研递归下降解析器，**绝不 eval 任意表达式**（无代码注入面）；
//! - 支持 `+ - * /`、括号、一元负号、小数；
//! - 除零、非法表达式、空输入均返回 error 态。

use std::sync::Arc;

use serde_json::json;

use crate::core::tool::def::{ToolParamDef, UnifiedToolDef};
use crate::core::tool::error::ToolError;
use crate::core::tool::registry::{ToolHandler, ToolRegistry};
use crate::core::tool::result::ToolResult;

/// uiPayload 契约：仅前端渲染通道消费（DEV_SPEC.md §8）。
const UI_PAYLOAD: &str = r#"{"displayHint":{"icon":"🧮","tone":"accent","note":"仅支持四则运算 + - * / 与括号"}}"#;

pub fn register(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "calculate",
        "计算四则运算表达式，如 `(1 + 2) * 3 - 4 / 2`",
        vec![ToolParamDef {
            name: "expression".into(),
            param_type: "string".into(),
            description: "四则运算表达式".into(),
            required: true,
            enum_values: None,
            default: None,
        }],
    )?
    .with_ui_payload(serde_json::from_str(UI_PAYLOAD).unwrap());

    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            let expr = args
                .get("expression")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            match evaluate(expr) {
                Ok(value) => Ok(ToolResult::ok(json!({ "expression": expr, "result": value }))),
                Err(msg) => Ok(ToolResult::err(format!("表达式无效: {}", msg))),
            }
        })
    });

    registry.register(def, handler)
}

// ---------- 表达式求值（递归下降） ----------

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

fn evaluate(input: &str) -> Result<f64, String> {
    if input.trim().is_empty() {
        return Err("空表达式".into());
    }
    let mut p = Parser {
        chars: input.chars().collect(),
        pos: 0,
    };
    let value = p.parse_expr()?;
    p.skip_ws();
    if p.pos < p.chars.len() {
        return Err(format!("多余字符: `{}`", p.remaining()));
    }
    Ok(value)
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(c) if c.is_whitespace()) {
            self.pos += 1;
        }
    }

    fn remaining(&self) -> String {
        self.chars[self.pos..].iter().collect()
    }

    fn parse_expr(&mut self) -> Result<f64, String> {
        self.parse_add_sub()
    }

    fn parse_add_sub(&mut self) -> Result<f64, String> {
        let mut lhs = self.parse_mul_div()?;
        loop {
            self.skip_ws();
            match self.peek() {
                Some('+') => {
                    self.pos += 1;
                    lhs += self.parse_mul_div()?;
                }
                Some('-') => {
                    self.pos += 1;
                    lhs -= self.parse_mul_div()?;
                }
                _ => return Ok(lhs),
            }
        }
    }

    fn parse_mul_div(&mut self) -> Result<f64, String> {
        let mut lhs = self.parse_unary()?;
        loop {
            self.skip_ws();
            match self.peek() {
                Some('*') => {
                    self.pos += 1;
                    lhs *= self.parse_unary()?;
                }
                Some('/') => {
                    self.pos += 1;
                    let rhs = self.parse_unary()?;
                    if rhs == 0.0 {
                        return Err("除数为零".into());
                    }
                    lhs /= rhs;
                }
                _ => return Ok(lhs),
            }
        }
    }

    fn parse_unary(&mut self) -> Result<f64, String> {
        self.skip_ws();
        match self.peek() {
            Some('-') => {
                self.pos += 1;
                Ok(-self.parse_unary()?)
            }
            // 注意：不支持一元正号 `+`，因此 `1 ++ 2` 会被拒绝；
            // 但 `1 + -2`（二元加 + 一元负号）合法。
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Result<f64, String> {
        self.skip_ws();
        match self.peek() {
            Some('(') => {
                self.pos += 1;
                let v = self.parse_expr()?;
                self.skip_ws();
                if self.peek() != Some(')') {
                    return Err("缺少右括号".into());
                }
                self.pos += 1;
                Ok(v)
            }
            Some(c) if c.is_ascii_digit() || c == '.' => self.parse_number(),
            Some(c) => Err(format!("非法字符 `{}`", c)),
            None => Err("表达式意外结束".into()),
        }
    }

    fn parse_number(&mut self) -> Result<f64, String> {
        self.skip_ws();
        let start = self.pos;
        let mut has_dot = false;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                self.pos += 1;
            } else if c == '.' && !has_dot {
                has_dot = true;
                self.pos += 1;
            } else {
                break;
            }
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        if text.is_empty() || text == "." {
            return Err(format!("非法数字 `{}`", text));
        }
        text.parse::<f64>()
            .map_err(|_| format!("非法数字 `{}`", text))
    }
}

#[cfg(test)]
mod tests {
    use super::evaluate;

    #[test]
    fn basic_arithmetic() {
        assert_eq!(evaluate("1 + 2 * 3").unwrap(), 7.0);
        assert_eq!(evaluate("(1 + 2) * 3").unwrap(), 9.0);
        assert_eq!(evaluate("10 / 4").unwrap(), 2.5);
        assert_eq!(evaluate("-5 + 3").unwrap(), -2.0);
        assert_eq!(evaluate("1 + -2").unwrap(), -1.0);
    }

    #[test]
    fn rejects_invalid() {
        assert!(evaluate("1 ++ 2").is_err());
        assert!(evaluate("").is_err());
        assert!(evaluate("1 / 0").is_err());
        assert!(evaluate("(1 + 2").is_err());
        assert!(evaluate("1 + a").is_err());
    }
}
