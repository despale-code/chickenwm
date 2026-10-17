//! Phase 1.3 & Phase 6.1: Emergency Failsafe & Recovery Mechanism (`Ctrl + Alt + Shift + Escape`)
//! and Windows Shell (`Winlogon\Shell` & `Directory\shell`) registry management.

use anyhow::Result;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;
use tracing::{error, info, warn};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcessId, OpenProcess, ReleaseMutex, TerminateProcess,
    PROCESS_TERMINATE,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW,
    RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, KEY_WRITE,
    REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
    MOD_SHIFT, VK_CONTROL, VK_ESCAPE, VK_MENU, VK_SHIFT,
};
use windows::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};
use windows::Win32::UI::WindowsAndMessaging::{PeekMessageW, MSG, PM_REMOVE, WM_HOTKEY};

/// Global atomic flag controlling whether `chickenwm-rooster` is allowed to suppress `explorer.exe`.
/// Starts `false` until the daemon reads `misc.suppress_explorer` (default `true`) from config,
/// and immediately flips to `false` when the emergency failsafe triggers.
pub static EXPLORER_KILLER_ENABLED: AtomicBool = AtomicBool::new(false);

/// Global atomic flag tracking whether experimental DWM DLL injection is currently active.
pub static DWM_HOOK_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Global atomic flag signaled when the emergency failsafe has been triggered.
pub static FAILSAFE_TRIGGERED: AtomicBool = AtomicBool::new(false);

const WINLOGON_SUBKEY: &str = r"Software\Microsoft\Windows NT\CurrentVersion\Winlogon";
const FOLDER_SHELL_SUBKEY: &str = r"Software\Classes\Folder\shell";

fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Signals and ejects any experimental DWM hooks (`chickenwm_dwm_hook.dll`) from `dwm.exe`.
pub fn eject_dwm_hooks() {
    DWM_HOOK_ACTIVE.store(false, Ordering::SeqCst);
    let signal_path = std::env::temp_dir().join("chickenwm_dwm_unhook.signal");
    let _ = std::fs::write(&signal_path, b"EJECT");
    if let Ok(mut exe) = std::env::current_exe() {
        exe.set_file_name("chickenwm-dwm-hook.exe");
        if exe.exists() {
            let _ = Command::new(exe)
                .arg("--eject")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
}

/// Immediately restores `explorer.exe` as the Windows shell, disables the process killer,
/// ejects any DWM hooks, launches `C:\Windows\explorer.exe`, and optionally terminates all `chickenwm` processes.
pub fn trigger_emergency_restore(terminate_self: bool) {
    // Prevent re-entrant triggers
    if FAILSAFE_TRIGGERED.swap(true, Ordering::SeqCst) && terminate_self {
        return;
    }

    // Several ChickenWM processes react to the same failsafe chord at once; the first one to
    // create the mutex does the restore, the rest just exit. Held until this process exits.
    let (restore_mutex, owns_restore) = claim_restore_mutex();
    if !owns_restore && terminate_self {
        std::process::exit(0);
    }

    // 1. Immediately disable the explorer.exe suppressor & eject any DWM hooks
    EXPLORER_KILLER_ENABLED.store(false, Ordering::SeqCst);
    eject_dwm_hooks();
    warn!("EMERGENCY FAILSAFE TRIGGERED: Restoring explorer.exe shell, ejecting DWM hooks, and disabling process suppressor!");

    // 2. Restore HKCU\Software\Microsoft\Windows NT\CurrentVersion\Winlogon\Shell -> explorer.exe
    if let Err(e) = restore_winlogon_shell_registry() {
        error!("Failed to restore Winlogon\\Shell via Win32 Reg API: {e}; attempting reg.exe fallback");
        let _ = Command::new("reg.exe")
            .args([
                "delete",
                r"HKCU\Software\Microsoft\Windows NT\CurrentVersion\Winlogon",
                "/v",
                "Shell",
                "/f",
            ])
            .status();
    }

    // 3. Restore Folder / Directory shell open association to default
    let _ = register_files_shell_handler(false, None);

    // 4. Terminate peer chickenwm processes first, so the daemon's explorer suppressor and
    //    watchdog can't kill the new shell or respawn modules behind our back.
    if terminate_self {
        terminate_processes(&CHICKENWM_PROCESSES);
    }

    // 5. Unhide and restore all windows that may have been hidden on other workspaces
    let restored_count = restore_all_desktop_windows();
    info!("Restored {restored_count} hidden desktop windows.");

    // 6. Launch C:\Windows\explorer.exe exactly once per session: every ChickenWM process watches
    //    the failsafe chord, so only the mutex owner launches, and only if no shell is running yet.
    if owns_restore && find_process_ids(&["explorer.exe"]).is_empty() {
        info!("Spawning C:\\Windows\\explorer.exe...");
        let _ = Command::new(r"C:\Windows\explorer.exe").spawn();
    } else {
        info!("explorer.exe already restored by another process; not relaunching.");
    }

    if terminate_self {
        std::process::exit(0);
    }
    if let Some(mutex) = restore_mutex {
        unsafe {
            let _ = ReleaseMutex(mutex);
            let _ = CloseHandle(mutex);
        }
    }
}

/// Window property `chickenwm-wm` sets on every window it hides for an inactive workspace.
/// Properties live on the window itself, so any process can find them — even after the WM was
/// killed by the failsafe before it could restore its windows.
const HIDDEN_BY_CHICKENWM_PROP: &str = "ChickenWMHiddenByWorkspace";

/// Tags `hwnd` as hidden by HyperWin, so [`restore_all_desktop_windows`] knows to bring it back.
pub fn mark_hidden_by_chickenwm(hwnd: u64) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::SetPropW;
    let prop = to_wide_null(HIDDEN_BY_CHICKENWM_PROP);
    unsafe {
        let _ = SetPropW(
            HWND(hwnd as *mut core::ffi::c_void),
            PCWSTR(prop.as_ptr()),
            HANDLE(1 as *mut core::ffi::c_void),
        );
    }
}

/// Removes the tag set by [`mark_hidden_by_chickenwm`] once HyperWin shows the window again.
pub fn clear_hidden_by_chickenwm(hwnd: u64) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::RemovePropW;
    let prop = to_wide_null(HIDDEN_BY_CHICKENWM_PROP);
    unsafe {
        let _ = RemovePropW(HWND(hwnd as *mut core::ffi::c_void), PCWSTR(prop.as_ptr()));
    }
}

/// Where windows of inactive workspaces are parked (far left of any monitor layout; not
/// -32000, which Windows uses for minimized windows).
pub const PARK_X: i32 = -30000;
const PARK_POS_PROP: &str = "ChickenWMParkedPos";

/// Parks `hwnd` off-screen for an inactive workspace instead of hiding it. The window stays
/// "visible" to Windows (Task Manager keeps it under Apps with its child processes; the app
/// isn't told it was hidden). Its on-screen position is saved on the window, so
/// [`restore_all_desktop_windows`] can bring it back even if the WM was killed.
pub fn park_window(hwnd: u64) {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, SetPropW, SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER};
    let h = HWND(hwnd as *mut core::ffi::c_void);
    let prop = to_wide_null(PARK_POS_PROP);
    unsafe {
        let mut rc = RECT::default();
        if GetWindowRect(h, &mut rc).is_err() {
            return;
        }
        if rc.left > PARK_X + 5000 {
            // Not parked yet: remember where it was (x and y packed into the handle value).
            let packed = ((rc.left as i64 + 0x8000_0000) << 32) | (rc.top as i64 + 0x8000_0000);
            let _ = SetPropW(h, PCWSTR(prop.as_ptr()), HANDLE(packed as usize as *mut core::ffi::c_void));
        }
        let _ = SetWindowPos(h, None, PARK_X, rc.top, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
    mark_hidden_by_chickenwm(hwnd);
}

/// The position saved by [`park_window`] (and forgets it).
fn take_parked_position(hwnd: windows::Win32::Foundation::HWND) -> Option<(i32, i32)> {
    use windows::Win32::UI::WindowsAndMessaging::{GetPropW, RemovePropW};
    let prop = to_wide_null(PARK_POS_PROP);
    unsafe {
        let v = GetPropW(hwnd, PCWSTR(prop.as_ptr())).0 as usize as i64;
        if v == 0 {
            return None;
        }
        let _ = RemovePropW(hwnd, PCWSTR(prop.as_ptr()));
        Some((((v >> 32) - 0x8000_0000) as i32, ((v & 0xFFFF_FFFF) - 0x8000_0000) as i32))
    }
}

/// Clears the parking marks when the WM brings a window back itself (it lays it out anew).
pub fn unpark_window(hwnd: u64) {
    let _ = take_parked_position(windows::Win32::Foundation::HWND(hwnd as *mut core::ffi::c_void));
    clear_hidden_by_chickenwm(hwnd);
}

/// Unhides and restores the windows HyperWin hid on inactive workspaces, e.g. after chickenwm-wm
/// or chickenwm-rooster was shut down or killed. Only windows tagged by [`mark_hidden_by_chickenwm`]
/// are touched — tray-hidden apps and hidden system helper windows stay hidden.
pub fn restore_all_desktop_windows() -> usize {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::System::StationsAndDesktops::{
        CloseDesktop, EnumDesktopWindows, OpenInputDesktop, DESKTOP_ACCESS_FLAGS,
        DESKTOP_CONTROL_FLAGS,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetPropW, IsIconic, IsWindow, RemovePropW, ShowWindow, SW_RESTORE,
        SW_SHOWNA,
    };

    struct Context {
        restored: usize,
        prop: Vec<u16>,
    }

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let ctx = unsafe { &mut *(lparam.0 as *mut Context) };
        unsafe {
            if !IsWindow(hwnd).as_bool() {
                return BOOL(1);
            }
            let prop = PCWSTR(ctx.prop.as_ptr());
            if GetPropW(hwnd, prop).0.is_null() {
                return BOOL(1);
            }
            let _ = RemovePropW(hwnd, prop);
            if let Some((x, y)) = take_parked_position(hwnd) {
                use windows::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER};
                let _ = SetWindowPos(hwnd, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            }
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            } else {
                let _ = ShowWindow(hwnd, SW_SHOWNA);
            }
            ctx.restored += 1;
        }
        BOOL(1)
    }

    let mut ctx = Context {
        restored: 0,
        prop: to_wide_null(HIDDEN_BY_CHICKENWM_PROP),
    };
    unsafe {
        // 0x0100 = DESKTOP_ENUMERATE, required to walk the input desktop's top-level windows.
        match OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_ACCESS_FLAGS(0x0100)) {
            Ok(hdesk) => {
                let _ = EnumDesktopWindows(
                    hdesk,
                    Some(enum_proc),
                    LPARAM(&mut ctx as *mut Context as isize),
                );
                let _ = CloseDesktop(hdesk);
            }
            Err(_) => {
                let _ = EnumWindows(
                    Some(enum_proc),
                    LPARAM(&mut ctx as *mut Context as isize),
                );
            }
        }
    }
    ctx.restored
}

/// Every ChickenWM executable; the daemon goes first so its watchdog stops respawning the rest.
const CHICKENWM_PROCESSES: [&str; 9] = [
    "chickenwm-rooster.exe",
    "chickenwm-wm.exe",
    "chickenwm-binds.exe",
    "chickenwm-paper.exe",
    "roobar.exe",
    "chickenwm-launcher.exe",
    "chickenwm-files.exe",
    "chickenwm-settings.exe",
    "chickenwm-dwm-hook.exe",
];

/// Session-wide mutex held by whichever process performs the emergency restore.
const RESTORE_MUTEX_NAME: &str = r"Local\ChickenWMEmergencyRestore";

/// Creates the restore mutex. Returns `(handle, owns)`: `owns` is false when another process
/// already created it (i.e. is already restoring explorer).
fn claim_restore_mutex() -> (Option<HANDLE>, bool) {
    unsafe {
        let name = to_wide_null(RESTORE_MUTEX_NAME);
        match CreateMutexW(None, true, PCWSTR(name.as_ptr())) {
            Ok(handle) => {
                let owns = GetLastError() != ERROR_ALREADY_EXISTS;
                (Some(handle), owns)
            }
            // Can't coordinate: fall back to the explorer-running check alone.
            Err(_) => (None, true),
        }
    }
}

/// PIDs of running processes whose image name matches one of `names` (case-insensitive),
/// excluding the current process.
pub fn find_process_ids(names: &[&str]) -> Vec<u32> {
    let mut pids = Vec::new();
    unsafe {
        let own_pid = GetCurrentProcessId();
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return pids;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut entry).is_ok() {
            loop {
                let len = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                let exe = String::from_utf16_lossy(&entry.szExeFile[..len]);
                if entry.th32ProcessID != own_pid
                    && names.iter().any(|n| n.eq_ignore_ascii_case(&exe))
                {
                    pids.push(entry.th32ProcessID);
                }
                if Process32NextW(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    pids
}

/// Stops every supervised ChickenWM module but leaves the daemon alone (and explorer
/// untouched), for handing the shell over to a freshly started daemon.
pub fn terminate_modules() {
    terminate_processes(&[
        "chickenwm-wm.exe",
        "chickenwm-binds.exe",
        "chickenwm-paper.exe",
        "roobar.exe",
        "chickenwm-launcher.exe",
        "chickenwm-notify.exe",
    ]);
}

/// Force-terminates every other process matching `names` (never the current process).
fn terminate_processes(names: &[&str]) {
    for pid in find_process_ids(names) {
        unsafe {
            if let Ok(proc) = OpenProcess(PROCESS_TERMINATE, false, pid) {
                let _ = TerminateProcess(proc, 0);
                let _ = CloseHandle(proc);
            }
        }
    }
}

/// Restores `HKCU\Software\Microsoft\Windows NT\CurrentVersion\Winlogon\Shell` by deleting the
/// per-user `Shell` override (causing Windows to fall back to `HKLM`'s `explorer.exe`) or setting `explorer.exe`.
pub fn restore_winlogon_shell_registry() -> Result<()> {
    unsafe {
        let subkey = to_wide_null(WINLOGON_SUBKEY);
        let value_name = to_wide_null("Shell");
        let mut hkey = HKEY::default();
        let status = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            0,
            KEY_SET_VALUE,
            &mut hkey,
        );
        if status.is_ok() {
            let _ = RegDeleteValueW(hkey, PCWSTR(value_name.as_ptr()));
            let _ = RegCloseKey(hkey);
        }
    }
    Ok(())
}

/// Sets or removes `chickenwm-rooster.exe` as the current user's Windows shell in
/// `HKCU\Software\Microsoft\Windows NT\CurrentVersion\Winlogon\Shell`.
pub fn set_winlogon_shell(enable_chickenwm: bool, daemon_exe_path: Option<&str>) -> Result<()> {
    if !enable_chickenwm {
        return restore_winlogon_shell_registry();
    }

    let exe_path = match daemon_exe_path {
        Some(p) => p.to_string(),
        None => std::env::current_exe()?
            .to_string_lossy()
            .into_owned(),
    };

    unsafe {
        let subkey = to_wide_null(WINLOGON_SUBKEY);
        let value_name = to_wide_null("Shell");
        let data_wide = to_wide_null(&exe_path);
        let mut hkey = HKEY::default();

        let status = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            0,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut hkey,
            None,
        );
        if status.is_err() {
            anyhow::bail!("RegCreateKeyExW failed for Winlogon: {:?}", status);
        }

        let bytes = std::slice::from_raw_parts(
            data_wide.as_ptr() as *const u8,
            data_wide.len() * std::mem::size_of::<u16>(),
        );
        let set_status = RegSetValueExW(
            hkey,
            PCWSTR(value_name.as_ptr()),
            0,
            REG_SZ,
            Some(bytes),
        );
        let _ = RegCloseKey(hkey);
        if set_status.is_err() {
            anyhow::bail!("RegSetValueExW failed for Winlogon\\Shell: {:?}", set_status);
        }
    }
    Ok(())
}

/// Checks whether `HKCU\Software\Microsoft\Windows NT\CurrentVersion\Winlogon\Shell` is currently
/// set to a ChickenWM binary.
pub fn is_winlogon_shell_chickenwm() -> bool {
    unsafe {
        let subkey = to_wide_null(WINLOGON_SUBKEY);
        let value_name = to_wide_null("Shell");
        let mut hkey = HKEY::default();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            0,
            KEY_QUERY_VALUE,
            &mut hkey,
        )
        .is_err()
        {
            return false;
        }

        let mut buf = [0u16; 512];
        let mut byte_len = (buf.len() * 2) as u32;
        let res = RegQueryValueExW(
            hkey,
            PCWSTR(value_name.as_ptr()),
            None,
            None,
            Some(buf.as_mut_ptr() as *mut u8),
            Some(&mut byte_len),
        );
        let _ = RegCloseKey(hkey);
        if res.is_err() {
            return false;
        }
        let char_len = (byte_len as usize / 2).saturating_sub(1);
        let val = String::from_utf16_lossy(&buf[..char_len]).to_ascii_lowercase();
        val.contains("chickenwm")
    }
}

/// Shell classes whose default verb is pointed at `chickenwm-files`. `Folder` is deliberately not
/// included: it also covers virtual folders (Control Panel, Recycle Bin, Libraries) that only
/// explorer.exe can render, so hijacking it breaks those.
const FILES_HANDLER_CLASSES: [&str; 2] = [r"Software\Classes\Directory", r"Software\Classes\Drive"];

/// "File Explorer" CLSID. Its `opennewwindow` verb is what `Win+E`, the taskbar/Start "File
/// Explorer" entries and `explorer.exe` with no arguments invoke.
const EXPLORER_NEW_WINDOW_SUBKEY: &str =
    r"Software\Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}\shell\opennewwindow";

const FILES_VERB: &str = "chickenwm_files";

fn reg_set_string(subkey: &str, name: &str, value: &str) -> bool {
    unsafe {
        let subkey_w = to_wide_null(subkey);
        let name_w = to_wide_null(name);
        let value_w = to_wide_null(value);
        let mut hkey = HKEY::default();
        if RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey_w.as_ptr()),
            0,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut hkey,
            None,
        )
        .is_err()
        {
            return false;
        }
        let bytes = std::slice::from_raw_parts(value_w.as_ptr() as *const u8, value_w.len() * 2);
        let ok = RegSetValueExW(hkey, PCWSTR(name_w.as_ptr()), 0, REG_SZ, Some(bytes)).is_ok();
        let _ = RegCloseKey(hkey);
        ok
    }
}

fn reg_query_string(subkey: &str, name: &str) -> Option<String> {
    unsafe {
        let subkey_w = to_wide_null(subkey);
        let name_w = to_wide_null(name);
        let mut hkey = HKEY::default();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey_w.as_ptr()),
            0,
            KEY_QUERY_VALUE,
            &mut hkey,
        )
        .is_err()
        {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut byte_len = (buf.len() * 2) as u32;
        let res = RegQueryValueExW(
            hkey,
            PCWSTR(name_w.as_ptr()),
            None,
            None,
            Some(buf.as_mut_ptr() as *mut u8),
            Some(&mut byte_len),
        );
        let _ = RegCloseKey(hkey);
        if res.is_err() {
            return None;
        }
        let char_len = (byte_len as usize / 2).min(buf.len());
        let s = String::from_utf16_lossy(&buf[..char_len]);
        Some(s.trim_end_matches('\0').to_string())
    }
}

fn reg_delete_value(subkey: &str, name: &str) {
    unsafe {
        let subkey_w = to_wide_null(subkey);
        let name_w = to_wide_null(name);
        let mut hkey = HKEY::default();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey_w.as_ptr()),
            0,
            KEY_SET_VALUE,
            &mut hkey,
        )
        .is_ok()
        {
            let _ = RegDeleteValueW(hkey, PCWSTR(name_w.as_ptr()));
            let _ = RegCloseKey(hkey);
        }
    }
}

fn reg_delete_tree(subkey: &str) {
    unsafe {
        let subkey_w = to_wide_null(subkey);
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(subkey_w.as_ptr()));
    }
}

/// Tells the shell that file associations changed so the new/removed handler takes effect
/// without a logoff.
fn notify_assoc_changed() {
    unsafe {
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
    }
}

fn default_files_exe() -> Result<String> {
    let mut cur = std::env::current_exe()?;
    cur.set_file_name("chickenwm-files.exe");
    Ok(cur.to_string_lossy().into_owned())
}

/// Phase 6.1: Make (or stop making) `chickenwm-files.exe` the system-wide default file manager for
/// the current user (HKCU only, no admin needed):
/// - `Directory` and `Drive` default verb → double-clicking folders/drives, "Open folder" in apps
/// - `File Explorer` CLSID `opennewwindow` → `Win+E`, Start/taskbar "File Explorer"
///
/// Disabling removes every key this writes (plus the legacy `Folder\shell` default from older
/// builds), so explorer.exe's own handlers apply again.
pub fn register_files_shell_handler(enable: bool, files_exe_path: Option<&str>) -> Result<()> {
    if !enable {

        for class in FILES_HANDLER_CLASSES {
            let shell = format!(r"{class}\shell");
            if reg_query_string(&shell, "").as_deref() == Some(FILES_VERB) {
                reg_delete_value(&shell, "");
            }
            reg_delete_tree(&format!(r"{shell}\{FILES_VERB}"));
        }
        if reg_query_string(FOLDER_SHELL_SUBKEY, "").as_deref() == Some(FILES_VERB) {
            reg_delete_value(FOLDER_SHELL_SUBKEY, "");
        }
        reg_delete_tree(EXPLORER_NEW_WINDOW_SUBKEY);
        notify_assoc_changed();
        return Ok(());
    }

    let files_exe = match files_exe_path {
        Some(p) => p.to_string(),
        None => default_files_exe()?,
    };
    // Pointing every folder open at a missing binary would leave the user unable to open folders.
    if !std::path::Path::new(&files_exe).is_file() {
        anyhow::bail!("chickenwm-files.exe not found at {files_exe}");
    }

    let open_cmd = format!("\"{files_exe}\" \"%1\"");
    let mut ok = true;
    for class in FILES_HANDLER_CLASSES {
        let verb_key = format!(r"{class}\shell\{FILES_VERB}");
        ok &= reg_set_string(&verb_key, "", "Open in ChickenWM Files");
        ok &= reg_set_string(&format!(r"{verb_key}\command"), "", &open_cmd);
        ok &= reg_set_string(&format!(r"{class}\shell"), "", FILES_VERB);
    }

    // An empty DelegateExecute overrides explorer's COM handler so `command` is used instead.
    let new_window_cmd = format!(r"{EXPLORER_NEW_WINDOW_SUBKEY}\command");
    ok &= reg_set_string(&new_window_cmd, "", &format!("\"{files_exe}\""));
    ok &= reg_set_string(&new_window_cmd, "DelegateExecute", "");

    notify_assoc_changed();
    if !ok {
        anyhow::bail!("failed to write one or more HKCU\\Software\\Classes keys");
    }
    Ok(())
}

/// Whether `chickenwm-files` is currently the default file manager (all handler keys in place).
pub fn is_files_handler_registered() -> bool {
    FILES_HANDLER_CLASSES.iter().all(|class| {
        reg_query_string(&format!(r"{class}\shell"), "").as_deref() == Some(FILES_VERB)
            && reg_query_string(&format!(r"{class}\shell\{FILES_VERB}\command"), "")
                .is_some_and(|c| c.to_ascii_lowercase().contains("chickenwm-files"))
    }) && reg_query_string(&format!(r"{EXPLORER_NEW_WINDOW_SUBKEY}\command"), "")
        .is_some_and(|c| c.to_ascii_lowercase().contains("chickenwm-files"))
}

/// Phase 1.3: Spawns a dedicated, zero-dependency emergency failsafe thread listening for
/// `Ctrl + Alt + Shift + Escape`.
/// Uses both Win32 `RegisterHotKey` AND raw `GetAsyncKeyState` polling so that even if another
/// application intercepts the hotkey or the message queue stalls, holding `Ctrl+Alt+Shift+Esc`
/// immediately restores `explorer.exe`.
pub fn spawn_emergency_failsafe_thread() -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("chickenwm-emergency-failsafe".to_string())
        .spawn(move || unsafe {
            const HOTKEY_ID: i32 = 0x7F99;
            let registered = RegisterHotKey(
                None,
                HOTKEY_ID,
                MOD_CONTROL | MOD_ALT | MOD_SHIFT | MOD_NOREPEAT,
                VK_ESCAPE.0 as u32,
            )
            .is_ok();

            if registered {
                info!("Emergency failsafe hotkey (Ctrl + Alt + Shift + Escape) registered.");
            } else {
                warn!("RegisterHotKey for Ctrl+Alt+Shift+Esc contended; using direct key-state polling fallback.");
            }

            let mut msg = MSG::default();
            loop {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    if msg.message == WM_HOTKEY && msg.wParam.0 as i32 == HOTKEY_ID {
                        let _ = UnregisterHotKey(None, HOTKEY_ID);
                        trigger_emergency_restore(true);
                        return;
                    }
                }

                // Direct hardware key state check (Ctrl + Alt + Shift + Escape simultaneously held)
                let ctrl = (GetAsyncKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000) != 0;
                let alt = (GetAsyncKeyState(VK_MENU.0 as i32) as u16 & 0x8000) != 0;
                let shift = (GetAsyncKeyState(VK_SHIFT.0 as i32) as u16 & 0x8000) != 0;
                let esc = (GetAsyncKeyState(VK_ESCAPE.0 as i32) as u16 & 0x8000) != 0;

                if ctrl && alt && shift && esc {
                    if registered {
                        let _ = UnregisterHotKey(None, HOTKEY_ID);
                    }
                    trigger_emergency_restore(true);
                    return;
                }

                thread::sleep(Duration::from_millis(25));
            }
        })
        .expect("Failed to spawn emergency failsafe thread")
}
