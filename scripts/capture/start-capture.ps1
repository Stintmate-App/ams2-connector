# Starts a raw capture of the game's shared memory in the background and prints the file path.
#   start-capture.ps1 [-Name <label>] [-Exe <path to the dev ams2-recorder.exe>]
# Works before the game is started: the capture waits for the game's mapping.
# Plain PowerShell 5.1, no admin. See README.md in this folder.
param(
    [string]$Name = 'session',
    [string]$Exe,
    [string]$ShmName   # test hook: capture a test mapping instead of the game
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '_common.ps1')

$cur = Get-CurrentCapture
if (Test-CaptureAlive $cur) {
    Write-Output ("A capture is already running (pid {0}): {1}" -f $cur.pid, $cur.file)
    Write-Output 'Stop it first with stop-capture.ps1.'
    exit 1
}
if ($null -ne $cur) { Remove-Item -LiteralPath $script:CurrentFile -Force -ErrorAction SilentlyContinue }

if (-not $Exe) {
    $candidates = @(
        (Join-Path $script:CaptureDir 'ams2-recorder.exe'),
        (Join-Path $PSScriptRoot '..\..\target\release\ams2-recorder.exe')
    )
    $Exe = $candidates | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
}
if (-not $Exe -or -not (Test-Path -LiteralPath $Exe)) {
    Write-Output 'The dev recorder (built with --features capture-raw) was not found.'
    Write-Output ('Put it at {0} or pass -Exe <path>.' -f (Join-Path $script:CaptureDir 'ams2-recorder.exe'))
    exit 1
}
$Exe = (Resolve-Path -LiteralPath $Exe).Path

$label = ($Name -replace '[^A-Za-z0-9_-]', '-')
if (-not $label) { $label = 'session' }
New-Item -ItemType Directory -Force -Path $script:CaptureDir | Out-Null
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$file = Join-Path $script:CaptureDir ("{0}_{1}.a2raw" -f $stamp, $label)
$stop = $file + '.stop'
$log = $file + '.log'
Remove-Item -LiteralPath $stop -Force -ErrorAction SilentlyContinue

$argLine = 'capture-raw "{0}" --stop-file "{1}"' -f $file, $stop
if ($ShmName) { $argLine += ' --shm-name "' + $ShmName + '"' }
$p = Start-Process -FilePath $Exe -ArgumentList $argLine -WindowStyle Hidden -PassThru `
    -RedirectStandardOutput $log -RedirectStandardError ($file + '.err')

Start-Sleep -Milliseconds 1200
if ($p.HasExited) {
    Write-Output ('The capture ended at once (exit code {0}):' -f $p.ExitCode)
    Get-Content -LiteralPath $log, ($file + '.err') -ErrorAction SilentlyContinue | Write-Output
    exit 1
}

$state = [ordered]@{
    pid           = $p.Id
    startFileTime = $p.StartTime.ToFileTimeUtc()
    started       = (Get-Date).ToString('o')
    file          = $file
    json          = $file + '.json'
    stopFile      = $stop
    log           = $log
}
$state | ConvertTo-Json | Set-Content -LiteralPath $script:CurrentFile -Encoding UTF8
Write-Output ('Capture started (pid {0}); it waits for the game if it is not running.' -f $p.Id)
Write-Output $file
