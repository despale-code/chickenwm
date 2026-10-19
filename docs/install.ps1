# ChickenWM installer.
#
#   irm https://despale-code.github.io/chickenwm/install.ps1 | iex
#
# Downloads the source of each ChickenWM program from GitHub, builds it with Cargo and installs it
# for the current user (no administrator rights): programs in %LOCALAPPDATA%\Programs\ChickenWM,
# Start Menu shortcuts, App Paths (so Win+R knows them), chickenctl on PATH, and an entry in
# Settings > Apps for uninstalling. Running it again updates what is installed.
#
# Options (environment variables, since `irm | iex` can't pass parameters):
#   $env:CHICKENWM_COMPONENTS = "henbrowser,chickenwm-files"   optional programs to install (skip the picker)
#   $env:CHICKENWM_ALL = "1"                                   install every optional program
#   $env:CHICKENWM_YES = "1"                                   don't ask (installs Rust if needed)
#   $env:CHICKENWM_DIR = "D:\Apps\ChickenWM"                   install somewhere else
#   $env:CHICKENWM_UPDATE = "1"                                update: rebuild only the programs whose
#                                                              repository changed (or "check": just list them)
# Updating: update.ps1 next to this file, `chickenctl update`, or Settings > General > Updates.
# Uninstall: Settings > Apps > ChickenWM, or run `uninstall.ps1` in the install folder.

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$Org = 'despale-code'
$Version = '0.1.1'

# Every program: repository, files it installs, Start Menu shortcut (if any), whether it is
# required, and whether it is picked by default.
$Catalog = @(
    @{ Repo = 'chickenwm-rooster';  Title = 'Rooster (daemon)';        Files = @('chickenwm-rooster.exe');  Shortcut = 'ChickenWM';            Required = $true;  Default = $true;  About = 'Starts and supervises everything, tray icon, failsafe' }
    @{ Repo = 'chickenwm-wm';       Title = 'Window manager';          Files = @('chickenwm-wm.exe');       Shortcut = $null;                  Required = $true;  Default = $true;  About = 'Tiling, workspaces, animations' }
    @{ Repo = 'chickenwm-binds';    Title = 'Keybindings';             Files = @('chickenwm-binds.exe');    Shortcut = $null;                  Required = $true;  Default = $true;  About = 'Global hotkeys and mouse binds' }
    @{ Repo = 'chickenwm-settings'; Title = 'Settings';                Files = @('chickenwm-settings.exe'); Shortcut = 'ChickenWM Settings';   Required = $true;  Default = $true;  About = 'The settings app' }
    @{ Repo = 'chickenctl';         Title = 'chickenctl';              Files = @('chickenctl.exe');         Shortcut = $null;                  Required = $true;  Default = $true;  About = 'Command-line control (added to PATH)' }
    @{ Repo = 'roobar';             Title = 'roobar';                  Files = @('roobar.exe');             Shortcut = $null;                  Required = $false; Default = $true;  About = 'Status bar' }
    @{ Repo = 'chickenwm-launcher'; Title = 'Launcher';                Files = @('chickenwm-launcher.exe'); Shortcut = $null;                  Required = $false; Default = $true;  About = 'App launcher, calculator, clipboard history' }
    @{ Repo = 'chickenwm-notify';   Title = 'Notifications';           Files = @('chickenwm-notify.exe');   Shortcut = $null;                  Required = $false; Default = $true;  About = 'Toasts and notification center' }
    @{ Repo = 'chickenwm-paper';    Title = 'Wallpapers';              Files = @('chickenwm-paper.exe');    Shortcut = $null;                  Required = $false; Default = $true;  About = 'Image and shader wallpapers' }
    @{ Repo = 'chickenwm-files';    Title = 'Files';                   Files = @('chickenwm-files.exe');    Shortcut = 'ChickenWM Files';      Required = $false; Default = $true;  About = 'File explorer' }
    @{ Repo = 'chickenwm-shot';     Title = 'Screenshots';             Files = @('chickenwm-shot.exe');     Shortcut = 'ChickenWM Screenshot'; Required = $false; Default = $true;  About = 'Region, window and screen captures' }
    @{ Repo = 'henbrowser';         Title = 'Hen Browser';             Files = @('henbrowser.exe', 'WebView2Loader.dll'); Shortcut = 'Hen Browser'; Required = $false; Default = $true; About = 'Web browser on WebView2' }
    @{ Repo = 'chickenwm-dwm-hook'; Title = 'DWM hook (experimental)'; Files = @('chickenwm-dwm-hook.exe', 'chickenwm_dwm_hook.dll'); Shortcut = $null; Required = $false; Default = $false; About = 'WARNING: can trip game anti-cheat; leave off if you play online' }
)

$InstallDir = if ($env:CHICKENWM_DIR) { $env:CHICKENWM_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\ChickenWM' }
$CacheDir = Join-Path $env:LOCALAPPDATA 'ChickenWM\build'
$StartMenu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
$UninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\ChickenWM'
$AppPaths = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\App Paths'
$Assume = $env:CHICKENWM_YES -eq '1'

function Say($text, $color = 'Gray') { Write-Host $text -ForegroundColor $color }
function Step($text) { Write-Host ''; Write-Host "==> $text" -ForegroundColor Yellow }
function Ask($question) {
    if ($Assume) { return $true }
    $answer = Read-Host "$question [Y/n]"
    return ($answer -eq '' -or $answer -match '^[yY]')
}

# --- Requirements -----------------------------------------------------------------------------

function Find-Tool($name) {
    $cmd = Get-Command $name -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    $local = Join-Path $env:USERPROFILE ".cargo\bin\$name.exe"
    if (Test-Path $local) { return $local }
    return $null
}

# Builds use Rust's GNU toolchain: it brings its own linker, so no Visual Studio is needed.
$GnuToolchain = 'stable-x86_64-pc-windows-gnu'

function Install-Requirements {
    Step 'Checking requirements'
    if (-not [Environment]::Is64BitOperatingSystem) { throw 'ChickenWM needs 64-bit Windows 10 or 11.' }

    $rustup = Find-Tool 'rustup'
    $script:Cargo = Find-Tool 'cargo'
    $script:Toolchain = @()
    if (-not $rustup -and -not $script:Cargo) {
        if (-not (Ask 'Rust is not installed. Install it now with rustup (the official installer, for your user only)?')) {
            throw 'Rust is required: https://rustup.rs'
        }
        $init = Join-Path $env:TEMP 'rustup-init.exe'
        Invoke-WebRequest 'https://win.rustup.rs/x86_64' -OutFile $init -UseBasicParsing
        & $init -y --default-host x86_64-pc-windows-gnu --default-toolchain stable --profile minimal
        if ($LASTEXITCODE -ne 0) { throw 'rustup failed.' }
        $rustup = Find-Tool 'rustup'
        $script:Cargo = Find-Tool 'cargo'
        if (-not $script:Cargo) { throw 'Rust installed, but cargo was not found. Open a new terminal and run this again.' }
    }
    if ($rustup) {
        & $rustup toolchain install $GnuToolchain --profile minimal --no-self-update | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "Couldn't install the Rust toolchain $GnuToolchain." }
        $script:Toolchain = @("+$GnuToolchain")
    } else {
        # Rust without rustup: use it as is; an MSVC Rust needs the Visual Studio C++ build tools.
        $rustHost = (& (Find-Tool 'rustc') -vV | Select-String '^host:').ToString()
        if ($rustHost -match 'msvc') { Say 'Your Rust uses the MSVC toolchain: it needs the Visual Studio C++ build tools.' 'DarkYellow' }
    }
    $tc = $script:Toolchain
    $cargoVersion = & $script:Cargo @tc --version
    Say "Rust: $cargoVersion" 'Green'

    $webview = Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}' -ErrorAction SilentlyContinue
    if (-not $webview) { Say 'Note: Hen Browser needs the WebView2 runtime (part of Windows 11; on Windows 10 get it from microsoft.com/edge/webview2).' 'DarkYellow' }
}

# --- Picking programs -------------------------------------------------------------------------

function Get-InstalledInfo {
    $file = Join-Path $InstallDir 'installed.json'
    if (Test-Path $file) { return (Get-Content $file -Raw | ConvertFrom-Json) }
    return $null
}

function Get-Installed {
    $info = Get-InstalledInfo
    if ($info) { return @($info.components) }
    return @()
}

# Progress for the Settings app, which runs this hidden and reads these lines:
# "@@ <repo> <state> [text]" with states checking, current, available, building, built, installed,
# failed; and "@@ done <count>" / "@@ error <text>" at the end.
$Machine = $env:CHICKENWM_MACHINE -eq '1'
function Report($repo, $state, $text = '') {
    if ($Machine) { [Console]::Out.WriteLine("@@ $repo $state $text"); [Console]::Out.Flush() }
}

# The newest commit of a program's repository (what an update would install).
function Get-LatestCommit($repo) {
    try {
        $r = Invoke-RestMethod "https://api.github.com/repos/$Org/$repo/commits/main" -Headers @{ 'User-Agent' = 'chickenwm-installer'; 'Accept' = 'application/vnd.github+json' }
        return $r.sha
    } catch {
        return $null
    }
}

function Confirm-DwmHook {
    Write-Host ''
    Say 'WARNING: the DWM hook injects code into dwm.exe, the Windows compositor.' 'Red'
    Say 'Games with anti-cheat (Easy Anti-Cheat, BattlEye, Riot Vanguard, FACEIT, ...) can treat that' 'Red'
    Say 'as tampering: they may refuse to start, and an account could be flagged or banned.' 'Red'
    Say 'It is experimental and can also make the desktop unstable.' 'Red'
    Say 'Installing it does not turn it on: it stays off until you pick "DWM hook" in Settings > Displays.' 'DarkYellow'
    return (Ask 'Install the DWM hook anyway?')
}

function Select-Components {
    $installed = Get-Installed
    $chosen = @{}
    foreach ($item in $Catalog) {
        $chosen[$item.Repo] = $item.Required -or ($(if ($installed.Count) { $installed -contains $item.Repo } else { $item.Default }))
    }
    if ($env:CHICKENWM_ALL -eq '1') {
        foreach ($item in $Catalog) { if ($item.Repo -ne 'chickenwm-dwm-hook') { $chosen[$item.Repo] = $true } }
    }
    if ($env:CHICKENWM_COMPONENTS) {
        $wanted = $env:CHICKENWM_COMPONENTS -split '[,; ]+' | Where-Object { $_ }
        foreach ($item in $Catalog) { $chosen[$item.Repo] = $item.Required -or ($wanted -contains $item.Repo) }
    }
    if ($Assume -or $env:CHICKENWM_COMPONENTS -or $env:CHICKENWM_ALL -eq '1') {
        # Asked for by name (or kept from an earlier install): still say what it means.
        if ($chosen['chickenwm-dwm-hook'] -and -not (Confirm-DwmHook)) { $chosen['chickenwm-dwm-hook'] = $false }
        return $chosen
    }

    while ($true) {
        Write-Host ''
        Say 'Programs to install (type a number to toggle it, Enter to continue):' 'Cyan'
        for ($i = 0; $i -lt $Catalog.Count; $i++) {
            $item = $Catalog[$i]
            $mark = if ($chosen[$item.Repo]) { '[x]' } else { '[ ]' }
            $lock = if ($item.Required) { ' (required)' } else { '' }
            $color = if ($item.Repo -eq 'chickenwm-dwm-hook') { 'DarkYellow' } elseif ($item.Required) { 'DarkGray' } else { 'Gray' }
            Write-Host ("  {0,2}. {1} {2,-26} {3}{4}" -f ($i + 1), $mark, $item.Title, $item.About, $lock) -ForegroundColor $color
        }
        $answer = Read-Host 'Toggle'
        if ($answer -eq '') { break }
        foreach ($part in ($answer -split '[,; ]+' | Where-Object { $_ })) {
            $n = 0
            if (-not [int]::TryParse($part, [ref]$n) -or $n -lt 1 -or $n -gt $Catalog.Count) { continue }
            $item = $Catalog[$n - 1]
            if ($item.Required) { continue }
            if ($item.Repo -eq 'chickenwm-dwm-hook' -and -not $chosen[$item.Repo]) {
                if (-not (Confirm-DwmHook)) { continue }
            }
            $chosen[$item.Repo] = -not $chosen[$item.Repo]
        }
    }
    return $chosen
}

# --- Building ---------------------------------------------------------------------------------

function Build-Component($item, $srcRoot, $commit) {
    $zip = Join-Path $srcRoot "$($item.Repo).zip"
    # A known commit is built exactly (so installed.json can say what is installed).
    $url = if ($commit) { "https://github.com/$Org/$($item.Repo)/archive/$commit.zip" } else { "https://github.com/$Org/$($item.Repo)/archive/refs/heads/main.zip" }
    Invoke-WebRequest $url -OutFile $zip -UseBasicParsing
    $dest = Join-Path $srcRoot $item.Repo
    if (Test-Path $dest) { Remove-Item $dest -Recurse -Force }
    Expand-Archive $zip -DestinationPath $dest -Force
    $manifest = Get-ChildItem $dest -Filter Cargo.toml -Recurse -Depth 1 | Select-Object -First 1
    if (-not $manifest) { throw "$($item.Repo): no Cargo.toml in the download" }
    $tc = $script:Toolchain
    & $script:Cargo @tc build --release --quiet --manifest-path $manifest.FullName
    if ($LASTEXITCODE -ne 0) { throw "$($item.Repo) failed to build (see the errors above)" }
}

# --- Installing -------------------------------------------------------------------------------

function New-Shortcut($path, $target, $description) {
    $shell = New-Object -ComObject WScript.Shell
    $lnk = $shell.CreateShortcut($path)
    $lnk.TargetPath = $target
    $lnk.WorkingDirectory = Split-Path $target
    $lnk.Description = $description
    $lnk.IconLocation = "$target,0"
    $lnk.Save()
}

function Stop-ChickenWM {
    $names = $Catalog | ForEach-Object { $_.Files } | Where-Object { $_ -like '*.exe' } | ForEach-Object { [IO.Path]::GetFileNameWithoutExtension($_) }
    $running = Get-Process -Name $names -ErrorAction SilentlyContinue
    if (-not $running) { return $false }
    Say "These are running and will be closed for the update: $(($running.Name | Sort-Object -Unique) -join ', ')" 'DarkYellow'
    if (-not (Ask 'Close them now?')) { throw 'Close ChickenWM first, then run this again.' }
    $running | Stop-Process -Force
    Start-Sleep -Milliseconds 800
    return $true
}

function Install-ChickenWM {
    Say "ChickenWM installer $Version" 'Cyan'
    Say "Installs to $InstallDir (your user only)."
    Install-Requirements
    $chosen = Select-Components
    $items = @($Catalog | Where-Object { $chosen[$_.Repo] })

    Step "Building $($items.Count) programs (the first build takes a while)"
    $srcRoot = Join-Path $env:TEMP 'chickenwm-install'
    New-Item -ItemType Directory -Force $srcRoot, $CacheDir | Out-Null
    $env:CARGO_TARGET_DIR = $CacheDir
    $commits = @{}
    $n = 0
    foreach ($item in $items) {
        $n++
        Say ("[{0}/{1}] {2}" -f $n, $items.Count, $item.Title) 'White'
        $commits[$item.Repo] = Get-LatestCommit $item.Repo
        Build-Component $item $srcRoot $commits[$item.Repo]
    }

    Step 'Installing'
    $wasRunning = Stop-ChickenWM
    New-Item -ItemType Directory -Force $InstallDir | Out-Null
    $release = Join-Path $CacheDir 'release'
    foreach ($item in $items) {
        foreach ($file in $item.Files) { Copy-Item (Join-Path $release $file) $InstallDir -Force }
    }

    # Programs that were installed before but are now unticked.
    foreach ($item in $Catalog | Where-Object { -not $chosen[$_.Repo] }) {
        foreach ($file in $item.Files) { Remove-Item (Join-Path $InstallDir $file) -Force -ErrorAction SilentlyContinue }
        if ($item.Shortcut) { Remove-Item (Join-Path $StartMenu "ChickenWM\$($item.Shortcut).lnk"), (Join-Path $StartMenu "$($item.Shortcut).lnk") -Force -ErrorAction SilentlyContinue }
        foreach ($file in $item.Files | Where-Object { $_ -like '*.exe' }) { Remove-Item (Join-Path $AppPaths $file) -Force -ErrorAction SilentlyContinue }
    }

    # Start Menu (Hen Browser is its own app, so it goes at the top level) and App Paths.
    New-Item -ItemType Directory -Force (Join-Path $StartMenu 'ChickenWM') | Out-Null
    foreach ($item in $items) {
        $exe = Join-Path $InstallDir $item.Files[0]
        if ($item.Shortcut) {
            $folder = if ($item.Repo -eq 'henbrowser') { $StartMenu } else { Join-Path $StartMenu 'ChickenWM' }
            New-Shortcut (Join-Path $folder "$($item.Shortcut).lnk") $exe $item.About
        }
        $key = Join-Path $AppPaths $item.Files[0]
        New-Item -Path $key -Force | Out-Null
        Set-ItemProperty -Path $key -Name '(default)' -Value $exe
        Set-ItemProperty -Path $key -Name 'Path' -Value $InstallDir
    }

    # chickenctl on PATH.
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (($userPath -split ';') -notcontains $InstallDir) {
        [Environment]::SetEnvironmentVariable('Path', (($userPath.TrimEnd(';') + ';' + $InstallDir).TrimStart(';')), 'User')
    }

    # The uninstaller and the record of what is installed.
    $uninstaller = Join-Path $InstallDir 'uninstall.ps1'
    Set-Content -Path $uninstaller -Value $UninstallScript -Encoding UTF8
    Save-InstalledInfo @($items | ForEach-Object { $_.Repo }) $commits

    New-Item -Path $UninstallKey -Force | Out-Null
    $rooster = Join-Path $InstallDir 'chickenwm-rooster.exe'
    $props = @{
        DisplayName = 'ChickenWM'; DisplayVersion = $Version; Publisher = 'ChickenWM Contributors'
        InstallLocation = $InstallDir; DisplayIcon = "$rooster,0"; URLInfoAbout = "https://github.com/$Org/chickenwm"
        UninstallString = "powershell.exe -NoProfile -ExecutionPolicy Bypass -File `"$uninstaller`""
        NoModify = 1; NoRepair = 1
    }
    foreach ($k in $props.Keys) { Set-ItemProperty -Path $UninstallKey -Name $k -Value $props[$k] }
    $size = (Get-ChildItem $InstallDir -File | Measure-Object Length -Sum).Sum
    Set-ItemProperty -Path $UninstallKey -Name EstimatedSize -Value ([int]($size / 1KB)) -Type DWord

    Step 'Done'
    Say "Installed: $(($items | ForEach-Object { $_.Title }) -join ', ')" 'Green'
    Say 'Start it from the Start Menu (ChickenWM), or run chickenwm-rooster. Your settings live in %USERPROFILE%\.config\chickenwm.'
    Say 'Emergency restore at any time: Ctrl + Alt + Shift + Esc.'
    if ($chosen['chickenwm-dwm-hook']) {
        Say 'The DWM hook is installed but off. Only turn it on (Settings > Displays) if you do not play games with anti-cheat.' 'DarkYellow'
    }
    if ($wasRunning -or (Ask 'Start ChickenWM now?')) { Start-Process $rooster -WorkingDirectory $InstallDir }
}

# Records what is installed and at which commit (compared by updates).
function Save-InstalledInfo($components, $commits) {
    $map = [ordered]@{}
    foreach ($repo in $components) { if ($commits[$repo]) { $map[$repo] = $commits[$repo] } }
    [ordered]@{ version = $Version; components = @($components); commits = $map } |
        ConvertTo-Json -Depth 3 | Set-Content (Join-Path $InstallDir 'installed.json') -Encoding UTF8
}

# Update: compares every installed program with its repository, then downloads, builds and
# installs the ones that changed, one by one. $checkOnly just reports what has an update.
function Update-ChickenWM([bool]$checkOnly) {
    $script:Assume = $true
    $info = Get-InstalledInfo
    if (-not $info) {
        throw "No installation found in $InstallDir. Install with: irm https://$Org.github.io/chickenwm/install.ps1 | iex"
    }
    Say "ChickenWM updater ($InstallDir)" 'Cyan'
    $installedCommits = @{}
    if ($info.commits) { foreach ($p in $info.commits.PSObject.Properties) { $installedCommits[$p.Name] = $p.Value } }
    $items = @($Catalog | Where-Object { @($info.components) -contains $_.Repo })

    Step 'Checking for updates'
    $latest = @{}
    $pending = @()
    foreach ($item in $items) {
        Report $item.Repo 'checking'
        $sha = Get-LatestCommit $item.Repo
        if (-not $sha) {
            Say ("  {0,-26} couldn't reach GitHub" -f $item.Title) 'Red'
            Report $item.Repo 'failed' "couldn't reach GitHub"
            continue
        }
        $latest[$item.Repo] = $sha
        if ($installedCommits[$item.Repo] -eq $sha) {
            Say ("  {0,-26} up to date" -f $item.Title) 'DarkGray'
            Report $item.Repo 'current'
        } else {
            Say ("  {0,-26} update available ({1})" -f $item.Title, $sha.Substring(0, 7)) 'White'
            Report $item.Repo 'available' $sha.Substring(0, 7)
            $pending += $item
        }
    }
    if ($checkOnly -or -not $pending.Count) {
        if (-not $pending.Count) { Say 'Everything is up to date.' 'Green' }
        Report 'done' $pending.Count
        return
    }

    Install-Requirements
    $srcRoot = Join-Path $env:TEMP 'chickenwm-install'
    New-Item -ItemType Directory -Force $srcRoot, $CacheDir | Out-Null
    $env:CARGO_TARGET_DIR = $CacheDir
    Step "Updating $($pending.Count) program(s), one by one"
    $built = @()
    foreach ($item in $pending) {
        Say "  $($item.Title)" 'White'
        Report $item.Repo 'building'
        try {
            Build-Component $item $srcRoot $latest[$item.Repo]
            Report $item.Repo 'built'
            $built += $item
        } catch {
            Say "    failed: $($_.Exception.Message)" 'Red'
            Report $item.Repo 'failed' $_.Exception.Message
        }
    }
    if (-not $built.Count) { Report 'done' 0; throw 'No update could be built.' }

    Step 'Installing'
    $wasRunning = Stop-ChickenWM
    $release = Join-Path $CacheDir 'release'
    foreach ($item in $built) {
        foreach ($file in $item.Files) { Copy-Item (Join-Path $release $file) $InstallDir -Force }
        $installedCommits[$item.Repo] = $latest[$item.Repo]
        Report $item.Repo 'installed'
    }
    Save-InstalledInfo @($info.components) $installedCommits
    Say "Updated: $(($built | ForEach-Object { $_.Title }) -join ', ')" 'Green'
    Report 'done' $built.Count
    if ($wasRunning) { Start-Process (Join-Path $InstallDir 'chickenwm-rooster.exe') -WorkingDirectory $InstallDir }
}

# Written next to the programs; Settings > Apps > ChickenWM runs it.
$UninstallScript = @'
$ErrorActionPreference = 'SilentlyContinue'
$dir = Split-Path -Parent $MyInvocation.MyCommand.Path
$startMenu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
$appPaths = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\App Paths'
Write-Host 'Uninstalling ChickenWM...' -ForegroundColor Yellow
# Put Explorer back as the shell (if ChickenWM replaced it) before anything goes away.
$rooster = Join-Path $dir 'chickenwm-rooster.exe'
if (Test-Path $rooster) { & $rooster --restore-explorer | Out-Null }
Get-ChildItem $dir -Filter *.exe | ForEach-Object {
    Get-Process -Name $_.BaseName | Stop-Process -Force
    Remove-Item (Join-Path $appPaths $_.Name) -Force
}
Start-Sleep -Milliseconds 800
Remove-Item (Join-Path $startMenu 'ChickenWM') -Recurse -Force
Remove-Item (Join-Path $startMenu 'Hen Browser.lnk') -Force
Remove-Item 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\ChickenWM' -Recurse -Force
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
[Environment]::SetEnvironmentVariable('Path', (($userPath -split ';' | Where-Object { $_ -and $_ -ne $dir }) -join ';'), 'User')
Remove-Item (Join-Path $env:LOCALAPPDATA 'ChickenWM\build') -Recurse -Force
Set-Location $env:TEMP
Remove-Item $dir -Recurse -Force
Write-Host 'ChickenWM is uninstalled. Your settings are kept in %USERPROFILE%\.config\chickenwm; delete that folder to remove them too.' -ForegroundColor Green
'@

try {
    if ($env:CHICKENWM_UPDATE) {
        Update-ChickenWM ($env:CHICKENWM_UPDATE -eq 'check')
    } else {
        Install-ChickenWM
    }
} catch {
    Write-Host ''
    Write-Host "Failed: $($_.Exception.Message)" -ForegroundColor Red
    Report 'error' $_.Exception.Message
}
