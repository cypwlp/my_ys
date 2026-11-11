# End-to-end UI check in the real window, in one process tree.
#
# The app is launched with an archive path so the Explorer-verb route is exercised
# too, and every step is captured: an interaction that "should" work is not evidence.
# ASCII only on purpose -- PowerShell 5.1 reads .ps1 as ANSI and non-ASCII literals
# break string terminators.
param([string]$Archive = "")
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$shots = Join-Path $root ".shots"
New-Item -ItemType Directory -Force -Path $shots | Out-Null
$env:PATH = "D:\mingw64\mingw64\bin;" + $env:PATH

$archive = if ($Archive) { $Archive } else { Join-Path $root ".e2e\out.7z" }
if (-not (Test-Path $archive)) { throw "no fixture at $archive - create one with qarc-cli a first" }

$capture = "D:\WORK\YS\QuickArcNET\tools\shot.ps1"
$clicker = "D:\WORK\YS\QuickArcNET\tools\click.ps1"

$vite = Start-Process -FilePath "cmd.exe" `
    -ArgumentList "/c cd /d $root\web && npx vite --port 1420 --strictPort" `
    -WindowStyle Hidden -PassThru
Start-Sleep -Seconds 14

$app = Start-Process -FilePath "$root\src-tauri\target\debug\quickarc.exe" `
    -ArgumentList "`"$archive`"" -WorkingDirectory "$root\src-tauri" -PassThru
Start-Sleep -Seconds 13

function Shot($name) {
    & powershell -NoProfile -ExecutionPolicy Bypass -File $capture -Process quickarc -Out (Join-Path $shots $name) | Out-Null
    Write-Host "  -> $name"
}

function Click($x, $y, $what) {
    # Windows refuses SetForegroundWindow from a background process, so the first
    # click only wakes the window up.  Clicking dead space is what wakes it.
    & powershell -NoProfile -ExecutionPolicy Bypass -File $clicker -X 600 -Y 420 | Out-Null
    Start-Sleep -Milliseconds 400
    & powershell -NoProfile -ExecutionPolicy Bypass -File $clicker -X $x -Y $y | Out-Null
    Write-Host "  clicked $what at $x,$y"
    Start-Sleep -Seconds 2
}

Write-Host ("alive=" + [bool](Get-Process -Id $app.Id -ErrorAction SilentlyContinue))

Shot "ui-1-loaded.png"
Click 310 55 "test"
Shot "ui-2-test-job.png"
Click 245 55 "compress"
Shot "ui-3-compress-dialog.png"
Click 60 620 "dismiss"
Click 160 55 "smart-extract"
Start-Sleep -Seconds 4
Shot "ui-4-extract-job.png"

Stop-Process -Id $app.Id -Force -ErrorAction SilentlyContinue
Stop-Process -Id $vite.Id -Force -ErrorAction SilentlyContinue
Get-Process node -ErrorAction SilentlyContinue | Where-Object { $_.StartTime -gt (Get-Date).AddMinutes(-3) } |
    Stop-Process -Force -ErrorAction SilentlyContinue
Write-Host "done"
