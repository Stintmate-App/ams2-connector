<#
.SYNOPSIS
  Builds the AMS2 connector's installer: dist\AMS2-Connector-<version>[-dev]-setup.exe + .sha256.

.DESCRIPTION
  1. cargo deny check            (licences, advisories, sources)
  2. cargo build --release --locked   (the three programs)
  3. cargo about generate        (THIRD-PARTY-NOTICES.txt from installer\THIRD-PARTY-NOTICES.hbs)
  4. Inno Setup 6 (ISCC) packs dist\stage into the setup; a .sha256 file is written beside it.

  -Dev builds the development flavour: "-dev" in the file name and its own identity (AppId,
  name, folder), so installing it never replaces or removes the real connector.
  -SkipDeny skips step 1 (offline work); a release build must not use it.

  Needs: Rust (cargo), cargo-deny and cargo-about (cargo install cargo-deny cargo-about) and
  Inno Setup 6 (https://jrsoftware.org/isdl.php; per user or for all users).
#>
[CmdletBinding()]
param(
    [switch]$Dev,
    [switch]$SkipDeny
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$dist = Join-Path $root 'dist'
$stage = Join-Path $dist 'stage'
$clock = [System.Diagnostics.Stopwatch]::StartNew()

function Fail([string]$Message) {
    Write-Host "error: $Message" -ForegroundColor Red
    exit 1
}

function Step([string]$Title) {
    Write-Host ""
    Write-Host "== $Title" -ForegroundColor Cyan
}

function Run([string]$What, [scriptblock]$Command) {
    & $Command
    if ($LASTEXITCODE -ne 0) { Fail "$What failed (exit code $LASTEXITCODE)" }
}

# ---- tools ----
$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
if (Test-Path $cargoBin) { $env:PATH = "$cargoBin;$env:PATH" }

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Fail 'cargo not found. Install Rust with rustup (https://rustup.rs); the version is pinned by rust-toolchain.toml.'
}
$null = & cargo deny --version 2>&1
if ($LASTEXITCODE -ne 0) { Fail 'cargo-deny not found. Install it with: cargo install cargo-deny --locked' }
$null = & cargo about --version 2>&1
if ($LASTEXITCODE -ne 0) { Fail 'cargo-about not found. Install it with: cargo install cargo-about --locked' }

$iscc = $null
foreach ($candidate in @(
        (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe'),
        (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'),
        (Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe'))) {
    if ($candidate -and (Test-Path $candidate)) { $iscc = $candidate; break }
}
if (-not $iscc) { Fail 'Inno Setup 6 (ISCC.exe) not found. Install it from https://jrsoftware.org/isdl.php (per user or for all users).' }

# ---- version (the workspace's) ----
$manifest = Get-Content (Join-Path $root 'Cargo.toml') -Raw
if ($manifest -notmatch '(?ms)\[workspace\.package\].*?^version\s*=\s*"(\d+)\.(\d+)\.(\d+)[^"]*"') {
    Fail 'could not read [workspace.package] version from Cargo.toml'
}
$version = "$($Matches[1]).$($Matches[2]).$($Matches[3])"
$versionInfo = "$version.0"
$flavour = if ($Dev) { '-dev' } else { '' }
$setupName = "AMS2-Connector-$version$flavour-setup"
Write-Host "AMS2 connector $version$flavour"

Push-Location $root
try {
    # ---- 1. licences and advisories ----
    if ($SkipDeny) {
        Write-Host 'cargo deny skipped (-SkipDeny)' -ForegroundColor Yellow
    } else {
        Step 'cargo deny check'
        Run 'cargo deny check' { cargo deny check }
    }

    # ---- 2. release build ----
    Step 'cargo build --release --locked'
    Run 'cargo build' {
        cargo build --release --locked -p ams2-recorder -p ams2-connector-ui -p ams2-connector-window
    }

    # ---- 3. the stage ----
    Step 'stage'
    if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
    New-Item -ItemType Directory -Path $stage -Force | Out-Null
    $release = Join-Path $root 'target\release'
    foreach ($exe in 'ams2-connector-ui.exe', 'ams2-connector-window.exe', 'ams2-recorder.exe') {
        $path = Join-Path $release $exe
        if (-not (Test-Path $path)) { Fail "$exe was not built ($path)" }
        Copy-Item $path $stage
    }
    foreach ($doc in 'LICENSE', 'NOTICE') { Copy-Item (Join-Path $root $doc) $stage }
    Copy-Item (Join-Path $root 'installer\README.txt') $stage
    # The connector's icon, declared in the descriptor (the file next to the control app).
    Copy-Item (Join-Path $root 'bins\ams2-connector-ui\assets\icon.png') $stage

    Run 'cargo about generate' {
        cargo about generate --config about.toml --output-file (Join-Path $stage 'THIRD-PARTY-NOTICES.txt') installer\THIRD-PARTY-NOTICES.hbs
    }
    if (-not (Test-Path (Join-Path $stage 'THIRD-PARTY-NOTICES.txt'))) { Fail 'THIRD-PARTY-NOTICES.txt was not generated' }

    # The control app's icon is the setup's icon.
    $icon = Get-Item (Join-Path $root 'bins\ams2-connector-ui\assets\icon.ico')

    # ---- 4. Inno Setup ----
    Step 'Inno Setup'
    $isccArgs = @('/Q', "/DVersion=$version", "/DVersionInfo=$versionInfo", "/DStage=$stage", "/DOutDir=$dist", "/DOutName=$setupName")
    if ($Dev) { $isccArgs += '/DDev' }
    if ($icon) { $isccArgs += "/DSetupIcon=$($icon.FullName)" }
    $isccArgs += (Join-Path $root 'installer\ams2-connector.iss')
    Run 'ISCC' { & $iscc @isccArgs }

    $setup = Join-Path $dist "$setupName.exe"
    if (-not (Test-Path $setup)) { Fail "the setup was not produced ($setup)" }
    $hash = (Get-FileHash $setup -Algorithm SHA256).Hash.ToLower()
    # sha256sum format (binary mode marker), written without a BOM
    [System.IO.File]::WriteAllText("$setup.sha256", "$hash *$setupName.exe`n", (New-Object System.Text.UTF8Encoding($false)))

    $size = (Get-Item $setup).Length
    Step 'done'
    Write-Host ("{0}  ({1:N1} MB)" -f $setup, ($size / 1MB))
    Write-Host "sha256  $hash"
    Write-Host ("built in {0:N0} s" -f $clock.Elapsed.TotalSeconds)
} finally {
    Pop-Location
}
