# Measures Omega's idle CPU and memory: focused window, then minimized.
# Usage: .\tools\perf.ps1 [-Exe target\release\omega.exe] [-Seconds 20]
param(
    [string]$Exe = (Join-Path (Split-Path -Parent $PSScriptRoot) 'target\release\omega.exe'),
    [int]$Seconds = 20
)
$ErrorActionPreference = 'Stop'
Add-Type -Namespace Win32 -Name User32 -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool ShowWindow(System.IntPtr hWnd, int nCmdShow);
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(System.IntPtr hWnd);
'@

function Measure-Phase {
    param([System.Diagnostics.Process]$Process, [int]$Seconds)
    $Process.Refresh()
    $cpu0 = $Process.TotalProcessorTime
    $clock = [System.Diagnostics.Stopwatch]::StartNew()
    $peak = 0
    for ($i = 0; $i -lt $Seconds; $i++) {
        Start-Sleep -Seconds 1
        $Process.Refresh()
        $peak = [Math]::Max($peak, $Process.WorkingSet64)
    }
    $used = ($Process.TotalProcessorTime - $cpu0).TotalMilliseconds
    $cores = [Environment]::ProcessorCount
    [pscustomobject]@{
        CpuPercent = [Math]::Round(100 * $used / $clock.Elapsed.TotalMilliseconds / $cores, 2)
        PeakMB     = [Math]::Round($peak / 1MB, 1)
    }
}

$process = Start-Process -FilePath $Exe -PassThru
try {
    Start-Sleep -Seconds 8
    $process.Refresh()
    $hwnd = $process.MainWindowHandle
    [void][Win32.User32]::SetForegroundWindow($hwnd)
    $focused = Measure-Phase -Process $process -Seconds $Seconds
    [void][Win32.User32]::ShowWindow($hwnd, 6)
    $minimized = Measure-Phase -Process $process -Seconds $Seconds
    [pscustomobject]@{ Phase = 'focus'; CpuPercent = $focused.CpuPercent; PeakMB = $focused.PeakMB }
    [pscustomobject]@{ Phase = 'minimise'; CpuPercent = $minimized.CpuPercent; PeakMB = $minimized.PeakMB }
} finally {
    Stop-Process -Id $process.Id -ErrorAction SilentlyContinue
}
