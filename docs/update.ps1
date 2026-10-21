# ChickenWM updater.
#
#   irm https://raw.githubusercontent.com/despale-code/chickenwm/main/docs/update.ps1 | iex
#
# Compares every installed ChickenWM program with its GitHub repository, then downloads, builds and
# installs the ones that changed in parallel with a live progress bar.
# Only check, without updating:
#
#   $env:CHICKENWM_UPDATE = "check"; irm https://raw.githubusercontent.com/despale-code/chickenwm/main/docs/update.ps1 | iex
#
# `chickenctl update` and Settings > General > Updates run the same thing. The work is done by the
# installer, so updates always use its latest version.

if (-not $env:CHICKENWM_UPDATE) { $env:CHICKENWM_UPDATE = '1' }

$localInstall = Join-Path $PSScriptRoot 'install.ps1'
if ($PSScriptRoot -and (Test-Path $localInstall)) {
    & $localInstall
} else {
    $script = Invoke-RestMethod 'https://raw.githubusercontent.com/despale-code/chickenwm/main/docs/install.ps1'
    Invoke-Expression $script
}
