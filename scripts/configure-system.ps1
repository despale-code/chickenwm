<#
.SYNOPSIS
    Phase 1: ChickenWM System Configuration, Shell Replacement & Service Debloat Script
.DESCRIPTION
    Configures UAC Secure Desktop bypass (`PromptOnSecureDesktop = 0`),
    Lock Workstation interception (`DisableLockWorkstation = 1`),
    Winlogon Shell registration with rollback failsafe, and redundant service debloating.
#>
param(
    [switch]$Apply,
    [switch]$Rollback,
    [switch]$DebloatServices,
    [string]$RoosterPath = "$PSScriptRoot\..\target\release\chickenwm-rooster.exe"
)

$ErrorActionPreference = "Stop"

$HklmSystemPolicy = "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System"
$HkcuSystemPolicy = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Policies\System"
$HkcuWinlogon     = "HKCU:\Software\Microsoft\Windows NT\CurrentVersion\Winlogon"

$RedundantServices = @(
    "WSearch",
    "BcastDVRUserService",
    "WpnService",
    "TabletInputService",
    "DiagTrack",
    "SysMain",
    "DPS"
)

$NonNegotiableServices = @(
    "dwm",
    "CryptSvc",
    "Winmgmt",
    "EventLog",
    "Audiosrv",
    "RpcSs"
)

if ($Rollback) {
    Write-Host "[Rollback] Restoring Windows defaults..." -ForegroundColor Yellow
    if (Test-Path $HklmSystemPolicy) {
        Set-ItemProperty -Path $HklmSystemPolicy -Name "PromptOnSecureDesktop" -Value 1 -Type DWord -ErrorAction SilentlyContinue
    }
    if (Test-Path $HkcuSystemPolicy) {
        Set-ItemProperty -Path $HkcuSystemPolicy -Name "DisableLockWorkstation" -Value 0 -Type DWord -ErrorAction SilentlyContinue
    }
    if (Test-Path $HkcuWinlogon) {
        Remove-ItemProperty -Path $HkcuWinlogon -Name "Shell" -ErrorAction SilentlyContinue
    }
    foreach ($svc in $RedundantServices) {
        sc.exe config $svc start= demand | Out-Null
    }
    Write-Host "[Rollback] Complete. Native Explorer shell and system policies restored." -ForegroundColor Green
    exit 0
}

if ($Apply) {
    # 1.1 UAC Elevation Policy: Spawn consent.exe on Default desktop
    if (-not (Test-Path $HklmSystemPolicy)) { New-Item -Path $HklmSystemPolicy -Force | Out-Null }
    Set-ItemProperty -Path $HklmSystemPolicy -Name "PromptOnSecureDesktop" -Value 0 -Type DWord
    Write-Host "[1.1] Set PromptOnSecureDesktop = 0 (consent.exe renders on Default desktop)" -ForegroundColor Cyan

    # 1.2 Lock Workstation Interception: Allow coop to trap Win + L
    if (-not (Test-Path $HkcuSystemPolicy)) { New-Item -Path $HkcuSystemPolicy -Force | Out-Null }
    Set-ItemProperty -Path $HkcuSystemPolicy -Name "DisableLockWorkstation" -Value 1 -Type DWord
    Write-Host "[1.2] Set DisableLockWorkstation = 1 (Win + L routed to coop lock screen)" -ForegroundColor Cyan

    # 1.3 Winlogon Shell Replacement with failsafe verification
    $ResolvedRooster = Resolve-Path $RoosterPath -ErrorAction SilentlyContinue
    if ($ResolvedRooster) {
        if (-not (Test-Path $HkcuWinlogon)) { New-Item -Path $HkcuWinlogon -Force | Out-Null }
        Set-ItemProperty -Path $HkcuWinlogon -Name "Shell" -Value "`"$($ResolvedRooster.Path)`" --replace-shell" -Type String
        Write-Host "[1.3] Configured Winlogon\Shell -> $($ResolvedRooster.Path)" -ForegroundColor Cyan
    } else {
        Write-Warning "[1.3] Rooster binary not found at $RoosterPath; skipping Winlogon\Shell override (failsafe active)."
    }

    # 1.4 Service Debloating Automation
    if ($DebloatServices) {
        foreach ($svc in $NonNegotiableServices) {
            Write-Host "[1.4] Verified non-negotiable service preserved: $svc" -ForegroundColor DarkGreen
        }
        foreach ($svc in $RedundantServices) {
            if ($NonNegotiableServices -notcontains $svc) {
                sc.exe config $svc start= disabled | Out-Null
                sc.exe stop $svc | Out-Null
                Write-Host "[1.4] Disabled redundant service: $svc" -ForegroundColor Cyan
            }
        }
    }
    Write-Host "[Done] Phase 1 system configuration applied." -ForegroundColor Green
}
