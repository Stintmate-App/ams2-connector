# Prints whether a capture is running, its file size and the frames so far.
#   status-capture.ps1
# The frame count comes from the summary JSON, refreshed about every 10 s.
# Plain PowerShell 5.1, no admin. See README.md in this folder.
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '_common.ps1')

$cur = Get-CurrentCapture
if ($null -eq $cur) {
    Write-Output 'No capture is running.'
    exit 0
}
if (Test-CaptureAlive $cur) {
    Write-Output ('A capture is running (pid {0}), started {1}.' -f $cur.pid, $cur.started)
} else {
    Write-Output 'The capture process is no longer running (stop-capture.ps1 prints its summary and clears it).'
}
Write-Output ('file: {0}' -f $cur.file)
if (Test-Path -LiteralPath $cur.file) {
    Write-Output ('size: {0:N1} MB' -f ((Get-Item -LiteralPath $cur.file).Length / 1MB))
}
$s = Get-CaptureSummary $cur
if ($null -ne $s) {
    if ($s.state -eq 'waiting') { Write-Output 'waiting for the game (the mapping does not exist yet)' }
    Write-Output ('state: {0}, frames so far: {1}, capture seconds: {2:N0}' -f $s.state, $s.frames, $s.capture_seconds)
    if ($s.cars) { Write-Output ('cars: {0}' -f ($s.cars -join '; ')) }
    if ($s.tracks) { Write-Output ('tracks: {0}' -f ($s.tracks -join '; ')) }
    if ($s.session_types) { Write-Output ('session types: {0}' -f ($s.session_types -join ', ')) }
}
