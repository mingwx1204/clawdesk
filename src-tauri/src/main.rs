// 预留给移动端入口；桌面端直接调用库入口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    clawdesk_lib::run()
}
