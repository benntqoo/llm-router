param(
  [string]$Out = "D:\Code\llm-router\docs\reviews\app-ui-t24-tos-dialog.png",
  [int]$X = 0,
  [int]$Y = 0,
  [int]$W = 1270,
  [int]$H = 1400,
  # 抢完前台后按几次 PageDown（**不用鼠标**——本机 SetCursorPos 被屏蔽）
  [int]$ScrollPages = 0
)

# 屏幕实拍（不走 PrintWindow）：
# **PrintWindow 拍不到 `position: fixed` 的 GPU 合成层** → 弹窗类浮层必须用屏幕实拍。

$ErrorActionPreference = 'Stop'

Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Win {
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr hWnd, IntPtr after, int x, int y, int cx, int cy, uint flags);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, IntPtr pid);
  [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint idAttach, uint idAttachTo, bool attach);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
  [DllImport("user32.dll")] public static extern void keybd_event(byte bVk, byte bScan, uint dwFlags, UIntPtr dwExtraInfo);
  [DllImport("user32.dll")] public static extern bool AllowSetForegroundWindow(int pid);
  [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr hWnd);
  public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
}
"@

$proc = Get-Process -Name "llm-router" -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $proc) { throw "没有正在运行的 llm-router 实例" }
$deadline = (Get-Date).AddSeconds(20)
while ((Get-Date) -lt $deadline) {
  $proc.Refresh()
  if ($proc.MainWindowHandle -ne [IntPtr]::Zero) { break }
  Start-Sleep -Milliseconds 500
}
$proc.Refresh()
$hwnd = $proc.MainWindowHandle
if ($hwnd -eq [IntPtr]::Zero) { throw "没有主窗口句柄" }

# 取消最大化并摆到指定位置（**避开桌面上别的窗口**）
[Win]::ShowWindow($hwnd, 9) | Out-Null   # SW_RESTORE
Start-Sleep -Milliseconds 600
[Win]::SetWindowPos($hwnd, [IntPtr]::Zero, $X, $Y, $W, $H, 0x0040) | Out-Null  # SWP_SHOWWINDOW
Start-Sleep -Milliseconds 800

# 抢前台（**三招叠加**）：
# ① ALT 敲一下放开前台的锁（Windows 的老规矩）
# ② AllowSetForegroundWindow 放行
# ③ AttachThreadInput 把两个线程接上
$ok = $false
for ($i = 0; $i -lt 10; $i++) {
  [Win]::AllowSetForegroundWindow(-1) | Out-Null
  [Win]::keybd_event(0x12, 0, 0, [UIntPtr]::Zero)            # ALT down
  [Win]::keybd_event(0x12, 0, 0x0002, [UIntPtr]::Zero)      # ALT up
  $fg = [Win]::GetForegroundWindow()
  $fgThread = [Win]::GetWindowThreadProcessId($fg, [IntPtr]::Zero)
  $myThread = [Win]::GetCurrentThreadId()
  [Win]::AttachThreadInput($myThread, $fgThread, $true) | Out-Null
  [Win]::SetForegroundWindow($hwnd) | Out-Null
  [Win]::BringWindowToTop($hwnd) | Out-Null
  [Win]::AttachThreadInput($myThread, $fgThread, $false) | Out-Null
  Start-Sleep -Milliseconds 700
  if ([Win]::GetForegroundWindow() -eq $hwnd) { $ok = $true; break }
}
Write-Output "window=$hwnd foreground=$ok"

if ($ScrollPages -gt 0) {
  Add-Type -AssemblyName System.Windows.Forms
  for ($i = 0; $i -lt $ScrollPages; $i++) {
    [System.Windows.Forms.SendKeys]::SendWait("{PGDN}")
    Start-Sleep -Milliseconds 700
  }
  Start-Sleep -Seconds 1
}

Add-Type -AssemblyName System.Drawing
$rect = New-Object Win+RECT
[Win]::GetWindowRect($hwnd, [ref]$rect) | Out-Null
$w = $rect.Right - $rect.Left
$h = $rect.Bottom - $rect.Top
$bmp = New-Object System.Drawing.Bitmap $w, $h
$gfx = [System.Drawing.Graphics]::FromImage($bmp)
$gfx.CopyFromScreen($rect.Left, $rect.Top, 0, 0, (New-Object System.Drawing.Size $w, $h))
$gfx.Dispose()
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Output "screenshot=$Out size=${w}x${h}"
