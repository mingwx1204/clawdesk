# Generate 1024x1024 placeholder source PNG (stage 0 scaffold; replaced in stage 5)
# Usage: powershell -ExecutionPolicy Bypass -File gen-icon-source.ps1
$ErrorActionPreference = "Stop"

$outPath = "D:\workspace\ClawDesk\app-icon-source.png"

Add-Type -AssemblyName System.Drawing

$size = 1024
$bmp = New-Object System.Drawing.Bitmap($size, $size)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias

# Background: solid deep blue
$bg = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(255, 30, 58, 138))
$g.FillRectangle($bg, 0, 0, $size, $size)

# Paw print: white palm pad + three toes
$white = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(255, 255, 255, 255))
$cx = 512
$cy = 560
$g.FillEllipse($white, $cx - 130, $cy - 90, 260, 260)          # palm pad
$g.FillEllipse($white, $cx - 250, $cy - 320, 130, 190)        # left toe
$g.FillEllipse($white, $cx - 65, $cy - 380, 130, 210)         # middle toe
$g.FillEllipse($white, $cx + 120, $cy - 320, 130, 190)        # right toe

$bmp.Save($outPath, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose()
$bg.Dispose()
$white.Dispose()
$bmp.Dispose()

Write-Host "PNG generated: $outPath ($((Get-Item $outPath).Length) bytes)"
