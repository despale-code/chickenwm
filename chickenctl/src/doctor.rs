//! `chickenctl doctor`: checks the pieces ChickenWM depends on and says what to do about
//! anything that is off. Read-only by default; pass `--fix` to repair safe issues automatically.

use chickenwm_core::ipc::{send_ipc_command_blocking, IpcCommand, IpcResponse, PIPE_NAME_BAR, PIPE_NAME_DAEMON, PIPE_NAME_WM};
use serde::Serialize;
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Ok,
    Info,
    Warn,
    Fail,
}

#[derive(Debug, Serialize)]
pub struct Check {
    pub name: &'static str,
    pub level: Level,
    pub detail: String,
    /// What to do about it (warn / fail only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
    /// True when `--fix` automatically repaired this issue during this run.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub fixed: bool,
}

fn check(name: &'static str, level: Level, detail: impl Into<String>, fix: Option<&str>) -> Check {
    Check {
        name,
        level,
        detail: detail.into(),
        fix: fix.map(str::to_owned),
        fixed: false,
    }
}

fn check_fixed(name: &'static str, detail: impl Into<String>) -> Check {
    Check {
        name,
        level: Level::Ok,
        detail: detail.into(),
        fix: None,
        fixed: true,
    }
}

/// Modules that should run at most once each.
const MODULES: [&str; 8] = [
    "chickenwm-rooster.exe",
    "chickenwm-wm.exe",
    "chickenwm-binds.exe",
    "chickenwm-paper.exe",
    "roobar.exe",
    "chickenwm-launcher.exe",
    "chickenwm-notify.exe",
    "chickenwm-dwm-hook.exe",
];

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A registry value via `reg.exe` (data as printed, e.g. `0x1` for DWORDs).
fn reg_value(key: &str, name: &str) -> Option<String> {
    use std::os::windows::process::CommandExt;
    let out = Command::new("reg.exe")
        .args(["query", key, "/v", name])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).lines().find_map(|line| {
        let mut parts = line.trim().splitn(3, "    ");
        let (n, _ty, data) = (parts.next()?, parts.next()?, parts.next()?);
        n.trim().eq_ignore_ascii_case(name).then(|| data.trim().to_owned())
    })
}

fn is_dwm_pipe_live() -> bool {
    unsafe {
        let wide: Vec<u16> = r"\\.\pipe\dwm_ipc"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        windows::Win32::System::Pipes::WaitNamedPipeW(
            windows::core::PCWSTR(wide.as_ptr()),
            20,
        )
        .as_bool()
    }
}

fn terminate_extra_pids(pids: &[u32]) -> usize {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
    let mut killed = 0;
    for &pid in pids.iter().skip(1) {
        unsafe {
            if let Ok(proc) = OpenProcess(PROCESS_TERMINATE, false, pid) {
                if TerminateProcess(proc, 0).is_ok() {
                    killed += 1;
                }
                let _ = CloseHandle(proc);
            }
        }
    }
    killed
}

const WINLOGON: &str = r"Software\Microsoft\Windows NT\CurrentVersion\Winlogon";

pub fn run(fix_mode: bool) -> Vec<Check> {
    let mut checks = Vec::new();

    if fix_mode {
        let _ = chickenwm_core::bootstrap_config_and_themes();
    }

    // Config
    match chickenwm_core::config::load_or_init_config() {
        Ok((_, cfg)) => {
            let issues = chickenwm_core::config_check::validate_user_config().unwrap_or_default();
            let errors = issues.iter().filter(|i| i.severity == chickenwm_core::config_check::Severity::Error).count();
            if issues.is_empty() {
                checks.push(check(
                    "config",
                    Level::Ok,
                    format!("{} has no problems", chickenwm_core::config::config_dir().join("chickenwm.conf").display()),
                    None,
                ));
            } else {
                let first = &issues[0];
                checks.push(check(
                    "config",
                    if errors > 0 { Level::Warn } else { Level::Info },
                    format!("{} issue(s), first on line {}: {}", issues.len(), first.line, first.message),
                    Some("run `chickenctl check-config` for the full list"),
                ));
            }
            if cfg.misc.dwm_hook_enabled {
                let mut pipe_live = is_dwm_pipe_live();
                let mut injected_by_fix = false;
                if !pipe_live && fix_mode {
                    if let Ok(mut exe) = std::env::current_exe() {
                        exe.set_file_name("chickenwm-dwm-hook.exe");
                        if exe.exists() {
                            let _ = Command::new(&exe)
                                .arg("--inject")
                                .stdout(std::process::Stdio::null())
                                .stderr(std::process::Stdio::null())
                                .status();
                            std::thread::sleep(std::time::Duration::from_millis(80));
                            pipe_live = is_dwm_pipe_live();
                            injected_by_fix = pipe_live;
                        }
                    }
                }
                if injected_by_fix {
                    checks.push(check_fixed(
                        "dwm hook",
                        "fixed: injected chickenwm_dwm_hook.dll into dwm.exe (\\\\.\\pipe\\dwm_ipc active)",
                    ));
                } else if pipe_live {
                    checks.push(check(
                        "dwm hook",
                        Level::Ok,
                        "injected into dwm.exe (\\\\.\\pipe\\dwm_ipc active)",
                        None,
                    ));
                } else {
                    checks.push(check(
                        "dwm hook",
                        Level::Warn,
                        "dwm_hook_enabled = true, but chickenwm_dwm_hook.dll is not yet injected into dwm.exe",
                        Some("run `chickenwm-dwm-hook --inject` as Administrator or set `dwm_hook_enabled = false`"),
                    ));
                }
            } else {
                checks.push(check("dwm hook", Level::Ok, "disabled", None));
            }
        }
        Err(e) => checks.push(check(
            "config",
            Level::Fail,
            format!("chickenwm.conf doesn't load: {e:#}"),
            Some("fix the line named above, then `chickenctl reload`"),
        )),
    }

    // Shell replacement
    let hkcu_shell = reg_value(&format!(r"HKCU\{WINLOGON}"), "Shell");
    let replaced = chickenwm_core::failsafe::is_winlogon_shell_chickenwm();
    match (&hkcu_shell, replaced) {
        (Some(path), true) => {
            let exists = std::path::Path::new(path.trim_matches('"')).exists();
            if !exists && fix_mode && chickenwm_core::failsafe::restore_winlogon_shell_registry().is_ok() {
                checks.push(check_fixed(
                    "shell",
                    format!("fixed: removed missing Winlogon Shell override ({path}); restored explorer.exe default"),
                ));
            } else {
                checks.push(check(
                    "shell",
                    if exists { Level::Ok } else { Level::Fail },
                    format!("ChickenWM is the shell: {path}{}", if exists { "" } else { " (file missing!)" }),
                    (!exists).then_some("the shell exe is gone: sign-in would leave a black screen. Run `chickenctl doctor --fix` or `chickenctl restore-explorer`"),
                ));
            }
            // With a replacement shell, Winlogon restarts *it* when it exits; killing explorer
            // the old way then starts a second rooster.
            let auto = reg_value(&format!(r"HKLM\{WINLOGON}"), "AutoRestartShell");
            if auto.as_deref().is_some_and(|v| v.trim() != "0x0") {
                checks.push(check(
                    "auto-restart shell",
                    Level::Info,
                    "Winlogon AutoRestartShell is on: if rooster exits it is started again (a second copy if one is still running)",
                    None,
                ));
            }
        }
        (Some(other), false) => checks.push(check("shell", Level::Info, format!("per-user shell is {other} (not ChickenWM)"), None)),
        (None, _) => checks.push(check("shell", Level::Info, "explorer.exe (default; ChickenWM runs as an app)", None)),
    }

    // Admin task
    let task = chickenwm_core::elevation::admin_task_exists();
    checks.push(check(
        "admin task",
        if task { Level::Ok } else { Level::Info },
        if task {
            format!("scheduled task `{}` is installed (elevated start without a UAC prompt)", chickenwm_core::elevation::ADMIN_TASK_NAME)
        } else {
            "not installed: ChickenWM can't manage elevated windows".to_owned()
        },
        None,
    ));

    // Modules
    let mut running = Vec::new();
    for name in MODULES {
        let pids = chickenwm_core::failsafe::find_process_ids(&[name]);
        let n = pids.len();
        if n > 1 {
            if fix_mode {
                let killed = terminate_extra_pids(&pids);
                if killed > 0 {
                    checks.push(check_fixed(
                        "duplicate module",
                        format!("fixed: terminated {killed} duplicate copy/copies of {name} (kept PID {})", pids[0]),
                    ));
                } else {
                    checks.push(check(
                        "duplicate module",
                        Level::Warn,
                        format!("{n} copies of {name} are running (could not terminate extra elevated copies)"),
                        Some("stop the extra copies in Task Manager, or `chickenctl reload`"),
                    ));
                }
            } else {
                checks.push(check(
                    "duplicate module",
                    Level::Warn,
                    format!("{n} copies of {name} are running"),
                    Some("run `chickenctl doctor --fix` to terminate extra copies, or `chickenctl reload`"),
                ));
            }
        }
        if n > 0 {
            running.push(name.trim_end_matches(".exe"));
        }
    }
    checks.push(check(
        "modules",
        if running.iter().any(|m| *m == "chickenwm-wm") { Level::Ok } else { Level::Warn },
        if running.is_empty() { "none running".to_owned() } else { running.join(", ") },
        (!running.iter().any(|m| *m == "chickenwm-wm")).then_some("the window manager isn't running: start ChickenWM (rooster) or chickenwm-wm.exe"),
    ));

    // IPC
    for (name, pipe, cmd) in [
        ("daemon ipc", PIPE_NAME_DAEMON, IpcCommand::QueryStatus),
        ("wm ipc", PIPE_NAME_WM, IpcCommand::QueryBarState),
    ] {
        match send_ipc_command_blocking(pipe, &cmd) {
            Ok(IpcResponse::Error { message }) => checks.push(check(name, Level::Warn, format!("answered with an error: {message}"), None)),
            Ok(_) => checks.push(check(name, Level::Ok, format!("{pipe} answers"), None)),
            Err(e) => checks.push(check(name, Level::Warn, format!("{pipe}: {e:#}"), Some("is that module running? (see `modules`)"))),
        }
    }
    let bar_up = !chickenwm_core::failsafe::find_process_ids(&["roobar.exe"]).is_empty();
    if bar_up && send_ipc_command_blocking(PIPE_NAME_BAR, &IpcCommand::ReloadConfig).is_err() {
        checks.push(check("bar ipc", Level::Warn, "roobar is running but its pipe doesn't answer", Some("restart roobar")));
    }

    // Failsafe hotkey: if we can register Ctrl+Alt+Shift+Esc ourselves, nobody holds it.
    checks.push(failsafe_hotkey());

    // Logs
    checks.push(recent_log_errors());

    checks
}

fn failsafe_hotkey() -> Check {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, VK_ESCAPE,
    };
    let free = unsafe {
        let ok = RegisterHotKey(None, 0x7F98, MOD_CONTROL | MOD_ALT | MOD_SHIFT | MOD_NOREPEAT, VK_ESCAPE.0 as u32).is_ok();
        if ok {
            let _ = UnregisterHotKey(None, 0x7F98);
        }
        ok
    };
    if free {
        check(
            "failsafe hotkey",
            Level::Warn,
            "Ctrl+Alt+Shift+Esc isn't registered by any ChickenWM module (or none is running)",
            Some("if ChickenWM is running, restart it so the emergency restore hotkey is armed; `chickenctl restore-explorer` always works"),
        )
    } else {
        check("failsafe hotkey", Level::Ok, "Ctrl+Alt+Shift+Esc is armed (emergency restore)", None)
    }
}

/// ERROR lines in each module log over the last day.
fn recent_log_errors() -> Check {
    let dir = chickenwm_core::config::config_dir().join("logs");
    let since = std::time::SystemTime::now() - std::time::Duration::from_secs(24 * 3600);
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let fresh = entry.metadata().and_then(|m| m.modified()).is_ok_and(|t| t >= since);
        if !fresh || path.extension().is_none_or(|e| e != "log") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let errors = text.lines().filter(|l| l.contains(" ERROR ")).count();
        if errors > 0 {
            found.push(format!("{} ({errors})", path.file_stem().unwrap_or_default().to_string_lossy()));
        }
    }
    if found.is_empty() {
        check("logs", Level::Ok, format!("no ERROR lines in {}", dir.display()), None)
    } else {
        check(
            "logs",
            Level::Info,
            format!("ERROR lines in: {}", found.join(", ")),
            Some("see ~/.config/chickenwm/logs/"),
        )
    }
}

pub fn print(checks: &[Check]) {
    for c in checks {
        let tag = if c.fixed {
            "fixed"
        } else {
            match c.level {
                Level::Ok => "  ok ",
                Level::Info => " info",
                Level::Warn => " WARN",
                Level::Fail => " FAIL",
            }
        };
        println!("{tag}  {:<18} {}", c.name, c.detail);
        if let Some(fix) = &c.fix {
            println!("       {:<18} → {fix}", "");
        }
    }
    let warn = checks.iter().filter(|c| c.level == Level::Warn).count();
    let fail = checks.iter().filter(|c| c.level == Level::Fail).count();
    let fixed = checks.iter().filter(|c| c.fixed).count();
    println!();
    match (fail, warn, fixed) {
        (0, 0, 0) => println!("All good."),
        (0, 0, n) => println!("All good ({n} issue(s) fixed)."),
        (f, w, 0) => println!("{f} problem(s), {w} warning(s)."),
        (f, w, n) => println!("{f} problem(s), {w} warning(s) ({n} issue(s) fixed)."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_json_serialization_includes_fixed_only_when_true() {
        let c1 = check("config", Level::Ok, "all good", None);
        let j1 = serde_json::to_value(&c1).unwrap();
        assert_eq!(j1["name"], "config");
        assert_eq!(j1["level"], "ok");
        assert!(j1.get("fixed").is_none());
        assert!(j1.get("fix").is_none());

        let c2 = check_fixed("shell", "fixed: restored explorer.exe");
        let j2 = serde_json::to_value(&c2).unwrap();
        assert_eq!(j2["fixed"], true);
        assert_eq!(j2["level"], "ok");
    }
}
