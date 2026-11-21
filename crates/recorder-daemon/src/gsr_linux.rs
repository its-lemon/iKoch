//! Vice-style priority Linux capture backend using `gpu-screen-recorder` when present in `PATH`.
//!
//! When `gpu-screen-recorder` is installed, it provides low-overhead Wayland/X11 replay buffering
//! (`SIGUSR1` to save replay) and session recording (`SIGINT` to finalize), while `iKoch` manages
//! `Clips/` / `Sessions/` output paths, `.json` sidecar metadata, hotkeys, and storage quotas.

use core_types::{CaptureTarget, VideoCodec};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tracing::{info, warn};

use crate::AppSettings;

/// Checks whether `gpu-screen-recorder` exists as an executable in `PATH`.
#[must_use]
pub fn is_gpu_screen_recorder_available() -> bool {
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path_var).any(|dir| dir.join("gpu-screen-recorder").is_file())
}

fn send_signal(pid: u32, sig: &str) -> Result<(), String> {
    let status = Command::new("kill")
        .args([sig, &pid.to_string()])
        .status()
        .map_err(|e| format!("Failed to invoke kill {sig} {pid}: {e}"))?;
    if !status.success() {
        return Err(format!("kill {sig} {pid} exited with {status}"));
    }
    Ok(())
}

fn move_file_cross_fs(src: &Path, dst: &Path) -> Result<u64, String> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create_dir_all {}: {e}", parent.display()))?;
    }
    if fs::rename(src, dst).is_ok() {
        return fs::metadata(dst)
            .map(|m| m.len())
            .map_err(|e| format!("stat {}: {e}", dst.display()));
    }
    let bytes = fs::copy(src, dst).map_err(|e| format!("copy {} -> {}: {e}", src.display(), dst.display()))?;
    let _ = fs::remove_file(src);
    Ok(bytes)
}

fn resolve_gsr_window_arg(target: &CaptureTarget) -> String {
    let is_wayland = std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v.eq_ignore_ascii_case("wayland"));
    if is_wayland {
        return "portal".to_string();
    }
    match target {
        CaptureTarget::Monitor { monitor_id, .. }
            if !monitor_id.is_empty()
                && monitor_id != "DISPLAY-0"
                && monitor_id != "wayland-0" =>
        {
            monitor_id.clone()
        }
        _ => "screen".to_string(),
    }
}

fn build_gsr_common_args(target: &CaptureTarget, settings: &AppSettings) -> Vec<String> {
    let mut args = vec![
        "-w".to_string(),
        resolve_gsr_window_arg(target),
        "-f".to_string(),
        settings.video.fps.clamp(15, 240).to_string(),
        "-c".to_string(),
        "mp4".to_string(),
        "-k".to_string(),
        match settings.video.codec {
            VideoCodec::Hevc => "hevc".to_string(),
            _ => "h264".to_string(),
        },
        "-restore-portal-session".to_string(),
        "yes".to_string(),
    ];

    if settings.audio.game_enabled && settings.audio.mic_enabled {
        args.extend(["-a".to_string(), "default_output|default_input".to_string()]);
        if settings.audio.separate_tracks {
            args.extend(["-a".to_string(), "default_output".to_string()]);
            args.extend(["-a".to_string(), "default_input".to_string()]);
        }
    } else if settings.audio.game_enabled {
        args.extend(["-a".to_string(), "default_output".to_string()]);
    } else if settings.audio.mic_enabled {
        args.extend(["-a".to_string(), "default_input".to_string()]);
    }

    args
}

/// Manages `gpu-screen-recorder` replay buffer and session recording processes on Linux.
pub struct GsrBackend {
    replay_staging_dir: PathBuf,
    replay_child: Option<Child>,
    session_child: Option<(Child, PathBuf, Instant)>,
}

impl GsrBackend {
    /// Creates a new [`GsrBackend`] using `temp_dir/gsr_replay` as staging folder.
    #[must_use]
    pub fn new(temp_dir: &Path) -> Self {
        let replay_staging_dir = temp_dir.join("gsr_replay");
        let _ = fs::create_dir_all(&replay_staging_dir);
        Self {
            replay_staging_dir,
            replay_child: None,
            session_child: None,
        }
    }

    #[must_use]
    pub fn is_replay_active(&self) -> bool {
        self.replay_child.is_some()
    }

    #[must_use]
    pub fn is_replay_running(&mut self) -> bool {
        if let Some(child) = self.replay_child.as_mut() {
            match child.try_wait() {
                Ok(None) => return true,
                Ok(Some(status)) => {
                    warn!("gpu-screen-recorder replay exited unexpectedly: {status}");
                    self.replay_child = None;
                }
                Err(_) => {
                    self.replay_child = None;
                }
            }
        }
        false
    }

    /// Starts the `gpu-screen-recorder` circular replay buffer process if not already running.
    pub fn ensure_replay_running(
        &mut self,
        target: &CaptureTarget,
        settings: &AppSettings,
    ) -> Result<(), String> {
        if self.is_replay_running() {
            return Ok(());
        }
        fs::create_dir_all(&self.replay_staging_dir)
            .map_err(|e| format!("create gsr staging dir failed: {e}"))?;

        let mut args = build_gsr_common_args(target, settings);
        args.push("-r".to_string());
        args.push(settings.replay_buffer.duration_secs.clamp(15, 300).to_string());
        args.push("-o".to_string());
        args.push(self.replay_staging_dir.to_string_lossy().to_string());

        let child = Command::new("gpu-screen-recorder")
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("spawn gpu-screen-recorder replay failed: {e}"))?;

        info!(
            "Started gpu-screen-recorder replay buffer (pid={}, window={}, {}s)",
            child.id(),
            resolve_gsr_window_arg(target),
            settings.replay_buffer.duration_secs
        );
        self.replay_child = Some(child);
        Ok(())
    }

    /// Stops the active replay buffer process cleanly.
    pub fn stop_replay(&mut self) {
        if let Some(mut child) = self.replay_child.take() {
            let pid = child.id();
            let _ = send_signal(pid, "-INT");
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline {
                if let Ok(Some(_)) = child.try_wait() {
                    return;
                }
                thread::sleep(Duration::from_millis(50));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Triggers `SIGUSR1` on `gpu-screen-recorder` and moves the saved replay `.mp4` to `dest_mp4`.
    pub fn save_replay_clip(
        &mut self,
        dest_mp4: &Path,
        fallback_duration_secs: f64,
    ) -> Result<(PathBuf, f64, u64), String> {
        if !self.is_replay_running() {
            return Err("gpu-screen-recorder replay buffer is not running".to_string());
        }
        let pid = self
            .replay_child
            .as_ref()
            .map(Child::id)
            .ok_or_else(|| "Missing replay child pid".to_string())?;

        let before: HashSet<PathBuf> = fs::read_dir(&self.replay_staging_dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .collect();

        send_signal(pid, "-USR1")?;

        let deadline = Instant::now() + Duration::from_secs(6);
        let mut found: Option<PathBuf> = None;
        while Instant::now() < deadline {
            if let Ok(entries) = fs::read_dir(&self.replay_staging_dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if !before.contains(&p)
                        && p.extension()
                            .and_then(|e| e.to_str())
                            .is_some_and(|e| e.eq_ignore_ascii_case("mp4"))
                    {
                        found = Some(p);
                        break;
                    }
                }
            }
            if found.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }

        let src = found.ok_or_else(|| {
            "Timed out waiting for gpu-screen-recorder to write replay MP4".to_string()
        })?;

        // Wait until file size stabilizes (writer closed).
        let mut prev_len = 0u64;
        for _ in 0..20 {
            let cur_len = fs::metadata(&src).map(|m| m.len()).unwrap_or(0);
            if cur_len > 0 && cur_len == prev_len {
                break;
            }
            prev_len = cur_len;
            thread::sleep(Duration::from_millis(120));
        }

        let bytes = move_file_cross_fs(&src, dest_mp4)?;
        Ok((dest_mp4.to_path_buf(), fallback_duration_secs, bytes))
    }

    /// Starts a full-length session recording directly into `dest_mp4`.
    pub fn start_session(
        &mut self,
        dest_mp4: &Path,
        target: &CaptureTarget,
        settings: &AppSettings,
    ) -> Result<(), String> {
        if self.session_child.is_some() {
            return Err("gpu-screen-recorder session already active".to_string());
        }
        if let Some(parent) = dest_mp4.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("create session dir failed: {e}"))?;
        }
        let mut args = build_gsr_common_args(target, settings);
        args.push("-o".to_string());
        args.push(dest_mp4.to_string_lossy().to_string());

        let child = Command::new("gpu-screen-recorder")
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("spawn gpu-screen-recorder session failed: {e}"))?;

        info!(
            "Started gpu-screen-recorder session (pid={}) -> {}",
            child.id(),
            dest_mp4.display()
        );
        self.session_child = Some((child, dest_mp4.to_path_buf(), Instant::now()));
        Ok(())
    }

    /// Finalizes the active session recording via `SIGINT` and returns `(path, elapsed_sec, bytes)`.
    pub fn stop_session(&mut self) -> Result<(PathBuf, f64, u64), String> {
        let (mut child, path, started) = self
            .session_child
            .take()
            .ok_or_else(|| "No active gpu-screen-recorder session".to_string())?;
        let pid = child.id();
        let _ = send_signal(pid, "-INT");
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut exited = false;
        while Instant::now() < deadline {
            if let Ok(Some(_)) = child.try_wait() {
                exited = true;
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        if !exited {
            let _ = child.kill();
            let _ = child.wait();
        }
        let bytes = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let elapsed_sec = started.elapsed().as_secs_f64().max(0.1);
        Ok((path, elapsed_sec, bytes))
    }
}

impl Drop for GsrBackend {
    fn drop(&mut self) {
        self.stop_replay();
        if self.session_child.is_some() {
            let _ = self.stop_session();
        }
    }
}
