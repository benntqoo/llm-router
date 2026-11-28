param(
  [string]$Exe = "D:\Code\llm-router\src-tauri\target\debug\llm-router.exe",
  [string]$Out = "D:\Code\llm-router\docs\reviews\app-ui-t26-tabs.png",
  [int]$WaitSecs = 15
)

$ErrorActionPreference = 'Stop'

Get-Process -Name "llm-router" -ErrorAction SilentlyContinue | ForEach-Object {
  try { $_.Kill(); $_.WaitForExit(5000) } catch {}
}
Start-Sleep -Seconds 2

$proc = Start-Process -FilePath $Exe -PassThru -WindowStyle Hidden
Write-Output "started pid=$($proc.Id)"

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

$bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$bmp = New-Object System.Drawing.Bitmap $bounds.Width, $bounds.Height
$gfx = [System.Drawing.Graphics]::FromImage($bmp)
$gfx.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size)
$gfx.Dispose()
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()

Write-Output "screenshot=$Out"
Write-Output "still running=$(-not $proc.HasExited)"
