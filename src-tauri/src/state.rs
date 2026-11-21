//! Shared Tauri application state, persistent settings storage, `asset://` directory
//! authorization, lazy `"main"` window lifecycle, and click-through Toast overlay spawning.

use core_types::{AppSettings, DaemonStateSnapshot, RecorderCommand};
use crossbeam_channel::Sender;
use parking_lot::Mutex;
use recorder_daemon::{RecorderDaemon, StorageQuotaManager};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;
use tauri::{
    AppHandle, Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder,
};
use tracing::{info, warn};

/// Global application state managed by Tauri across background tray & UI windows.
pub struct AppState {
    daemon: Mutex<Option<RecorderDaemon>>,
    cmd_tx: Sender<RecorderCommand>,
    settings_path: PathBuf,
    quitting: AtomicBool,
}

impl AppState {
    /// Creates a new [`AppState`] wrapping a running [`RecorderDaemon`].
    #[must_use]
    pub fn new(daemon: RecorderDaemon, settings_path: PathBuf) -> Self {
        let cmd_tx = daemon.command_sender();
        Self {
            daemon: Mutex::new(Some(daemon)),
            cmd_tx,
            settings_path,
            quitting: AtomicBool::new(false),
        }
    }

    /// Dispatches a command directly to `recorder-daemon`.
    pub fn send_command(&self, cmd: RecorderCommand) -> Result<(), String> {
        self.cmd_tx
            .send(cmd)
            .map_err(|_| "Recorder daemon command channel closed".to_string())
    }

    /// Returns a fresh [`DaemonStateSnapshot`], or default if shutting down.
    #[must_use]
    pub fn state_snapshot(&self) -> DaemonStateSnapshot {
        self.daemon
            .lock()
            .as_ref()
            .map_or_else(DaemonStateSnapshot::default, RecorderDaemon::state_snapshot)
    }

    /// Returns active normalized settings from the daemon.
    #[must_use]
    pub fn current_settings(&self) -> AppSettings {
        self.daemon
            .lock()
            .as_ref()
            .map_or_else(AppSettings::default, RecorderDaemon::settings)
    }

    /// Persists `settings` atomically to `%APPDATA%/iKoch/settings.json`.
    pub fn persist_settings(&self, settings: &AppSettings) -> Result<(), String> {
        if let Some(parent) = self.settings_path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create settings dir failed: {e}"))?;
        }
        let json = serde_json::to_string_pretty(settings)
            .map_err(|e| format!("Serialize settings failed: {e}"))?;
        let tmp = self.settings_path.with_extension("json.tmp");
        fs::write(&tmp, json.as_bytes()).map_err(|e| format!("Write settings tmp failed: {e}"))?;
        fs::rename(&tmp, &self.settings_path)
            .map_err(|e| format!("Rename settings file failed: {e}"))?;
        Ok(())
    }

    /// Marks intentional app exit so `RunEvent::ExitRequested` permits termination.
    pub fn mark_quitting(&self) {
        self.quitting.store(true, Ordering::SeqCst);
    }

    #[must_use]
    pub fn is_quitting(&self) -> bool {
        self.quitting.load(Ordering::SeqCst)
    }

    /// Cleanly stops `RecorderDaemon` when exiting via tray Quit and applies temporary cache policy.
    pub fn shutdown_daemon(&self) {
        let daemon_opt = self.daemon.lock().take();
        if let Some(daemon) = daemon_opt {
            let storage = daemon.settings().storage.clone();
            daemon.shutdown();
            recorder_daemon::apply_temp_cache_policy(&storage);
        }
    }

    /// Enforces disk quota immediately inside the configured output directory.
    pub fn run_storage_cleanup_now(&self) -> Result<(usize, u64), String> {
        let cfg = self.current_settings();
        let report = StorageQuotaManager::enforce_quota(
            &cfg.storage.output_dir,
            cfg.storage.max_storage_gb,
            cfg.storage.max_age_days,
        )?;
        Ok((report.deleted_files, report.freed_bytes))
    }
}

/// Resolves the canonical path for persisted user settings (`%APPDATA%/iKoch/settings.json` on Windows,
/// `~/.config/iKoch/settings.json` on Linux).
#[must_use]
pub fn resolve_settings_path() -> PathBuf {
    if let Ok(appdata) = std::env::var("APPDATA") {
        PathBuf::from(appdata).join("iKoch").join("settings.json")
    } else if let Ok(xdg_cfg) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(xdg_cfg).join("iKoch").join("settings.json")
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home)
            .join(".config")
            .join("iKoch")
            .join("settings.json")
    } else {
        PathBuf::from("iKoch").join("settings.json")
    }
}

/// Resolves legacy Ferrix / RustClip settings paths in priority order for seamless migration.
#[must_use]
pub fn resolve_legacy_settings_paths() -> Vec<PathBuf> {
    let base = std::env::var("APPDATA").ok().map(PathBuf::from);
    ["Ferrix", "RustClip"]
        .into_iter()
        .map(|folder| match &base {
            Some(appdata) => appdata.join(folder).join("settings.json"),
            None => PathBuf::from(folder).join("settings.json"),
        })
        .collect()
}

/// Loads persisted settings (or migrates legacy Ferrix / RustClip `settings.json`), falling back
/// to defaults rooted in `%USERPROFILE%/Videos/iKoch` and `%LOCALAPPDATA%/RustClip/cache/tmp`.
pub fn load_or_init_settings(settings_path: &Path) -> AppSettings {
    let legacy_path = resolve_legacy_settings_paths()
        .into_iter()
        .find(|p| p.exists());
    let migrated_from_legacy = !settings_path.exists() && legacy_path.is_some();
    let had_existing = settings_path.exists() || migrated_from_legacy;

    let read_source = if settings_path.exists() {
        Some(settings_path.to_path_buf())
    } else if let Some(legacy) = legacy_path {
        info!(
            "Migrating legacy settings from '{}' to '{}'",
            legacy.display(),
            settings_path.display()
        );
        Some(legacy)
    } else {
        None
    };

    let mut settings = if let Some(src) = read_source {
        fs::read_to_string(src)
            .ok()
            .and_then(|raw| serde_json::from_str::<AppSettings>(&raw).ok())
            .unwrap_or_default()
    } else {
        AppSettings::default()
    };

    // Resolve default relative output_dir to user's Videos folder so desktop shortcuts & Program Files launches work.
    if settings.storage.output_dir.is_relative() {
        let base = std::env::var("USERPROFILE")
            .ok()
            .map(|home| PathBuf::from(home).join("Videos"))
            .or_else(|| {
                std::env::var("XDG_VIDEOS_DIR")
                    .ok()
                    .filter(|s| !s.trim().is_empty())
                    .map(PathBuf::from)
            })
            .or_else(|| {
                std::env::var("HOME")
                    .ok()
                    .map(|home| PathBuf::from(home).join("Videos"))
            })
            .unwrap_or_else(|| PathBuf::from("."));
        settings.storage.output_dir = base.join(&settings.storage.output_dir);
    }

    // Migrate any relative or in-output-folder `.tmp` directory into `%LOCALAPPDATA%\RustClip\cache\tmp`.
    let out_norm = settings
        .storage
        .output_dir
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase();
    let tmp_norm = settings
        .storage
        .temp_dir
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase();
    let tmp_inside_output =
        tmp_norm == format!("{out_norm}\\.tmp") || tmp_norm.starts_with(&format!("{out_norm}\\.tmp\\"));

    let mut migrated_temp = false;
    if settings.storage.temp_dir.is_relative() || tmp_inside_output {
        let old_tmp = settings.storage.output_dir.join(".tmp");
        if old_tmp.exists() {
            let _ = fs::remove_dir_all(&old_tmp);
        }
        settings.storage.temp_dir = core_types::ikoch_temp_dir();
        migrated_temp = true;
    }

    settings.normalize();

    if had_existing && (migrated_temp || migrated_from_legacy) {
        if let Some(parent) = settings_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&settings) {
            let _ = fs::write(settings_path, json.as_bytes());
        }
    }

    settings
}

/// Grants Tauri's `asset://` protocol read+range access to `dir` (recursively) so `<video>` seeks
/// arbitrary file sizes smoothly with HTTP Range without copying files into memory.
pub fn allow_directory_in_asset_scope(app: &AppHandle, dir: &Path) {
    if let Err(err) = fs::create_dir_all(dir) {
        warn!("Failed to ensure media directory '{}': {err}", dir.display());
        return;
    }
    if let Err(err) = app.asset_protocol_scope().allow_directory(dir, true) {
        warn!(
            "Failed to allow directory '{}' in asset:// scope: {err}",
            dir.display()
        );
    } else {
        info!("Authorized asset:// directory scope -> {}", dir.display());
    }
}

/// Lazily recreates or focuses the `"main"` WebView2 window without blocking the calling UI/tray thread.
pub fn open_or_focus_main_window(app: &AppHandle, route_hash: Option<&str>) {
    if let Some(existing) = app.get_webview_window("main") {
        let _ = existing.unminimize();
        let _ = existing.show();
        let _ = existing.set_focus();
        if let Some(route) = route_hash {
            let clean = route.replace('\'', "");
            let _ = existing.eval(format!("window.location.hash = '{clean}';"));
        }
        return;
    }

    let handle = app.clone();
    let target_url = match route_hash {
        Some(hash) if !hash.is_empty() => {
            let h = hash.trim_start_matches('#');
            WebviewUrl::App(format!("index.html#{h}").into())
        }
        _ => WebviewUrl::App("index.html".into()),
    };

    // Build WebviewWindow on a separate thread to avoid Windows WebView2 main-thread synchronous deadlocks.
    thread::spawn(move || {
        match WebviewWindowBuilder::new(&handle, "main", target_url)
            .title("iKoch")
            .decorations(false)
            .inner_size(1240.0, 780.0)
            .min_inner_size(960.0, 620.0)
            .center()
            .build()
        {
            Ok(win) => {
                let _ = win.show();
                let _ = win.set_focus();
            }
            Err(err) => warn!("Failed to lazily create main WebView2 window: {err}"),
        }
    });
}

/// Displays a transparent, always-on-top, click-through Toast notification for `duration_ms` ms.
pub fn show_toast_overlay(
    app: &AppHandle,
    title: String,
    message: String,
    duration_ms: u32,
) {
    let handle = app.clone();
    let wait_ms = duration_ms.clamp(500, 10_000);
    let cfg = app
        .try_state::<AppState>()
        .map(|s| s.current_settings())
        .unwrap_or_default();
    let theme_slug = cfg.theme.as_str().to_string();
    let lang_slug = cfg.language.as_str().to_string();
    let custom_theme = cfg.custom_theme;

    thread::spawn(move || {
        if let Some(old) = handle.get_webview_window("toast") {
            let _ = old.destroy();
        }

        let toast_w = 370u32;
        let toast_h = 98u32;
        let margin = 20i32;
        let (x, y) = if let Ok(Some(mon)) = handle.primary_monitor() {
            let sz = mon.size();
            let pos = mon.position();
            (
                pos.x + (sz.width as i32) - (toast_w as i32) - margin,
                pos.y + (sz.height as i32) - (toast_h as i32) - 64,
            )
        } else {
            (40, 40)
        };

        let mut url_str = format!(
            "toast.html?title={}&message={}&theme={}&lang={}",
            percent_encode_query(&title),
            percent_encode_query(&message),
            percent_encode_query(&theme_slug),
            percent_encode_query(&lang_slug)
        );
        if theme_slug == "custom" {
            url_str.push_str(&format!(
                "&cbg={}&cpanel={}&caccent={}&ctext={}",
                percent_encode_query(&custom_theme.bg_app),
                percent_encode_query(&custom_theme.bg_panel),
                percent_encode_query(&custom_theme.accent),
                percent_encode_query(&custom_theme.text_primary),
            ));
        }
        let builder = WebviewWindowBuilder::new(&handle, "toast", WebviewUrl::App(url_str.into()))
            .title("iKoch Notification")
            .inner_size(f64::from(toast_w), f64::from(toast_h))
            .transparent(true)
            .decorations(false)
            .shadow(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .focused(false)
            .resizable(false);

        if let Ok(win) = builder.build() {
            let _ = win.set_position(PhysicalPosition::new(x, y));
            let _ = win.set_ignore_cursor_events(true);
            let _ = win.show();

            thread::sleep(Duration::from_millis(u64::from(wait_ms)));
            if let Some(still_there) = handle.get_webview_window("toast") {
                let _ = still_there.destroy();
            }
        }
    });
}

fn percent_encode_query(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 2);
    for b in input.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char);
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
