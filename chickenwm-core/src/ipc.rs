//! Phase 1.2: IPC Infrastructure (`chickenwm-core`)
//! Asynchronous Windows Named Pipe server (`\\.\pipe\chickenwm-ipc`) and client,
//! plus protocol definitions for commands, actions, window clients, and monitor state.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};

pub const PIPE_NAME_DAEMON: &str = r"\\.\pipe\chickenwm-ipc";
pub const PIPE_NAME_WM: &str = r"\\.\pipe\chickenwm-wm-ipc";
pub const PIPE_NAME_LAUNCHER: &str = r"\\.\pipe\chickenwm-launcher-ipc";
pub const PIPE_NAME_PAPER: &str = r"\\.\pipe\chickenwm-paper-ipc";
pub const PIPE_NAME_BAR: &str = r"\\.\pipe\chickenwm-bar-ipc";
pub const PIPE_NAME_NOTIFY: &str = r"\\.\pipe\chickenwm-notify-ipc";

/// Direction for focus / swap / move operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    pub fn from_hypr_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "l" | "left" => Some(Self::Left),
            "r" | "right" => Some(Self::Right),
            "u" | "up" | "t" | "top" => Some(Self::Up),
            "d" | "down" | "b" | "bottom" => Some(Self::Down),
            _ => None,
        }
    }
}

/// Hyprland-compatible dispatcher actions (`bind = $mainMod, Q, exec, ...`, `killactive`, etc.).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Action {
    /// Execute a shell/process command (`exec, <cmd>`).
    Exec(String),
    /// Close the currently focused window (`killactive`).
    KillActive,
    /// Toggle floating state on active window (`togglefloating`).
    ToggleFloating,
    /// Toggle fullscreen state (`fullscreen, 0|1`).
    Fullscreen(u8),
    /// Toggle pseudo-tiling state (`pseudo`).
    Pseudo,
    /// Toggle split orientation in Dwindle layout (`togglesplit`).
    ToggleSplit,
    /// Move focus in a direction (`movefocus, l/r/u/d`).
    MoveFocus(Direction),
    /// Move active window in a direction (`movewindow, l/r/u/d`).
    MoveWindow(Direction),
    /// Switch active workspace (`workspace, 1..10`).
    Workspace(u32),
    /// Switch workspace relatively (`workspace, +1`, `workspace, -1`, `workspace, e+1`, `workspace, e-1`).
    WorkspaceRelative(i32),
    /// Switch to the previously active workspace (`workspace, previous`).
    WorkspacePrevious,
    /// Move active window to workspace (`movetoworkspace, 1..10`).
    MoveToWorkspace(u32),
    /// Move active window to workspace silently (`movetoworkspacesilent, 1..10`).
    MoveToWorkspaceSilent(u32),
    /// Resize active window by delta pixels (`resizeactive, dx dy`).
    ResizeActive { dx: i32, dy: i32 },
    /// Move active floating window by delta pixels (`moveactive, dx dy`).
    MoveActive { dx: i32, dy: i32 },
    /// Start of a Super + LMB/RMB mouse drag (`movewindow` / `resizewindow`).
    MouseDragStart {
        is_resize: bool,
        x: i32,
        y: i32,
    },
    /// Mouse drag move/resize (`movewindow` / `resizewindow` with cursor coords).
    MouseDragWindow {
        is_resize: bool,
        start_x: i32,
        start_y: i32,
        curr_x: i32,
        curr_y: i32,
    },
    /// End of a Super + LMB/RMB mouse drag (`movewindow` / `resizewindow`).
    MouseDragEnd {
        is_resize: bool,
        x: i32,
        y: i32,
    },
    /// Focus a specific window by HWND (`focuswindow, <hwnd>`).
    FocusWindow(u64),
    /// Switch layout engine (`layout, dwindle|master`).
    SetLayout(String),
    /// Summon or toggle the application launcher.
    SummonLauncher,
    /// Open ChickenWM File Explorer at optional path.
    OpenFiles(Option<String>),
    /// Open ChickenWM Settings GUI.
    OpenSettings,
    /// Show / hide the notification center (`chickenwm-notify`).
    ToggleNotificationCenter,
    /// Exit ChickenWM and restore explorer if desired.
    Exit,
}

impl Action {
    /// Parse a Hyprland dispatcher name and argument string into an `Action`.
    pub fn parse_dispatcher(dispatcher: &str, arg: &str) -> Option<Self> {
        let disp = dispatcher.trim().to_ascii_lowercase();
        let arg = arg.trim();
        match disp.as_str() {
            "exec" | "execr" => Some(Action::Exec(arg.to_string())),
            "killactive" | "closewindow" => Some(Action::KillActive),
            "togglefloating" | "setfloating" => Some(Action::ToggleFloating),
            "fullscreen" => {
                let mode = arg.parse::<u8>().unwrap_or(0);
                Some(Action::Fullscreen(mode))
            }
            "pseudo" => Some(Action::Pseudo),
            "togglesplit" | "swapsplit" => Some(Action::ToggleSplit),
            "movefocus" => Direction::from_hypr_str(arg).map(Action::MoveFocus),
            "movewindow" => {
                if let Some(dir) = Direction::from_hypr_str(arg) {
                    Some(Action::MoveWindow(dir))
                } else {
                    Some(Action::MouseDragWindow {
                        is_resize: false,
                        start_x: 0,
                        start_y: 0,
                        curr_x: 0,
                        curr_y: 0,
                    })
                }
            }
            "resizewindow" => Some(Action::MouseDragWindow {
                is_resize: true,
                start_x: 0,
                start_y: 0,
                curr_x: 0,
                curr_y: 0,
            }),
            "workspace" => {
                let lower = arg.to_ascii_lowercase();
                if lower == "previous" || lower == "prev" {
                    Some(Action::WorkspacePrevious)
                } else if let Some(rel) = lower
                    .strip_prefix("e+")
                    .or_else(|| lower.strip_prefix("r+"))
                    .or_else(|| lower.strip_prefix("m+"))
                    .or_else(|| lower.strip_prefix('+'))
                {
                    rel.parse::<i32>().ok().map(Action::WorkspaceRelative)
                } else if let Some(rel) = lower
                    .strip_prefix("e-")
                    .or_else(|| lower.strip_prefix("r-"))
                    .or_else(|| lower.strip_prefix("m-"))
                    .or_else(|| lower.strip_prefix('-'))
                {
                    rel.parse::<i32>().ok().map(|n| Action::WorkspaceRelative(-n))
                } else {
                    arg.parse::<u32>().ok().map(Action::Workspace)
                }
            }
            "movetoworkspace" => arg.parse::<u32>().ok().map(Action::MoveToWorkspace),
            "movetoworkspacesilent" => arg.parse::<u32>().ok().map(Action::MoveToWorkspaceSilent),
            "resizeactive" => {
                let mut parts = arg.split_whitespace();
                let dx = parts.next()?.parse::<i32>().ok()?;
                let dy = parts.next()?.parse::<i32>().ok()?;
                Some(Action::ResizeActive { dx, dy })
            }
            "moveactive" => {
                let mut parts = arg.split_whitespace();
                let dx = parts.next()?.parse::<i32>().ok()?;
                let dy = parts.next()?.parse::<i32>().ok()?;
                Some(Action::MoveActive { dx, dy })
            }
            "focuswindow" => arg.parse::<u64>().ok().map(Action::FocusWindow),
            "layout" => Some(Action::SetLayout(arg.to_string())),
            "launcher" | "summonlauncher" => Some(Action::SummonLauncher),
            "files" | "openfiles" => Some(Action::OpenFiles(if arg.is_empty() {
                None
            } else {
                Some(arg.to_string())
            })),
            "settings" | "opensettings" => Some(Action::OpenSettings),
            "notifications" | "notificationcenter" => Some(Action::ToggleNotificationCenter),
            "exit" => Some(Action::Exit),
            _ => None,
        }
    }
}

/// Summary of a single workspace (`1..10`) for `roobar` synchronization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSummary {
    pub id: u32,
    pub active: bool,
    pub occupied: bool,
    pub client_count: usize,
    pub focused_title: Option<String>,
}

/// Real-time snapshot pushed from `chickenwm-wm` to `roobar` (`\\.\pipe\chickenwm-bar-ipc`)
/// on every workspace switch, window focus/open/close, or layout toggle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BarStateSnapshot {
    pub active_workspace: u32,
    pub workspaces: Vec<WorkspaceSummary>,
    pub active_window_title: String,
    pub active_window_class: String,
    pub active_window_process: String,
    pub active_layout: String,
    pub monitor_name: String,
    pub monitor_refresh_hz: u32,
    pub dwm_hook_active: bool,
}

impl Default for BarStateSnapshot {
    fn default() -> Self {
        Self {
            active_workspace: 1,
            workspaces: (1..=10)
                .map(|id| WorkspaceSummary {
                    id,
                    active: id == 1,
                    occupied: false,
                    client_count: 0,
                    focused_title: None,
                })
                .collect(),
            active_window_title: String::new(),
            active_window_class: String::new(),
            active_window_process: String::new(),
            active_layout: "dwindle".to_string(),
            monitor_name: r"\\.\DISPLAY1".to_string(),
            monitor_refresh_hz: 60,
            dwm_hook_active: false,
        }
    }
}

/// Top-level IPC request sent across `\\.\pipe\chickenwm-ipc` or module sub-pipes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IpcCommand {
    /// Hot-reload `chickenwm.conf` and CSS stylesheets across all running modules.
    ReloadConfig,
    /// Dispatch a window manager or launcher action.
    Dispatch(Action),
    /// Override or reset target framerate (`None` or `Some(0)` = monitor native Hz).
    SetFramerate(Option<u32>),
    /// Query all managed top-level windows across workspaces.
    QueryClients,
    /// Query all connected monitors and their native refresh rates / DPI / geometry.
    QueryMonitors,
    /// Query current workspace & active window snapshot for `roobar`.
    QueryBarState,
    /// Direct real-time push from `chickenwm-wm` to `roobar` (`\\.\pipe\chickenwm-bar-ipc`) when workspaces or windows change.
    NotifyBarState(BarStateSnapshot),
    /// Summon or toggle `chickenwm-launcher`.
    SummonLauncher,
    /// Trigger immediate failsafe restoration of `explorer.exe`.
    RestoreExplorer,
    /// Toggle `explorer.exe` process suppression and shell registry replacement.
    SetShellConfig {
        replace_winlogon_shell: bool,
        suppress_explorer: bool,
        register_files_handler: bool,
    },
    /// Restart the whole shell with administrator rights (shows the UAC prompt), so windows of
    /// elevated programs can be tiled. Answers with an error if the prompt is declined.
    RestartElevated,
    /// Query overall daemon, WM, and module watchdog status.
    QueryStatus,
    /// Set wallpaper or live WGSL shader on `chickenwm-paper`.
    SetWallpaper {
        monitor: Option<String>,
        path: String,
        transition: Option<String>,
    },
    /// Phase 1.1 & 9.3: Anti-Cheat Safe Switch — toggle between Standard Win32/DirectComposition mode
    /// (`false`) and Experimental DWM DLL Injection (`chickenwm-dwm-hook`) mode (`true`).
    SetCompositorMode {
        dwm_hook_enabled: bool,
    },
    /// Show a notification (`chickenwm-notify`, `\\.\pipe\chickenwm-notify-ipc`).
    Notify(Notification),
    /// Dismiss every notification currently on screen.
    DismissNotifications,
    /// Show / hide the notification center (history of past notifications).
    ToggleNotificationCenter,
}

/// A desktop notification shown by `chickenwm-notify`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Notification {
    /// Sending app ("Discord", "Steam", "chickenctl").
    pub app: String,
    pub title: String,
    pub body: String,
    /// Seconds on screen; `None` = the configured default, `Some(0)` = until dismissed.
    pub timeout_secs: Option<u32>,
    /// `low` | `normal` | `critical` (critical ones stay until dismissed).
    pub urgency: Option<String>,
}

/// Representation of a managed Win32 window client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    pub hwnd: u64,
    pub title: String,
    pub class: String,
    pub process_name: String,
    pub pid: u32,
    pub workspace_id: u32,
    pub monitor_name: String,
    pub rect: [i32; 4], // [x, y, width, height]
    pub floating: bool,
    pub fullscreen: bool,
    pub pseudo: bool,
    pub pinned: bool,
    pub focused: bool,
    pub opacity: f32,
}

/// Representation of a connected display monitor and its refresh rate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorInfo {
    pub id: usize,
    pub name: String,
    pub adapter_name: String,
    pub width: i32,
    pub height: i32,
    pub x: i32,
    pub y: i32,
    pub work_rect: [i32; 4], // [x, y, width, height]
    pub refresh_rate_hz: u32,
    pub dpi_scale: f32,
    pub active_workspace_id: u32,
    pub is_primary: bool,
}

/// Overall system status returned by `QueryStatus`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub version: String,
    pub uptime_secs: u64,
    pub fps_limit: u32,
    pub vfr_enabled: bool,
    pub active_layout: String,
    pub explorer_suppressed: bool,
    pub winlogon_shell_replaced: bool,
    pub dwm_hook_enabled: bool,
    pub modules_running: Vec<String>,
    pub monitor_count: usize,
    pub client_count: usize,
}

/// Unified IPC response payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IpcResponse {
    Ok { message: String },
    Error { message: String },
    Clients(Vec<ClientInfo>),
    Monitors(Vec<MonitorInfo>),
    Status(DaemonStatus),
    BarState(BarStateSnapshot),
}

/// Asynchronous Named Pipe Server listening on `pipe_name`.
/// Creates one pipe instance that any program of the signed-in user can open. The default ACL of
/// an elevated process only admits administrators, which would lock normal-privilege clients
/// (chickenctl, Settings, the launcher, the WM) out of an elevated daemon.
fn create_pipe(name: &str, first: bool) -> std::io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    use windows::core::w;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};

    // SYSTEM, Administrators and interactive users: full access.
    let mut sd = PSECURITY_DESCRIPTOR::default();
    let converted = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            w!("D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;IU)"),
            SDDL_REVISION_1,
            &mut sd,
            None,
        )
    };
    let mut options = ServerOptions::new();
    options.first_pipe_instance(first);
    if converted.is_err() {
        return options.create(name);
    }
    let mut attrs = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0,
        bInheritHandle: false.into(),
    };
    let pipe = unsafe {
        options.create_with_security_attributes_raw(name, &mut attrs as *mut _ as *mut core::ffi::c_void)
    };
    unsafe {
        let _ = LocalFree(HLOCAL(sd.0));
    }
    pipe
}

pub async fn serve_ipc<F, Fut>(pipe_name: &str, handler: F) -> Result<()>
where
    F: Fn(IpcCommand) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = IpcResponse> + Send + 'static,
{
    let pipe_name = pipe_name.to_string();
    let handler = Arc::new(handler);
    let mut server = create_pipe(&pipe_name, true)
        .with_context(|| format!("Failed to create named pipe server {}", pipe_name))?;

    loop {
        let connected_res = server.connect().await;
        // Immediately allocate the next pipe instance before handling the current client
        // so concurrent callers never hit ERROR_FILE_NOT_FOUND / ERROR_PIPE_BUSY.
        let next_server = match create_pipe(&pipe_name, false) {
            Ok(s) => s,
            Err(_) => {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                match create_pipe(&pipe_name, false) {
                    Ok(s) => s,
                    Err(e) => return Err(e).context("Failed to recreate named pipe instance"),
                }
            }
        };
        let connected = std::mem::replace(&mut server, next_server);
        if connected_res.is_err() {
            continue;
        }

        let handler_clone = Arc::clone(&handler);
        tokio::spawn(async move {
            let (reader, mut writer) = tokio::io::split(connected);
            let mut buf_reader = BufReader::new(reader);
            let mut line = String::new();

            if let Ok(bytes_read) = buf_reader.read_line(&mut line).await {
                if bytes_read > 0 {
                    let response = match serde_json::from_str::<IpcCommand>(line.trim()) {
                        Ok(cmd) => handler_clone(cmd).await,
                        Err(e) => IpcResponse::Error {
                            message: format!("Invalid IPC JSON request: {e}"),
                        },
                    };
                    if let Ok(mut encoded) = serde_json::to_vec(&response) {
                        encoded.push(b'\n');
                        let _ = writer.write_all(&encoded).await;
                        let _ = writer.flush().await;
                    }
                }
            }
        });
    }
}

/// Send an `IpcCommand` to `pipe_name` (defaults to `PIPE_NAME_DAEMON`) and await `IpcResponse`.
pub async fn send_ipc_command_to(pipe_name: &str, cmd: &IpcCommand) -> Result<IpcResponse> {
    let mut retries = 0;
    let client = loop {
        match ClientOptions::new().open(pipe_name) {
            Ok(c) => break c,
            Err(e) => {
                retries += 1;
                if retries > 5 {
                    return Err(e).with_context(|| {
                        format!(
                            "Could not connect to ChickenWM IPC pipe '{}' (is chickenwm-rooster running?)",
                            pipe_name
                        )
                    });
                }
                tokio::time::sleep(std::time::Duration::from_millis(15)).await;
            }
        }
    };

    let (reader, mut writer) = tokio::io::split(client);
    let mut payload = serde_json::to_vec(cmd)?;
    payload.push(b'\n');
    writer.write_all(&payload).await?;
    writer.flush().await?;

    let mut buf_reader = BufReader::new(reader);
    let mut response_line = String::new();
    buf_reader.read_line(&mut response_line).await?;

    let resp: IpcResponse = serde_json::from_str(response_line.trim())
        .context("Failed to deserialize IPC response from server")?;
    Ok(resp)
}

/// Convenience helper to send an `IpcCommand` to the primary daemon pipe (`\\.\pipe\chickenwm-ipc`).
pub async fn send_ipc_command(cmd: &IpcCommand) -> Result<IpcResponse> {
    send_ipc_command_to(PIPE_NAME_DAEMON, cmd).await
}

/// Synchronous (blocking) helper for non-async threads (such as Win32 hook threads or GUI callbacks).
pub fn send_ipc_command_blocking(pipe_name: &str, cmd: &IpcCommand) -> Result<IpcResponse> {
    use std::io::{BufRead, BufReader as StdBufReader, Write};
    let mut retries = 0;
    let mut file = loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(pipe_name)
        {
            Ok(f) => break f,
            Err(e) => {
                retries += 1;
                if retries > 5 {
                    return Err(e).with_context(|| format!("Failed to open named pipe {}", pipe_name));
                }
                std::thread::sleep(std::time::Duration::from_millis(8));
            }
        }
    };

    let mut payload = serde_json::to_vec(cmd)?;
    payload.push(b'\n');
    file.write_all(&payload)?;
    file.flush()?;

    let mut reader = StdBufReader::new(file);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let resp: IpcResponse = serde_json::from_str(line.trim())?;
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn secured_pipe_instances_can_be_created_and_connected() {
        let name = format!(r"\\.\pipe\chickenwm-test-{}", std::process::id());
        let first = create_pipe(&name, true).expect("first instance");
        let second = create_pipe(&name, false).expect("second instance");
        let client = ClientOptions::new().open(&name).expect("client connects");
        drop((first, second, client));
    }
}
