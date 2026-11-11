# One entry point so the toolchain requirement is not tribal knowledge.
#
# Rust here is x86_64-pc-windows-gnu, and windows-rs generates raw-dylib import
# libraries with dlltool.  Without winlibs on PATH the build dies with
#   error calling dlltool 'dlltool.exe': program not found
# which looks like a code problem and is not.
param(
    [ValidateSet("core", "app", "web", "all")] [string]$Target = "all",
    [switch]$Release
)

$env:PATH = "D:\mingw64\mingw64\bin;" + $env:PATH

function Invoke-Step($name, [scriptblock]$body) {
    Write-Host "=== $name ==="
    & $body
    if ($LASTEXITCODE -ne 0) { Write-Error "$name failed with exit $LASTEXITCODE"; exit $LASTEXITCODE }
}

$root = Split-Path -Parent $PSScriptRoot

if ($Target -in @("core", "all")) {
    Push-Location (Join-Path $root "crates/core")
    Invoke-Step "cargo test (qarc-core)" { if ($Release) { cargo test --release } else { cargo test } }
    Pop-Location
}

if ($Target -in @("web", "all")) {
    Push-Location (Join-Path $root "web")
    Invoke-Step "tsc --noEmit" { npx tsc --noEmit }
    Invoke-Step "vite build" { npx vite build }
    Pop-Location
}

if ($Target -in @("app", "all")) {
    Push-Location (Join-Path $root "src-tauri")
    # custom-protocol is not optional: without it a release binary loads devUrl
    # and the window shows the webview error page instead of the app.
    Invoke-Step "cargo build (quickarc)" { if ($Release) { cargo build --release --features custom-protocol } else { cargo build } }
    Pop-Location
}

Write-Host "OK"
