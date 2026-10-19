# ChickenWM updater.
#
#   irm https://despale-code.github.io/chickenwm/update.ps1 | iex
#
# Compares every installed ChickenWM program with its GitHub repository, then downloads, builds and
# installs the ones that changed, one by one (ChickenWM restarts at the end if it was running).
# Only check, without updating:
#
#   $env:CHICKENWM_UPDATE = "check"; irm https://despale-code.github.io/chickenwm/update.ps1 | iex
#
# `chickenctl update` and Settings > General > Updates run the same thing. The work is done by the
# installer, so updates always use its latest version.

if (-not $env:CHICKENWM_UPDATE) { $env:CHICKENWM_UPDATE = '1' }
Invoke-Expression (Invoke-RestMethod 'https://despale-code.github.io/chickenwm/install.ps1')
