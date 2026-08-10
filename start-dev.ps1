<#
ClawDesk 开发启动脚本（终端/日志乱码修复版）
用法:
  .\start-dev.ps1            # 日志写到 D:\clawdesk_dev.log
  .\start-dev.ps1 21         # 日志写到 D:\clawdesk_dev21.log

修复的 4 个乱码问题:
  1. chcp 65001 + UTF-8 输出编码
     -> 控制台默认 GBK(936)，Vite 的 "➜" 等 UTF-8 字符被解码成 "鉃"
  2. NO_COLOR / CARGO_TERM_COLOR / FORCE_COLOR
     -> 去掉 \x1b[39m 这类 ANSI 颜色转义码残留
  3. CARGO_TERM_PROGRESS_WHEN=never
     -> 关闭 cargo 用 \r 刷新的进度条动画，避免日志里 "clawdes鈥?" 残影
  4. 日志按 UTF-8 写入
     -> PowerShell 5.1 的 Tee-Object 默认写 UTF-16(BOM FF FE)，换成 Out-File -Encoding utf8
#>
param([string]$DevNo = '')

# ---------- 1. 终端切换 UTF-8 ----------
chcp 65001 | Out-Null
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new()
$OutputEncoding           = [System.Text.UTF8Encoding]::new()

# ---------- 2. 禁用颜色与进度条动画 ----------
$env:NO_COLOR                 = '1'
$env:FORCE_COLOR              = '0'
$env:CARGO_TERM_COLOR         = 'never'
$env:CARGO_TERM_PROGRESS_WHEN = 'never'

Set-Location (Split-Path -Parent $MyInvocation.MyCommand.Path)

# ---------- 3. 日志文件（UTF-8 无乱码） ----------
$logFile = if ($DevNo) { "D:\clawdesk_dev$DevNo.log" } else { 'D:\clawdesk_dev.log' }
Remove-Item $logFile -ErrorAction SilentlyContinue

Write-Host "ClawDesk dev 启动: UTF-8 终端 / 无颜色 / 日志 -> $logFile" -ForegroundColor Cyan

# ---------- 4. 运行：实时显示 + UTF-8 写日志 ----------
# 用 cmd /c 在内部合并 stderr(2>&1)，避免 PowerShell 5.1 把 cargo/vite 的
# stderr 包装成 RemoteException 噪音行；日志用无 BOM 的 UTF-8 写入
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$writer = New-Object System.IO.StreamWriter($logFile, $false, $utf8NoBom)
try {
    cmd /c "npm run tauri dev 2>&1" | ForEach-Object {
        $_
        $writer.WriteLine($_)
        $writer.Flush()
    }
} finally {
    $writer.Close()
}
