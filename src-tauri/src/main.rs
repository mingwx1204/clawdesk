#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // reqwest 使用 rustls-no-provider：必须在构建 Client 前安装加密 provider，否则 panic
    let _ = rustls::crypto::ring::default_provider().install_default();
    // harness 引擎日志走 tracing：不初始化 subscriber 时 warn!/error! 全部静默丢弃，
    // 引擎故障（SSE 解析失败/压缩降级/权限超时）将完全不可见 → 轻量输出到 stderr（warn+）
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_target(false)
        .try_init();
    clawdesk_lib::run()
}
