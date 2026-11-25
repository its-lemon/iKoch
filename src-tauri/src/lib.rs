//! Stage 5 Tauri v2 Desktop Host:
//! - Single-instance enforcement (`tauri-plugin-single-instance`)
//! - Zero-RAM tray background lifecycle (main WebView2 destroyed cleanly on `[X]`, rebuilt on demand)
//! - Dynamic Win32 tray status icons & context menu (`tray-icon`)
//! - HTTP Range streaming (`asset://`) scoped dynamically to `output_dir` / `temp_dir`
//! - Click-through overlay Toast window, FFmpeg/FFprobe sidecar, Win32 `CF_HDROP` clipboard, Discord webhook

pub mod autostart;
pub mod clipboard;
pub mod commands;
pub mod ffmpeg_editor;
pub mod state;
pub mod tray;
pub mod updater;
pub mod webhook;

pub use core_types::{AppSettings, ClipMetadata, DaemonStateSnapshot, RecorderEvent};
pub use ffmpeg_editor::{
    capture_frame_png, prepare_editor_media, run_advanced_export, run_discord_compress,
    run_fast_trim, PreparedEditorMedia,
};

use core_types::RecorderCommand;
use crossbeam_channel::Receiver;
use recorder_daemon::{load_clip_metadata, save_clip_metadata, RecorderDaemon};
use state::{
    allow_directory_in_asset_scope, load_or_init_settings, open_or_focus_main_window,
    resolve_settings_path, show_toast_overlay, AppState,
};
use std::path::Path;
use std::thread;
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};
use tracing::{info, warn};
use tray::TrayController;

struct TeeLogWriter {
    file: Option<std::sync::Arc<parking_lot::Mutex<std::fs::File>>>,
}

impl std::io::Write for TeeLogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = std::io::stderr().write(buf);
        if let Some(f) = &self.file {
            let _ = f.lock().write_all(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let _ = std::io::stderr().flush();
        if let Some(f) = &self.file {
            let _ = f.lock().flush();
        }
        Ok(())
    }
}

/// Starts the Tauri v2 desktop application with background recording daemon and system tray.
pub fn run() {
    let logs_dir = core_types::ikoch_logs_dir();
    let _ = std::fs::create_dir_all(&logs_dir);
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(logs_dir.join("ikoch.log"))
        .ok()
        .map(|f| std::sync::Arc::new(parking_lot::Mutex::new(f)));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_ansi(false)
        .with_writer(move || TeeLogWriter {
            file: log_file.clone(),
        })
        .try_init();

    tracing::info!(
        "Starting iKoch v{} from '{}'",
        env!("CARGO_PKG_VERSION"),
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "<unknown>".to_string())
    );

    let start_minimized = std::env::args().any(|a| a == "--minimized" || a == "--background");

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            open_or_focus_main_window(app, None);
        }))
        .invoke_handler(tauri::generate_handler![
            commands::get_daemon_state,
            commands::get_settings,
            commands::save_settings,
            commands::allow_media_directory,
            commands::list_monitors,
            commands::list_windows,
            commands::list_audio_output_devices,
            commands::list_audio_input_devices,
            commands::list_active_audio_apps,
            commands::send_recorder_command,
            commands::set_capture_target,
            commands::save_clip_now,
            commands::toggle_session_now,
            commands::add_bookmark_now,
            commands::toggle_mic_mute_now,
            commands::toggle_privacy_mode_now,
            commands::play_test_sound,
            commands::list_clips,
            commands::toggle_clip_favorite,
            commands::rename_clip,
            commands::delete_clip,
            commands::run_storage_cleanup_now,
            commands::prepare_editor_media,
            commands::capture_frame_png,
            commands::fast_trim_clip,
            commands::replace_clip_fast_trim,
            commands::compress_clip_for_discord,
            commands::export_advanced_clip,
            commands::replace_clip_advanced,
            commands::copy_clip_to_clipboard,
            commands::send_to_discord_webhook,
            commands::open_clips_folder,
            commands::reveal_clip_in_explorer,
            commands::get_editor_cache_stats,
            commands::clear_editor_cache_now,
            commands::pick_audio_file,
            commands::check_for_updates,
            commands::download_and_install_update,
        ])
        .on_window_event(|window, event| {
            // Zero-RAM background design: destroy the main WebView2 process completely when closed.
            if window.label() == "main" {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.destroy();
                    info!("Main WebView2 window destroyed; daemon continues headless in tray");
                    if let Some(st) = window.app_handle().try_state::<AppState>() {
                        let storage = st.current_settings().storage;
                        if storage.editor_cache.mode == core_types::CacheCleanupMode::OnClose {
                            thread::spawn(move || {
                                // Brief pause so WebView2 releases any open .m4a handles before cleanup.
                                thread::sleep(std::time::Duration::from_millis(350));
                                recorder_daemon::cleanup_temp_dir(&storage.temp_dir);
                            });
                        }
                    }
                }
            }
        })
        .setup(move |app| {
            let settings_path = resolve_settings_path();
            let settings = load_or_init_settings(&settings_path);
            let output_dir = settings.storage.output_dir.clone();
            let temp_dir = settings.storage.temp_dir.clone();

            allow_directory_in_asset_scope(app.handle(), &output_dir);
            allow_directory_in_asset_scope(app.handle(), &temp_dir);
            allow_directory_in_asset_scope(app.handle(), &core_types::ikoch_cache_root_dir());

            if settings.launch_at_startup {
                if let Err(err) = autostart::sync_windows_autostart(true) {
                    warn!("Failed to refresh Windows autostart registry entry: {err}");
                }
            }

            let (daemon, evt_rx) = RecorderDaemon::start(settings.clone())
                .map_err(|e| format!("Failed to start RecorderDaemon: {e}"))?;
            let state = AppState::new(daemon, settings_path);
            if settings.replay_buffer.enabled {
                let _ = state.send_command(RecorderCommand::StartBuffering);
            }
            app.manage(state);

            let tray = TrayController::setup(app.handle())?;
            app.manage(tray.clone());

            spawn_event_dispatcher(app.handle().clone(), tray, evt_rx);

            if !start_minimized {
                open_or_focus_main_window(app.handle(), None);
            }
            Ok(())
        });

    let app = match builder.build(tauri::generate_context!()) {
        Ok(a) => a,
        Err(err) => {
            warn!("Tauri build failed: {err}");
            return;
        }
    };

    app.run(|app_handle, event| {
        match event {
            RunEvent::ExitRequested { api, .. } => {
                if let Some(st) = app_handle.try_state::<AppState>() {
                    if !st.is_quitting() {
                        // Keep headless event loop alive when all WebView2 windows are destroyed.
                        api.prevent_exit();
                    }
                }
            }
            RunEvent::Exit => {
                if let Some(st) = app_handle.try_state::<AppState>() {
                    st.shutdown_daemon();
                }
            }
            _ => {}
        }
    });
}

fn spawn_event_dispatcher(
    app: AppHandle,
    tray: TrayController,
    evt_rx: Receiver<RecorderEvent>,
) {
    thread::Builder::new()
        .name("ikoch-tauri-events".to_string())
        .spawn(move || {
            let mut last_tray_sig: Option<(String, bool, bool, bool, bool, String)> = None;

            while let Ok(evt) = evt_rx.recv() {
                if let Some(_win) = app.get_webview_window("main") {
                    let _ = app.emit_to("main", "ikoch://event", &evt);
                }

                match &evt {
                    RecorderEvent::StateChanged(snap) => {
                        let lang = app
                            .try_state::<AppState>()
                            .map_or(core_types::AppLanguage::Ru, |s| s.current_settings().language);
                        let sig = (
                            format!("{:?}/{:?}", snap.tray_state, snap.capture_target),
                            snap.privacy_mode,
                            snap.mic_muted,
                            snap.active_session.is_some(),
                            snap.active_game.is_some(),
                            lang.as_str().to_string(),
                        );
                        if last_tray_sig.as_ref() != Some(&sig) {
                            last_tray_sig = Some(sig);
                            tray.refresh_with_language(&app, snap, lang);
                        }
                    }
                    RecorderEvent::ClipSaved(meta) => {
                        maybe_generate_background_thumbnail(&meta.file_path);
                        let st = app.state::<AppState>();
                        let cfg = st.current_settings();
                        if cfg.notifications.overlay_toast_enabled {
                            let is_en = cfg.language == core_types::AppLanguage::En;
                            show_toast_overlay(
                                &app,
                                if is_en {
                                    format!("Clip Saved ({:.1}s)", meta.duration_sec)
                                } else {
                                    format!("Клип сохранён ({:.1} сек)", meta.duration_sec)
                                },
                                meta.title.clone(),
                                cfg.notifications.overlay_duration_ms,
                            );
                        }
                    }
                    RecorderEvent::SessionStarted(_info) => {
                        let st = app.state::<AppState>();
                        let cfg = st.current_settings();
                        if cfg.notifications.overlay_toast_enabled {
                            let is_en = cfg.language == core_types::AppLanguage::En;
                            show_toast_overlay(
                                &app,
                                if is_en {
                                    "Recording Started".to_string()
                                } else {
                                    "Запись начата".to_string()
                                },
                                if is_en {
                                    format!("Press {} to stop recording", cfg.hotkeys.toggle_session)
                                } else {
                                    format!("Нажмите {} для остановки записи", cfg.hotkeys.toggle_session)
                                },
                                cfg.notifications.overlay_duration_ms,
                            );
                        }
                    }
                    RecorderEvent::SessionStopped(meta) => {
                        maybe_generate_background_thumbnail(&meta.file_path);
                        let st = app.state::<AppState>();
                        let cfg = st.current_settings();
                        if cfg.notifications.overlay_toast_enabled {
                            let is_en = cfg.language == core_types::AppLanguage::En;
                            show_toast_overlay(
                                &app,
                                if is_en {
                                    format!("Recording Saved ({:.1}s)", meta.duration_sec)
                                } else {
                                    format!("Запись сохранена ({:.1} сек)", meta.duration_sec)
                                },
                                meta.title.clone(),
                                cfg.notifications.overlay_duration_ms,
                            );
                        }
                    }
                    RecorderEvent::ShowOverlayToast {
                        title,
                        message,
                        ..
                    } => {
                        let st = app.state::<AppState>();
                        let notif = st.current_settings().notifications;
                        if notif.overlay_toast_enabled {
                            show_toast_overlay(
                                &app,
                                title.clone(),
                                message.clone(),
                                notif.overlay_duration_ms,
                            );
                        }
                    }
                    _ => {}
                }
            }
        })
        .ok();
}

fn maybe_generate_background_thumbnail(video_path: &Path) {
    let mp4 = video_path.to_path_buf();
    let _ = thread::Builder::new()
        .name("ikoch-thumb".to_string())
        .spawn(move || {
            let thumb_path = core_types::ClipMetadata::thumbnail_path_for(&mp4);
            if ffmpeg_editor::generate_thumbnail_jpg(&mp4, &thumb_path, 0.5).is_ok() {
                let sidecar = core_types::ClipMetadata::sidecar_path_for(&mp4);
                if let Ok(mut meta) = load_clip_metadata(&sidecar) {
                    meta.thumbnail_path = Some(thumb_path);
                    let _ = save_clip_metadata(&meta);
                }
            }
        });
}
