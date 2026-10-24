//! Download & Torrent execution engine for `chickenwm-downloader`.
//! Supports HTTP/HTTPS Range pause/resume (`.part` files), token-bucket speed limiting
//! (global + per-download), BitTorrent/Magnet execution, and JSON state persistence.

use crate::torrent::{
    announce_http_tracker, announce_udp_tracker, download_from_peer, resolve_torrent_target_path,
    TorrentMeta,
};
use anyhow::{anyhow, bail, Result};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadKind {
    Http,
    Torrent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadStatus {
    Queued,
    Downloading,
    Paused,
    Completed,
    Failed(String),
}

impl DownloadStatus {
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Downloading | Self::Queued)
    }

    pub fn can_pause(&self) -> bool {
        matches!(self, Self::Downloading | Self::Queued)
    }

    pub fn can_resume(&self) -> bool {
        matches!(self, Self::Paused | Self::Failed(_) | Self::Queued)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadItem {
    pub id: u64,
    pub kind: DownloadKind,
    pub url: String,
    pub filename: String,
    pub save_dir: String,
    pub received_bytes: u64,
    pub total_bytes: u64,
    pub status: DownloadStatus,
    pub speed_bps: u64,
    /// Optional per-download speed limit in KB/s (`0` = follow global limit).
    #[serde(default)]
    pub speed_limit_kbps: u32,
    pub added_at: u64,
    /// Torrent-specific metadata
    #[serde(default)]
    pub info_hash_hex: String,
    #[serde(default)]
    pub peers_connected: u32,
    #[serde(default)]
    pub seeders: u32,
    #[serde(default)]
    pub leechers: u32,
    #[serde(default)]
    pub completed_pieces: Vec<bool>,
}

impl DownloadItem {
    pub fn target_path(&self) -> PathBuf {
        Path::new(&self.save_dir).join(&self.filename)
    }

    pub fn part_path(&self) -> PathBuf {
        Path::new(&self.save_dir).join(format!("{}.part", self.filename))
    }

    pub fn progress_fraction(&self) -> f32 {
        if self.status == DownloadStatus::Completed {
            return 1.0;
        }
        if self.total_bytes > 0 {
            (self.received_bytes as f64 / self.total_bytes as f64).clamp(0.0, 1.0) as f32
        } else if !self.completed_pieces.is_empty() {
            let done = self.completed_pieces.iter().filter(|&&b| b).count();
            (done as f32 / self.completed_pieces.len() as f32).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    pub fn eta_seconds(&self) -> Option<u64> {
        if self.status != DownloadStatus::Downloading || self.speed_bps == 0 {
            return None;
        }
        if self.total_bytes > self.received_bytes {
            Some((self.total_bytes - self.received_bytes) / self.speed_bps)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedState {
    pub default_save_dir: String,
    pub ask_save_location: bool,
    pub global_speed_limit_kbps: u32,
    pub next_id: u64,
    pub items: Vec<DownloadItem>,
}

impl Default for PersistedState {
    fn default() -> Self {
        let default_dir = dirs::download_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."))
            .to_string_lossy()
            .into_owned();
        Self {
            default_save_dir: default_dir,
            ask_save_location: true,
            global_speed_limit_kbps: 0,
            next_id: 1,
            items: Vec::new(),
        }
    }
}

pub struct WorkerControl {
    pub stop_flag: AtomicBool,
    pub item_limit_kbps: AtomicU32,
}

pub struct DownloadEngine {
    pub state: Arc<RwLock<PersistedState>>,
    pub state_path: PathBuf,
    pub global_limit_kbps: Arc<AtomicU32>,
    workers: Arc<RwLock<HashMap<u64, Arc<WorkerControl>>>>,
}

impl DownloadEngine {
    pub fn load_or_new(state_path: PathBuf) -> Arc<Self> {
        let mut persisted: PersistedState = std::fs::read_to_string(&state_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

        // Any download that was `Downloading` when the app closed is marked `Paused` so the user
        // can resume it cleanly with one click (preserving its `.part` file on disk).
        for item in &mut persisted.items {
            if item.status == DownloadStatus::Downloading || item.status == DownloadStatus::Queued {
                item.status = DownloadStatus::Paused;
            }
            item.speed_bps = 0;
            if item.kind == DownloadKind::Http && item.status != DownloadStatus::Completed {
                if let Ok(meta) = std::fs::metadata(item.part_path()) {
                    item.received_bytes = meta.len();
                }
            }
        }

        let global_limit = Arc::new(AtomicU32::new(persisted.global_speed_limit_kbps));
        Arc::new(Self {
            state: Arc::new(RwLock::new(persisted)),
            state_path,
            global_limit_kbps: global_limit,
            workers: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub fn save(&self) {
        let json = {
            let st = self.state.read();
            serde_json::to_string_pretty(&*st).ok()
        };
        if let Some(json) = json {
            if let Some(parent) = self.state_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&self.state_path, json);
        }
    }

    pub fn set_global_speed_limit(self: &Arc<Self>, kbps: u32) {
        self.global_limit_kbps.store(kbps, Ordering::Relaxed);
        self.state.write().global_speed_limit_kbps = kbps;
        self.save();
    }

    pub fn set_item_speed_limit(self: &Arc<Self>, id: u64, kbps: u32) {
        if let Some(ctrl) = self.workers.read().get(&id) {
            ctrl.item_limit_kbps.store(kbps, Ordering::Relaxed);
        }
        if let Some(item) = self.state.write().items.iter_mut().find(|i| i.id == id) {
            item.speed_limit_kbps = kbps;
        }
        self.save();
    }

    pub fn add_download(
        self: &Arc<Self>,
        url: String,
        filename: Option<String>,
        save_dir: Option<String>,
        is_torrent_hint: bool,
        start_immediately: bool,
    ) -> u64 {
        let is_torrent = is_torrent_hint || detect_is_torrent(&url, filename.as_deref());
        let kind = if is_torrent {
            DownloadKind::Torrent
        } else {
            DownloadKind::Http
        };

        let resolved_filename = filename
            .map(|f| sanitize_filename(&f))
            .filter(|f| !f.is_empty())
            .unwrap_or_else(|| infer_filename_from_url(&url, is_torrent));

        let id = {
            let mut st = self.state.write();
            let id = st.next_id;
            st.next_id += 1;
            let dir = save_dir
                .filter(|d| !d.trim().is_empty())
                .unwrap_or_else(|| st.default_save_dir.clone());
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            st.items.insert(
                0,
                DownloadItem {
                    id,
                    kind,
                    url,
                    filename: resolved_filename,
                    save_dir: dir,
                    received_bytes: 0,
                    total_bytes: 0,
                    status: if start_immediately {
                        DownloadStatus::Queued
                    } else {
                        DownloadStatus::Paused
                    },
                    speed_bps: 0,
                    speed_limit_kbps: 0,
                    added_at: now,
                    info_hash_hex: String::new(),
                    peers_connected: 0,
                    seeders: 0,
                    leechers: 0,
                    completed_pieces: Vec::new(),
                },
            );
            id
        };
        self.save();
        if start_immediately {
            self.start_or_resume(id);
        }
        id
    }

    pub fn pause(self: &Arc<Self>, id: u64) {
        if let Some(ctrl) = self.workers.write().remove(&id) {
            ctrl.stop_flag.store(true, Ordering::SeqCst);
        }
        if let Some(item) = self.state.write().items.iter_mut().find(|i| i.id == id) {
            if item.status != DownloadStatus::Completed {
                item.status = DownloadStatus::Paused;
                item.speed_bps = 0;
            }
        }
        self.save();
    }

    pub fn pause_all_in_tab(self: &Arc<Self>, kind: DownloadKind) {
        let ids: Vec<u64> = self
            .state
            .read()
            .items
            .iter()
            .filter(|i| i.kind == kind && i.status.can_pause())
            .map(|i| i.id)
            .collect();
        for id in ids {
            self.pause(id);
        }
    }

    pub fn resume_all_in_tab(self: &Arc<Self>, kind: DownloadKind) {
        let ids: Vec<u64> = self
            .state
            .read()
            .items
            .iter()
            .filter(|i| i.kind == kind && i.status.can_resume())
            .map(|i| i.id)
            .collect();
        for id in ids {
            self.start_or_resume(id);
        }
    }

    pub fn change_save_location(self: &Arc<Self>, id: u64, new_dir: String) {
        let was_active = self
            .state
            .read()
            .items
            .iter()
            .find(|i| i.id == id)
            .is_some_and(|i| i.status.is_active());
        if was_active {
            self.pause(id);
        }
        {
            let mut st = self.state.write();
            if let Some(item) = st.items.iter_mut().find(|i| i.id == id) {
                let old_part = item.part_path();
                let old_target = item.target_path();
                item.save_dir = new_dir;
                let new_part = item.part_path();
                let new_target = item.target_path();
                let _ = std::fs::create_dir_all(&item.save_dir);
                if old_part.exists() && old_part != new_part {
                    let _ = std::fs::rename(&old_part, &new_part);
                }
                if old_target.exists() && old_target != new_target {
                    let _ = std::fs::rename(&old_target, &new_target);
                }
            }
        }
        self.save();
        if was_active {
            self.start_or_resume(id);
        }
    }

    pub fn remove_item(self: &Arc<Self>, id: u64, delete_files: bool) {
        self.pause(id);
        let removed = {
            let mut st = self.state.write();
            if let Some(pos) = st.items.iter().position(|i| i.id == id) {
                Some(st.items.remove(pos))
            } else {
                None
            }
        };
        if delete_files {
            if let Some(item) = removed {
                let _ = std::fs::remove_file(item.part_path());
                let _ = std::fs::remove_file(item.target_path());
            }
        }
        self.save();
    }

    pub fn clear_completed(self: &Arc<Self>, kind: DownloadKind) {
        self.state
            .write()
            .items
            .retain(|i| !(i.kind == kind && i.status == DownloadStatus::Completed));
        self.save();
    }

    pub fn start_or_resume(self: &Arc<Self>, id: u64) {
        if self.workers.read().contains_key(&id) {
            return;
        }
        let item_snapshot = {
            let mut st = self.state.write();
            let Some(item) = st.items.iter_mut().find(|i| i.id == id) else {
                return;
            };
            if item.status == DownloadStatus::Completed {
                return;
            }
            item.status = DownloadStatus::Downloading;
            item.speed_bps = 0;
            item.clone()
        };
        self.save();

        let ctrl = Arc::new(WorkerControl {
            stop_flag: AtomicBool::new(false),
            item_limit_kbps: AtomicU32::new(item_snapshot.speed_limit_kbps),
        });
        self.workers.write().insert(id, Arc::clone(&ctrl));

        let engine = Arc::clone(self);
        std::thread::Builder::new()
            .name(format!("chickenwm-dl-{id}"))
            .spawn(move || {
                let res = match item_snapshot.kind {
                    DownloadKind::Http => engine.run_http_worker(id, &item_snapshot, &ctrl),
                    DownloadKind::Torrent => engine.run_torrent_worker(id, &item_snapshot, &ctrl),
                };
                engine.workers.write().remove(&id);

                let stopped = ctrl.stop_flag.load(Ordering::SeqCst);
                let mut completed_name = None;
                {
                    let mut st = engine.state.write();
                    if let Some(item) = st.items.iter_mut().find(|i| i.id == id) {
                        item.speed_bps = 0;
                        if stopped {
                            if item.status != DownloadStatus::Completed {
                                item.status = DownloadStatus::Paused;
                            }
                        } else {
                            match res {
                                Ok(()) => {
                                    item.status = DownloadStatus::Completed;
                                    if item.total_bytes > 0 {
                                        item.received_bytes = item.total_bytes;
                                    }
                                    completed_name = Some(item.filename.clone());
                                }
                                Err(e) => {
                                    item.status = DownloadStatus::Failed(e.to_string());
                                }
                            }
                        }
                    }
                }
                engine.save();
                if let Some(name) = completed_name {
                    notify_download_finished(&name);
                }
            })
            .ok();
    }

    fn effective_limit_kbps(&self, ctrl: &WorkerControl) -> u32 {
        let item_lim = ctrl.item_limit_kbps.load(Ordering::Relaxed);
        let global_lim = self.global_limit_kbps.load(Ordering::Relaxed);
        match (item_lim, global_lim) {
            (0, g) => g,
            (i, 0) => i,
            (i, g) => i.min(g),
        }
    }

    fn run_http_worker(
        &self,
        id: u64,
        snapshot: &DownloadItem,
        ctrl: &Arc<WorkerControl>,
    ) -> Result<()> {
        #[cfg(windows)]
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        let _ = std::fs::create_dir_all(&snapshot.save_dir);
        let part_path = snapshot.part_path();
        let target_path = snapshot.target_path();

        let existing_len = std::fs::metadata(&part_path).map(|m| m.len()).unwrap_or(0);

        // Probe total size / range support via HEAD or headers dump
        let header_path = std::env::temp_dir().join(format!("chickenwm_dl_hdr_{id}.txt"));
        let _ = std::fs::remove_file(&header_path);

        let mut cmd = Command::new("curl.exe");
        cmd.args([
            "-sSL",
            "--fail",
            "--connect-timeout",
            "15",
            "-A",
            "ChickenWM-Downloader/0.2.1",
            "-D",
            &header_path.to_string_lossy(),
        ]);
        if existing_len > 0 {
            cmd.args(["-r", &format!("{existing_len}-")]);
        }
        cmd.arg(&snapshot.url);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);

        let mut child = cmd.spawn().map_err(|e| anyhow!("failed to spawn curl.exe: {e}"))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("failed to open download stream"))?;

        let mut file = if existing_len > 0 {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&part_path)?
        } else {
            std::fs::File::create(&part_path)?
        };

        let mut received = existing_len;
        let mut headers_parsed = false;
        let mut buf = [0u8; 16_384];
        let mut rate_window_start = Instant::now();
        let mut rate_window_bytes = 0u64;
        let mut speed_tick = Instant::now();
        let mut speed_bytes = 0u64;

        loop {
            if ctrl.stop_flag.load(Ordering::SeqCst) {
                let _ = child.kill();
                let _ = file.flush();
                return Ok(());
            }

            let n = stdout.read(&mut buf)?;
            if n == 0 {
                break;
            }

            if !headers_parsed {
                if let Some((total, accepted_range)) = parse_dumped_headers(&header_path, existing_len) {
                    headers_parsed = true;
                    if existing_len > 0 && !accepted_range {
                        // Server ignored Range and sent 200 OK from byte 0; truncate and rewrite
                        file = std::fs::File::create(&part_path)?;
                        received = 0;
                    }
                    if let Some(item) = self.state.write().items.iter_mut().find(|i| i.id == id) {
                        if total > 0 {
                            item.total_bytes = total;
                        }
                    }
                }
            }

            file.write_all(&buf[..n])?;
            received += n as u64;
            rate_window_bytes += n as u64;
            speed_bytes += n as u64;

            if speed_tick.elapsed() >= Duration::from_millis(250) {
                let dt = speed_tick.elapsed().as_secs_f64().max(0.001);
                let bps = (speed_bytes as f64 / dt) as u64;
                speed_tick = Instant::now();
                speed_bytes = 0;
                if let Some(item) = self.state.write().items.iter_mut().find(|i| i.id == id) {
                    item.received_bytes = received;
                    item.speed_bps = bps;
                }
            }

            // Token-bucket rate limiter
            let limit_kbps = self.effective_limit_kbps(ctrl);
            if limit_kbps > 0 {
                let limit_bps = (limit_kbps as f64) * 1024.0;
                let expected_secs = (rate_window_bytes as f64) / limit_bps;
                let elapsed = rate_window_start.elapsed().as_secs_f64();
                if expected_secs > elapsed {
                    std::thread::sleep(Duration::from_secs_f64((expected_secs - elapsed).min(0.25)));
                }
                if rate_window_start.elapsed() >= Duration::from_secs(1) {
                    rate_window_start = Instant::now();
                    rate_window_bytes = 0;
                }
            }
        }

        file.flush()?;
        drop(file);
        let status = child.wait()?;
        let _ = std::fs::remove_file(&header_path);

        if ctrl.stop_flag.load(Ordering::SeqCst) {
            return Ok(());
        }
        if !status.success() {
            bail!("HTTP download interrupted (status {})", status);
        }

        if target_path.exists() {
            let _ = std::fs::remove_file(&target_path);
        }
        std::fs::rename(&part_path, &target_path)?;
        if let Some(item) = self.state.write().items.iter_mut().find(|i| i.id == id) {
            item.received_bytes = received;
            if item.total_bytes == 0 {
                item.total_bytes = received;
            }
        }
        Ok(())
    }

    fn run_torrent_worker(
        &self,
        id: u64,
        snapshot: &DownloadItem,
        ctrl: &Arc<WorkerControl>,
    ) -> Result<()> {
        let _ = std::fs::create_dir_all(&snapshot.save_dir);

        // 1. Parse metadata from magnet URI, local .torrent file, or remote .torrent URL
        let meta = if snapshot.url.trim().starts_with("magnet:") {
            TorrentMeta::from_magnet_uri(&snapshot.url)?
        } else if Path::new(&snapshot.url).exists() {
            let bytes = std::fs::read(&snapshot.url)?;
            TorrentMeta::from_torrent_bytes(&bytes)?
        } else if snapshot.url.starts_with("http://") || snapshot.url.starts_with("https://") {
            let bytes = fetch_url_bytes(&snapshot.url)?;
            TorrentMeta::from_torrent_bytes(&bytes)?
        } else {
            bail!("unsupported torrent source: {}", snapshot.url);
        };

        let num_pieces = if !meta.piece_hashes.is_empty() {
            meta.piece_hashes.len()
        } else if meta.total_size > 0 && meta.piece_length > 0 {
            ((meta.total_size + meta.piece_length - 1) / meta.piece_length) as usize
        } else {
            16
        };

        let mut completed_pieces = {
            let mut st = self.state.write();
            let Some(item) = st.items.iter_mut().find(|i| i.id == id) else {
                return Ok(());
            };
            if !meta.name.is_empty() && item.filename.starts_with("magnet-") {
                item.filename = sanitize_filename(&meta.name);
            }
            item.info_hash_hex = meta.info_hash_hex();
            if meta.total_size > 0 {
                item.total_bytes = meta.total_size;
            }
            if item.completed_pieces.len() != num_pieces {
                item.completed_pieces = vec![false; num_pieces];
            }
            item.completed_pieces.clone()
        };

        let mut peer_id = [b'0'; 20];
        peer_id[0..8].copy_from_slice(b"-CW0210-");
        let pid_seed = (id as u32).wrapping_mul(0x9E37_79B9) ^ std::process::id();
        for (i, b) in format!("{pid_seed:012x}").bytes().take(12).enumerate() {
            peer_id[8 + i] = b;
        }

        // 2. Query trackers (UDP BEP 15 and HTTP/HTTPS)
        let mut all_peers = Vec::new();
        let mut max_seeders = 0u32;
        let mut max_leechers = 0u32;

        for tr in &meta.trackers {
            if ctrl.stop_flag.load(Ordering::SeqCst) {
                return Ok(());
            }
            let res = if tr.starts_with("udp://") {
                announce_udp_tracker(
                    tr,
                    &meta.info_hash,
                    &peer_id,
                    snapshot.received_bytes,
                    meta.total_size.saturating_sub(snapshot.received_bytes),
                )
            } else if tr.starts_with("http://") || tr.starts_with("https://") {
                announce_http_tracker(
                    tr,
                    &meta.info_hash,
                    &peer_id,
                    snapshot.received_bytes,
                    meta.total_size.saturating_sub(snapshot.received_bytes),
                )
            } else {
                continue;
            };

            if let Ok(ann) = res {
                max_seeders = max_seeders.max(ann.seeders);
                max_leechers = max_leechers.max(ann.leechers);
                for p in ann.peers {
                    if !all_peers.contains(&p) {
                        all_peers.push(p);
                    }
                }
            }
        }

        if let Some(item) = self.state.write().items.iter_mut().find(|i| i.id == id) {
            item.seeders = max_seeders;
            item.leechers = max_leechers;
            item.peers_connected = all_peers.len().min(12) as u32;
        }

        let output_path = resolve_torrent_target_path(Path::new(&snapshot.save_dir), &meta.name);
        let mut speed_tick = Instant::now();
        let mut speed_bytes = 0u64;

        for peer in all_peers.iter().take(24) {
            if ctrl.stop_flag.load(Ordering::SeqCst) {
                return Ok(());
            }
            if completed_pieces.iter().all(|&b| b) && !meta.piece_hashes.is_empty() {
                break;
            }
            let _ = download_from_peer(
                *peer,
                &meta,
                &peer_id,
                &output_path,
                &mut completed_pieces,
                || ctrl.stop_flag.load(Ordering::SeqCst),
                || self.effective_limit_kbps(ctrl),
                |delta| {
                    speed_bytes += delta;
                    if speed_tick.elapsed() >= Duration::from_millis(250) {
                        let dt = speed_tick.elapsed().as_secs_f64().max(0.001);
                        let bps = (speed_bytes as f64 / dt) as u64;
                        speed_tick = Instant::now();
                        speed_bytes = 0;
                        if let Some(item) = self.state.write().items.iter_mut().find(|i| i.id == id) {
                            item.received_bytes = item.received_bytes.saturating_add(delta);
                            item.speed_bps = bps;
                        }
                    } else if let Some(item) = self.state.write().items.iter_mut().find(|i| i.id == id) {
                        item.received_bytes = item.received_bytes.saturating_add(delta);
                    }
                },
            );
            if let Some(item) = self.state.write().items.iter_mut().find(|i| i.id == id) {
                item.completed_pieces = completed_pieces.clone();
            }
            self.save();
        }

        if ctrl.stop_flag.load(Ordering::SeqCst) {
            return Ok(());
        }

        if !meta.piece_hashes.is_empty() && completed_pieces.iter().all(|&b| b) {
            Ok(())
        } else if all_peers.is_empty() {
            bail!("no active peers returned by trackers (can resume later)")
        } else {
            bail!("peers choked or metadata exchange pending (click Resume to retry)")
        }
    }
}

fn fetch_url_bytes(url: &str) -> Result<Vec<u8>> {
    #[cfg(windows)]
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let mut cmd = Command::new("curl.exe");
    cmd.args(["-sSL", "--fail", "--max-time", "15", url]);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let out = cmd.output()?;
    if !out.status.success() || out.stdout.is_empty() {
        bail!("failed to fetch .torrent URL");
    }
    Ok(out.stdout)
}

fn parse_dumped_headers(header_path: &Path, existing_len: u64) -> Option<(u64, bool)> {
    let file = std::fs::File::open(header_path).ok()?;
    let reader = BufReader::new(file);
    let mut status_code = 0u16;
    let mut content_len = 0u64;
    let mut content_range_total = None;

    for line in reader.lines().map_while(Result::ok) {
        let trim = line.trim();
        if let Some(rest) = trim.strip_prefix("HTTP/") {
            if let Some(code_str) = rest.split_whitespace().nth(1) {
                if let Ok(code) = code_str.parse::<u16>() {
                    status_code = code;
                    content_len = 0;
                    content_range_total = None;
                }
            }
        } else if let Some((k, v)) = trim.split_once(':') {
            let k_lower = k.trim().to_ascii_lowercase();
            let v_trim = v.trim();
            if k_lower == "content-length" {
                content_len = v_trim.parse::<u64>().unwrap_or(0);
            } else if k_lower == "content-range" {
                if let Some((_, total_str)) = v_trim.rsplit_once('/') {
                    content_range_total = total_str.trim().parse::<u64>().ok();
                }
            }
        }
    }

    if status_code == 0 {
        return None;
    }
    let accepted_range = status_code == 206;
    let total = content_range_total.unwrap_or_else(|| {
        if accepted_range {
            existing_len + content_len
        } else {
            content_len
        }
    });
    Some((total, accepted_range))
}

pub fn detect_is_torrent(url: &str, filename: Option<&str>) -> bool {
    let u = url.trim().to_ascii_lowercase();
    if u.starts_with("magnet:") {
        return true;
    }
    let path_part = u.split('?').next().unwrap_or(&u);
    if path_part.ends_with(".torrent") {
        return true;
    }
    if let Some(f) = filename {
        if f.trim().to_ascii_lowercase().ends_with(".torrent") {
            return true;
        }
    }
    false
}

pub fn sanitize_filename(name: &str) -> String {
    let base = Path::new(name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(name);
    let cleaned: String = base
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            _ => c,
        })
        .collect();
    cleaned.trim().to_string()
}

pub fn infer_filename_from_url(url: &str, is_torrent: bool) -> String {
    let trim = url.trim();
    if trim.to_ascii_lowercase().starts_with("magnet:") {
        if let Ok(meta) = TorrentMeta::from_magnet_uri(trim) {
            return sanitize_filename(&meta.name);
        }
        return "magnet-download.torrent".to_string();
    }
    let without_query = trim.split('?').next().unwrap_or(trim);
    let last_seg = without_query
        .rsplit('/')
        .next()
        .unwrap_or("")
        .trim();
    let decoded = crate::torrent::percent_decode(last_seg);
    let clean = sanitize_filename(&decoded);
    if !clean.is_empty() && clean != "." {
        clean
    } else if is_torrent {
        "download.torrent".to_string()
    } else {
        "download.bin".to_string()
    }
}

pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

pub fn format_speed(bps: u64) -> String {
    if bps == 0 {
        "0 KB/s".to_string()
    } else {
        format!("{}/s", format_bytes(bps))
    }
}

pub fn format_eta(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

fn notify_download_finished(filename: &str) {
    let body = filename.to_string();
    std::thread::spawn(move || {
        let cmd = chickenwm_core::ipc::IpcCommand::Notify(chickenwm_core::ipc::Notification {
            app: "ChickenWM Downloader".to_string(),
            title: "Download complete".to_string(),
            body,
            timeout_secs: Some(5),
            urgency: Some("normal".to_string()),
        });
        let _ = chickenwm_core::ipc::send_ipc_command_blocking(
            chickenwm_core::ipc::PIPE_NAME_NOTIFY,
            &cmd,
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_torrent_and_magnet_links_accurately() {
        assert!(detect_is_torrent("magnet:?xt=urn:btih:a9993e364706816aba3e25717850c26c9cd0d89d", None));
        assert!(detect_is_torrent("https://releases.ubuntu.com/24.04/ubuntu.iso.torrent?mirror=1", None));
        assert!(detect_is_torrent("https://example.com/dl?id=5", Some("archlinux.torrent")));
        assert!(!detect_is_torrent("https://example.com/archive.zip", Some("archive.zip")));
    }

    #[test]
    fn infers_clean_filenames_from_urls_and_magnets() {
        assert_eq!(
            infer_filename_from_url("https://example.com/files/ Chicken%20WM%20Setup.exe?token=abc", false),
            "Chicken WM Setup.exe"
        );
        assert_eq!(
            infer_filename_from_url(
                "magnet:?xt=urn:btih:a9993e364706816aba3e25717850c26c9cd0d89d&dn=Debian+12+Live.iso",
                true
            ),
            "Debian 12 Live.iso"
        );
    }

    #[test]
    fn engine_persists_pause_resume_and_speed_limit_state() {
        let tmp_json = std::env::temp_dir().join("chickenwm_downloader_test_state.json");
        let _ = std::fs::remove_file(&tmp_json);

        let engine = DownloadEngine::load_or_new(tmp_json.clone());
        engine.set_global_speed_limit(2048);

        let id1 = engine.add_download(
            "https://example.com/large-archive.zip".to_string(),
            Some("large-archive.zip".to_string()),
            Some("C:\\Users\\Public\\Downloads".to_string()),
            false,
            false,
        );
        let id2 = engine.add_download(
            "magnet:?xt=urn:btih:a9993e364706816aba3e25717850c26c9cd0d89d&dn=Linux.iso".to_string(),
            None,
            Some("C:\\Users\\Public\\Downloads".to_string()),
            true,
            false,
        );
        engine.set_item_speed_limit(id1, 512);

        // Reload from disk to verify "resume later" persistence across restarts
        let reloaded = DownloadEngine::load_or_new(tmp_json.clone());
        let st = reloaded.state.read();
        assert_eq!(st.global_speed_limit_kbps, 2048);
        assert_eq!(st.items.len(), 2);
        let http_item = st.items.iter().find(|i| i.id == id1).unwrap();
        assert_eq!(http_item.kind, DownloadKind::Http);
        assert_eq!(http_item.status, DownloadStatus::Paused);
        assert_eq!(http_item.speed_limit_kbps, 512);

        let torrent_item = st.items.iter().find(|i| i.id == id2).unwrap();
        assert_eq!(torrent_item.kind, DownloadKind::Torrent);
        assert_eq!(torrent_item.filename, "Linux.iso");

        let _ = std::fs::remove_file(&tmp_json);
    }
}
