param(
  [string]$Exe = "D:\Code\llm-router\src-tauri\target\debug\llm-router.exe",
  [string]$Out = "D:\Code\llm-router\docs\reviews\app-ui-t26-tabs.png",
  [int]$WaitSecs = 15,
  # 抢完前台后按几次 PageDown（本机 SetCursorPos 被屏蔽 → 靠键盘滚动）
  [int]$ScrollPages = 0,
  # 滚轮滚动次数（**光标本来就在应用窗口内**时有效；不动光标）
  [int]$Wheel = 0,
  # 复用已经在跑的实例（不重启）—— 配合 HMR 取证
  [switch]$UseRunning
)

$ErrorActionPreference = 'Stop'

$proc = $null
if ($UseRunning) {
  # 复用已经在跑的实例（**别重启** —— 重启会丢 HMR 后的状态）
  $proc = Get-Process -Name "llm-router" -ErrorAction SilentlyContinue | Select-Object -First 1
  if (-not $proc) { throw "没有正在运行的 llm-router 实例" }
  Write-Output "reusing pid=$($proc.Id)"
} else {
  Get-Process -Name "llm-router" -ErrorAction SilentlyContinue | ForEach-Object {
    try { $_.Kill(); $_.WaitForExit(5000) } catch {}
  }
  Start-Sleep -Seconds 2
  $proc = Start-Process -FilePath $Exe -PassThru -WindowStyle Hidden
  Write-Output "started pid=$($proc.Id)"
}

Start-Sleep -Seconds $WaitSecs

# 把应用窗口置前（**不用鼠标**——本机 SetCursorPos 被环境屏蔽）
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
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdcBlt, uint nFlags);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, int dx, int dy, int data, UIntPtr extra);
  public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
}
"@
$deadline = (Get-Date).AddSeconds(20)
while ((Get-Date) -lt $deadline) {
  $proc.Refresh()
  if ($proc.MainWindowHandle -ne [IntPtr]::Zero) { break }
  Start-Sleep -Milliseconds 500
}
$proc.Refresh()
if ($proc.MainWindowHandle -ne [IntPtr]::Zero) {
  $hwnd = $proc.MainWindowHandle
  # **抢前台**：AttachThreadInput 把两个线程接上（否则 Windows 不让后台进程抢焦点）
  # 抢不到就重试——桌面上的别的应用可能正好在弹窗抢焦点。
  $ok = $false
  for ($i = 0; $i -lt 8; $i++) {
    $fg = [Win]::GetForegroundWindow()
    $fgThread = [Win]::GetWindowThreadProcessId($fg, [IntPtr]::Zero)
    $myThread = [Win]::GetCurrentThreadId()
    [Win]::AttachThreadInput($myThread, $fgThread, $true) | Out-Null
    [Win]::ShowWindow($hwnd, 3) | Out-Null          # SW_MAXIMIZE
    [Win]::SetForegroundWindow($hwnd) | Out-Null
    [Win]::AttachThreadInput($myThread, $fgThread, $false) | Out-Null
    Start-Sleep -Milliseconds 700
    if ([Win]::GetForegroundWindow() -eq $hwnd) { $ok = $true; break }
  }
  Write-Output "window handle=$hwnd foreground=$ok"
} else {
  Write-Output "no main window handle"
}
Start-Sleep -Seconds 3

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

if ($ScrollPages -gt 0) {
  # **不用鼠标**：靠键盘翻页（本机 SetCursorPos 被环境屏蔽，点不动）
  for ($i = 0; $i -lt $ScrollPages; $i++) {
    [System.Windows.Forms.SendKeys]::SendWait("{PGDN}")
    Start-Sleep -Milliseconds 600
  }
  Start-Sleep -Seconds 1
}

if ($Wheel -gt 0) {
  # 滚轮事件：**不移动光标**（光标本来就在应用窗口内）
  for ($i = 0; $i -lt $Wheel; $i++) {
    [Win]::mouse_event(0x0800, 0, 0, -300, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 350
  }
  Start-Sleep -Seconds 1
}

# 抓**应用窗口本身**（PrintWindow）——桌面上的别的窗口挡着也不影响
# （PW_RENDERFULLCONTENT = 2，对 WebView2 这类合成窗口也有效）
$printed = $false
if ($proc.MainWindowHandle -ne [IntPtr]::Zero) {
  $rect = New-Object Win+RECT
  [Win]::GetWindowRect($proc.MainWindowHandle, [ref]$rect) | Out-Null
  $w = $rect.Right - $rect.Left
  $h = $rect.Bottom - $rect.Top
  if ($w -gt 0 -and $h -gt 0) {
    $bmp = New-Object System.Drawing.Bitmap $w, $h
    $gfx = [System.Drawing.Graphics]::FromImage($bmp)
    $hdc = $gfx.GetHdc()
    $printed = [Win]::PrintWindow($proc.MainWindowHandle, $hdc, 2)
    $gfx.ReleaseHdc($hdc)
    $gfx.Dispose()
    $bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
  }
}

if (-not $printed) {
  # 兜底：截全屏（PrintWindow 失败时）
  $bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
  $bmp = New-Object System.Drawing.Bitmap $bounds.Width, $bounds.Height
  $gfx = [System.Drawing.Graphics]::FromImage($bmp)
  $gfx.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size)
  $gfx.Dispose()
  $bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
  $bmp.Dispose()
}

Write-Output "screenshot=$Out (printWindow=$printed)"
Write-Output "still running=$(-not $proc.HasExited)"
