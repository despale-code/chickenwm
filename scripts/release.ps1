<#
.SYNOPSIS
    Builds ChickenWM in release mode and packages it: dist\chickenwm-<version>-x64.zip plus a
    .sha256 file next to it.

.PARAMETER NoBuild
    Package what is already in target\release (skip `cargo build`).

.PARAMETER OutDir
    Where the zip and checksum go (default: dist\ in the repository).

.EXAMPLE
    .\scripts\release.ps1
    .\scripts\release.ps1 -NoBuild
#>
param(
    [switch]$NoBuild,
    [string]$OutDir = ""
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
if (-not $OutDir) { $OutDir = Join-Path $root "dist" }

# Version from [workspace.package] in Cargo.toml.
$cargo = Get-Content (Join-Path $root "Cargo.toml") -Raw
if ($cargo -notmatch '(?ms)\[workspace\.package\].*?^version\s*=\s*"([^"]+)"') { throw "No workspace version in Cargo.toml" }
$version = $Matches[1]
$name = "chickenwm-$version-x64"

if (-not $NoBuild) {
    Write-Host "Building release ($version)..."
    Push-Location $root
    try {
        cargo build --release --workspace
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed (is ChickenWM running? Its exes are locked while they run)" }
    } finally { Pop-Location }
}

$release = Join-Path $root "target\release"
$files = @(Get-ChildItem $release -File | Where-Object { $_.Extension -in ".exe", ".dll" })
if ($files.Count -eq 0) { throw "Nothing in $release - build first or drop -NoBuild" }

# Stage: binaries, assets, readme and license.
$stage = Join-Path ([System.IO.Path]::GetTempPath()) "$name-$PID"
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
$pkg = Join-Path $stage $name
New-Item -ItemType Directory -Force $pkg | Out-Null
foreach ($f in $files) { Copy-Item $f.FullName $pkg }
foreach ($extra in "assets", "README.md", "LICENSE") {
    $p = Join-Path $root $extra
    if (Test-Path $p) { Copy-Item $p $pkg -Recurse }
}

New-Item -ItemType Directory -Force $OutDir | Out-Null
$zip = Join-Path $OutDir "$name.zip"
if (Test-Path $zip) { Remove-Item $zip -Force }
Compress-Archive -Path $pkg -DestinationPath $zip -CompressionLevel Optimal
Remove-Item $stage -Recurse -Force

$hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
$sumFile = "$zip.sha256"
# `sha256sum -c` format: "<hash>  <file name>".
Set-Content -Path $sumFile -Value "$hash  $name.zip" -Encoding ascii -NoNewline

$size = "{0:N1} MB" -f ((Get-Item $zip).Length / 1MB)
Write-Host "Packaged $($files.Count) binaries -> $zip ($size)"
Write-Host "SHA-256 $hash"
