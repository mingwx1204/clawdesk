//! ClawDesk Tauri 权限弹窗适配 —— harness/hooks 的 Tauri 桥（追加模块）。
//!
//! 关系说明：
//! - 复用现有 `middleware/sandbox` 白名单层（`SandboxManager` 挂在
//!   `ToolDispatcher` 中间件链中，引擎工具执行同样走该链）；
//! - 本模块只负责「StepConfirm 模式的用户确认弹窗」与「前端统一入口」；
//! - 高危红线（HighRiskGuard）拦截也自动生效于 dispatch 中间件链。

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex as AsyncMutex, oneshot};

/// 前端权限弹窗载荷。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PermissionRequest {
    pub request_id: String,
    pub tool_name: String,
    pub tool_args: serde_json::Value,
    pub description: String,
    pub risk_level: String, // "safe" | "warning" | "danger"
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// 前端回传决策。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PermissionDecision {
    pub request_id: String,
    pub approved: bool,
    pub note: Option<String>,
}

/// Tauri 权限桥 —— 负责：
///   1. 把权限请求转发给前端（宿主注入 `emit_to("permission-request")` 回调）；
///   2. 维护 pending 表，`resolve` 时用 oneshot 回传决策；
///   3. 超时（60s）默认拒绝（fail-closed）。
pub struct TauriPermissionBridge {
    pending: AsyncMutex<HashMap<String, oneshot::Sender<PermissionDecision>>>,
    on_request: Option<Box<dyn Fn(PermissionRequest) + Send + Sync>>,
}

impl Default for TauriPermissionBridge {
    fn default() -> Self {
        Self {
            pending: AsyncMutex::new(HashMap::new()),
            on_request: None,
        }
    }
}

impl TauriPermissionBridge {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注入请求回调（宿主在 setup 中设置为 `emit_to`）。
    pub fn set_emitter(&mut self, f: Box<dyn Fn(PermissionRequest) + Send + Sync>) {
        self.on_request = Some(f);
    }

    /// 请求权限 —— 阻塞等待用户决策，最多 60s，超时拒绝。
    pub async fn request_permission(
        &self,
        tool_name: &str,
        tool_args: &serde_json::Value,
        description: &str,
        risk_level: &str,
    ) -> bool {
        let request_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();

        {
            let mut pending = self.pending.lock().await;
            pending.insert(request_id.clone(), tx);
        }

        if let Some(emitter) = &self.on_request {
            emitter(PermissionRequest {
                request_id: request_id.clone(),
                tool_name: tool_name.to_string(),
                tool_args: tool_args.clone(),
                description: description.to_string(),
                risk_level: risk_level.to_string(),
                timestamp: chrono::Utc::now(),
            });
        } else {
            tracing::warn!("TauriPermissionBridge 未注入 emitter，默认拒绝");
            self.pending.lock().await.remove(&request_id);
            return false;
        }

        match tokio::time::timeout(std::time::Duration::from_secs(60), rx).await {
            Ok(Ok(d)) => d.approved,
            Ok(Err(_)) => false,
            Err(_) => {
                tracing::warn!("权限请求超时（拒绝）：{request_id}");
                self.pending.lock().await.remove(&request_id);
                false
            }
        }
    }

    /// 前端回传决策（harness_respond_permission 命令调用）。
    pub async fn resolve(&self, request_id: &str, approved: bool, note: Option<String>) -> bool {
        let sender = self.pending.lock().await.remove(request_id);
        match sender {
            Some(tx) => {
                let _ = tx.send(PermissionDecision {
                    request_id: request_id.to_string(),
                    approved,
                    note,
                });
                true
            }
            None => false,
        }
    }
}

/// 全局权限桥单例（宿主 setup 初始化）。
pub static PERMISSION_BRIDGE: std::sync::OnceLock<Arc<TauriPermissionBridge>> =
    std::sync::OnceLock::new();


/// 创建 StepConfirm 确认回调（公共提取，供 runner 与 commands 共用）。
/// - `registry`：工具注册表 Arc（查 def 做 risk_of 分级）
/// - `tx_event`：可选引擎事件发送器（传 Some 时发 ConfirmRequired 给 progress 转发）
pub fn make_step_confirm_callback(
    registry: Arc<crate::core::tool::registry::ToolRegistry>,
    tx_event: Option<tokio::sync::mpsc::Sender<crate::harness::core::turn_loop::EngineEvent>>,
) -> crate::llm::runner::ConfirmFn {
    use crate::middleware::risk::{RiskLevel, risk_of};
    Arc::new(move |_call_id: &str, tool_id: &str, arguments: &serde_json::Value| {
        let risk = registry
            .get(tool_id)
            .map(|def| risk_of(&def, arguments))
            .unwrap_or(RiskLevel::High);
        let risk_level = match risk {
            RiskLevel::High => "high".to_string(),
            RiskLevel::Normal => "normal".to_string(),
        };
        if let Some(ref tx) = tx_event {
            // async runtime 线程内不能用 blocking_send（会 panic），用 try_send 非阻塞发送
            let _ = tx.try_send(crate::harness::core::turn_loop::EngineEvent::ConfirmRequired {
                call_id: _call_id.to_string(),
                tool_id: tool_id.to_string(),
                risk_level: risk_level.clone(),
                arguments: arguments.clone(),
            });
        }
        match PERMISSION_BRIDGE.get() {
            Some(bridge) => {
                // 当前处于 ENGINE_RT 的工作线程内：不能直接 ENGINE_RT.block_on（会 panic）。
                // 方案：在 ENGINE_RT 上 spawn 异步请求，用标准 std 通道同步等待结果。
                // std mpsc 的 recv 不会触发 tokio 的 "blocking within runtime" 检测；
                // ENGINE_RT 是多线程 runtime（4 worker），spawn 的任务在其他 worker 上推进，不会死锁。
                let (tx_ok, rx_ok) = std::sync::mpsc::channel::<bool>();
                let b = bridge.clone();
                let tool = tool_id.to_string();
                let args = arguments.clone();
                let desc = format!("工具 {tool_id} 需要权限确认");
                let lvl = risk_level.clone();
                crate::harness::core::turn_loop::ENGINE_RT.spawn(async move {
                    let ok = b
                        .request_permission(&tool, &args, &desc, &lvl)
                        .await;
                    let _ = tx_ok.send(ok);
                });
                rx_ok.recv().unwrap_or(false)
            }
            None => false,
        }
    })
}
