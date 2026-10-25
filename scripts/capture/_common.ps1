# Shared by start-capture.ps1, stop-capture.ps1 and status-capture.ps1 (dot-sourced).
# Plain PowerShell 5.1, no admin.

$script:CaptureDir = if ($env:STINTMATE_CAPTURE_DIR) { $env:STINTMATE_CAPTURE_DIR } else { Join-Path $env:LOCALAPPDATA 'Stintmate\dev-captures' }  # the override is for tests
$script:CurrentFile = Join-Path $script:CaptureDir 'current.json'

# The state of the running capture (current.json), or $null.
function Get-CurrentCapture {
    if (-not (Test-Path -LiteralPath $script:CurrentFile)) { return $null }
    try {
        return Get-Content -LiteralPath $script:CurrentFile -Raw | ConvertFrom-Json
    } catch {
        return $null
    }
}

# True while the recorded capture process (same PID and same start time) is alive.
function Test-CaptureAlive($Current) {
    if ($null -eq $Current) { return $false }
    $p = Get-Process -Id ([int]$Current.pid) -ErrorAction SilentlyContinue
    if ($null -eq $p) { return $false }
    try {
        return $p.StartTime.ToFileTimeUtc() -eq [int64]$Current.startFileTime
    } catch {
        return $false
    }
}

# The summary JSON next to the capture file, or $null.
function Get-CaptureSummary($Current) {
    if ($null -eq $Current -or -not (Test-Path -LiteralPath $Current.json)) { return $null }
    try {
        return Get-Content -LiteralPath $Current.json -Raw | ConvertFrom-Json
    } catch {
        return $null
    }
}

function Write-CaptureSummary($Summary) {
    if ($null -eq $Summary) { Write-Output 'summary: not written yet'; return }
    Write-Output ("state: {0}" -f $Summary.state)
    Write-Output ("frames: {0}" -f $Summary.frames)
    Write-Output ("capture seconds: {0:N1}" -f $Summary.capture_seconds)
    Write-Output ("first/last timestamp (us): {0} / {1}" -f $Summary.first_timestamp_us, $Summary.last_timestamp_us)
    Write-Output ("cars: {0}" -f ($Summary.cars -join '; '))
    Write-Output ("tracks: {0}" -f ($Summary.tracks -join '; '))
    Write-Output ("session types: {0}" -f ($Summary.session_types -join ', '))
    Write-Output ("file bytes: {0}" -f $Summary.file_bytes)
    Write-Output ("cpu seconds: {0}" -f $Summary.cpu_seconds)
    Write-Output ("waited for the game (s): {0}" -f $Summary.waited_for_game_seconds)
    Write-Output ("stop reason: {0}" -f $Summary.stop_reason)
}
