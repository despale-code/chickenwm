//! Administrator ("elevated") support.
//!
//! Windows (UIPI) stops a normal process from moving or resizing windows of an elevated one, so
//! ChickenWM can only tile those when it runs elevated itself. This module provides:
//! - [`is_elevated`] / [`run_elevated`]: check the current token and relaunch a program through
//!   the UAC prompt.
//! - A scheduled task (`ChickenWM`, "highest privileges", on-demand) that lets the shell start
//!   elevated at sign-in *without* a prompt after a one-time approval.

use anyhow::{anyhow, bail, Context, Result};
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, OpenProcessToken, WaitForSingleObject, INFINITE,
};
use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

/// Name of the on-demand scheduled task that starts ChickenWM elevated.
pub const ADMIN_TASK_NAME: &str = "ChickenWM";

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Whether this process runs with an elevated (administrator) token.
pub fn is_elevated() -> bool {
    static ELEVATED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ELEVATED.get_or_init(|| unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut info = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut info as *mut _ as *mut core::ffi::c_void),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && info.TokenIsElevated != 0
    })
}

fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        arg.to_string()
    } else {
        format!("\"{}\"", arg.replace('"', "\\\""))
    }
}

/// Runs `exe` with `args` through the UAC prompt (`runas`). Returns the exit code when `wait`
/// is set (otherwise 0). Fails when the user declines the prompt.
pub fn run_elevated(exe: &Path, args: &[&str], wait: bool) -> Result<u32> {
    let file = wide(&exe.to_string_lossy());
    let params = wide(&args.iter().map(|a| quote_arg(a)).collect::<Vec<_>>().join(" "));
    let verb = wide("runas");
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: if wait { SEE_MASK_NOCLOSEPROCESS } else { 0 },
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe {
        ShellExecuteExW(&mut info).map_err(|e| anyhow!("administrator request was not approved: {e}"))?;
        if !wait || info.hProcess.is_invalid() {
            return Ok(0);
        }
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 0u32;
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        Ok(code)
    }
}

fn schtasks(args: &[&str]) -> Result<std::process::Output> {
    Command::new("schtasks.exe")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .context("could not run schtasks.exe")
}

/// Whether the elevated-start scheduled task is registered.
pub fn admin_task_exists() -> bool {
    schtasks(&["/query", "/tn", ADMIN_TASK_NAME]).map(|o| o.status.success()).unwrap_or(false)
}

/// Starts the scheduled task (an elevated ChickenWM, no prompt). True when Windows accepted it.
pub fn start_admin_task() -> bool {
    schtasks(&["/run", "/tn", ADMIN_TASK_NAME]).map(|o| o.status.success()).unwrap_or(false)
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Registers the elevated-start task for the current user. Needs an elevated caller.
pub fn install_admin_task(exe: &Path) -> Result<()> {
    if !is_elevated() {
        bail!("administrator rights are required to register the task");
    }
    let user = format!(
        "{}\\{}",
        std::env::var("USERDOMAIN").unwrap_or_default(),
        std::env::var("USERNAME").unwrap_or_default()
    );
    // No trigger: the task only ever runs on demand, so a shell-started (unelevated) rooster can
    // hand over to it without ever creating a second instance at sign-in.
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>Starts ChickenWM with administrator rights, without a UAC prompt.</Description></RegistrationInfo>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>4</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec><Command>{exe}</Command></Exec>
  </Actions>
</Task>
"#,
        user = xml_escape(&user),
        exe = xml_escape(&exe.to_string_lossy()),
    );
    let mut bytes = vec![0xFF, 0xFE];
    for unit in xml.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    let path = std::env::temp_dir().join("chickenwm-admin-task.xml");
    std::fs::write(&path, bytes)?;
    let out = schtasks(&["/create", "/tn", ADMIN_TASK_NAME, "/xml", &path.to_string_lossy(), "/f"]);
    let _ = std::fs::remove_file(&path);
    let out = out?;
    if !out.status.success() {
        bail!("schtasks failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

/// Removes the elevated-start task (a missing task is fine). Needs an elevated caller.
pub fn remove_admin_task() -> Result<()> {
    if !admin_task_exists() {
        return Ok(());
    }
    if !is_elevated() {
        bail!("administrator rights are required to remove the task");
    }
    let out = schtasks(&["/delete", "/tn", ADMIN_TASK_NAME, "/f"])?;
    if !out.status.success() {
        bail!("schtasks failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}
