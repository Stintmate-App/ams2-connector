# Stops the running capture cleanly (stop file), waits up to 15 s and prints its summary.
#   stop-capture.ps1 [-Force]    -Force ends the process if it did not stop in time
# Plain PowerShell 5.1, no admin. See README.md in this folder.
param([switch]$Force)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '_common.ps1')

$cur = Get-CurrentCapture
if ($null -eq $cur) {
    Write-Output 'No capture is running (no current.json).'
    exit 0
}
$alive = Test-CaptureAlive $cur
if ($alive) {
    Set-Content -LiteralPath $cur.stopFile -Value 'stop' -Encoding ASCII
    $deadline = (Get-Date).AddSeconds(15)
    while ((Test-CaptureAlive $cur) -and (Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 250
    }
    $alive = Test-CaptureAlive $cur
    if ($alive) {
        if ($Force) {
            Stop-Process -Id ([int]$cur.pid) -Force
            Write-Output 'The capture did not stop in 15 s and was ended; the last seconds may be missing.'
        } else {
            Write-Output ('The capture is still running after 15 s (pid {0}). Run stop-capture.ps1 -Force to end it.' -f $cur.pid)
            exit 1
        }
    }
} else {
    Write-Output 'The capture process was no longer running (it ended by itself, for example when the game closed).'
}

Write-Output ('capture: {0}' -f $cur.file)
Write-CaptureSummary (Get-CaptureSummary $cur)
if (Test-Path -LiteralPath $cur.file) {
    Write-Output ('size: {0:N1} MB' -f ((Get-Item -LiteralPath $cur.file).Length / 1MB))
}
Remove-Item -LiteralPath $script:CurrentFile -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath $cur.stopFile -Force -ErrorAction SilentlyContinue
