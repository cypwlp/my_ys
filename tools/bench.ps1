# Same-corpus benchmark: the Rust port against the .NET original.
#
# Both sides are driven the same way -- external wall-clock around their own `a` and
# `x` verbs, release binaries on both -- because an internal benchmark in one of them
# is not comparable to a different internal benchmark in the other.
# ASCII only: PowerShell 5.1 reads .ps1 as ANSI.
param(
    [int]$Megabytes = 20,
    [switch]$Keep
)

$ErrorActionPreference = "Continue"
$root = Split-Path -Parent $PSScriptRoot
$rust = Join-Path $root "target\debug\qarc-cli.exe"
$rustRelease = Join-Path $root "target\release\qarc-cli.exe"
$dotnet = "D:\WORK\YS\QuickArcNET\src\QuickArc.Cli\bin\Release\net9.0\qarc.exe"
$bench = Join-Path $root ".bench"

if (Test-Path $rustRelease) { $rust = $rustRelease; Write-Host "using release CLI" } else { Write-Host "WARNING: using debug CLI" }
foreach ($p in @($rust, $dotnet)) {
    if (-not (Test-Path $p)) { throw "missing binary: $p" }
}

if (Test-Path $bench) { Remove-Item -Recurse -Force $bench }
New-Item -ItemType Directory -Force -Path "$bench\src\text", "$bench\src\mixed" | Out-Null

# Deterministic corpus: half compressible text, half incompressible noise.
$per = [Math]::Max(1, [int]($Megabytes / 2))
python -c @"
import os, pathlib
text = pathlib.Path(r'$bench\src\text')
noise = pathlib.Path(r'$bench\src\mixed')
for i in range(12):
    (text / f'doc{i:02d}.txt').write_bytes((b'quarterly report line %d\n' % i) * 40000)
for i in range($per):
    (noise / f'blob{i:02d}.bin').write_bytes(os.urandom(1024*1024))
print('corpus files', len(list(text.iterdir())) + len(list(noise.iterdir())))
"@

function Timed([scriptblock]$body) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $out = & $body 2>&1
    $sw.Stop()
    [pscustomobject]@{ seconds = [Math]::Round($sw.Elapsed.TotalSeconds, 2); out = ($out | Select-Object -Last 1) }
}

function Size($path) { if (Test-Path $path) { [Math]::Round((Get-Item $path).Length / 1MB, 2) } else { 0 } }

$rows = @()
$srcBytes = (Get-ChildItem "$bench\src" -Recurse -File | Measure-Object Length -Sum).Sum
Write-Host ("corpus = {0:N1} MB in {1} files" -f ($srcBytes / 1MB), (Get-ChildItem "$bench\src" -Recurse -File).Count)

foreach ($case in @(
        @{ tag = "7z"; ext = "7z" },
        @{ tag = "zip"; ext = "zip" },
        @{ tag = "tar.gz"; ext = "tar.gz" }
    )) {
    foreach ($side in @(@{ name = "rust"; exe = $rust }, @{ name = "dotnet"; exe = $dotnet })) {
        $arc = "$bench\out-$($side.name)-$($case.tag).$($case.ext)"
        
        $src = "$bench\src"
        $mk = Timed { & $side.exe a $arc $src }
        $exDir = "$bench\x-$($side.name)-$($case.tag)"
        $ex = Timed { & $side.exe x $arc "-o$exDir" }
        $rows += [pscustomobject]@{
            format  = $case.tag
            side    = $side.name
            createS = $mk.seconds
            sizeMB  = Size $arc
            ratio   = [Math]::Round(1 - ((Size $arc) * 1MB) / $srcBytes, 3)
            extractS = $ex.seconds
            note    = ("{0}" -f $mk.out)
        }
    }
}

$rows | Format-Table -AutoSize
$rows | ConvertTo-Json | Set-Content -Encoding UTF8 "$bench\bench.json"
Write-Host "detail -> $bench\bench.json"

if (-not $Keep) {
    Remove-Item -Recurse -Force $bench -ErrorAction SilentlyContinue
}
