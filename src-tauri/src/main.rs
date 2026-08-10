#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // reqwest 使用 rustls-no-provider：必须在构建 Client 前安装加密 provider，否则 panic
    let _ = rustls::crypto::ring::default_provider().install_default();
    clawdesk_lib::run()
}
