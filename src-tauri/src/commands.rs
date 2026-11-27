//! Tauri IPC command handlers exposed to the Stage 6 frontend.

use crate::clipboard::copy_file_to_clipboard_cf_hdrop;
use crate::ffmpeg_editor::{
    capture_frame_png as capture_frame_png_impl, prepare_editor_media as prepare_editor_media_impl,
    replace_clip_with_advanced, replace_clip_with_fast_trim, run_advanced_export,
    run_discord_compress, run_fast_trim, PreparedEditorMedia,
};
use crate::state::{allow_directory_in_asset_scope, AppState};
use crate::webhook::send_file_to_discord_webhook;
use core_audio::{enumerate_capture_devices, enumerate_render_devices, list_active_audio_apps as list_active_audio_apps_impl};
use core_capture::{enumerate_monitors, enumerate_windows};
use core_types::{
    ActiveAudioAppInfo, AdvancedExportRequest, AppSettings, AudioDeviceInfo, CaptureTarget,
    ClipMetadata, DaemonStateSnapshot, DiscordCompressRequest, EditorCacheStats,
    FrameScreenshotRequest, GalleryFilter, MonitorInfo, RecorderCommand, SoundNotificationKind,
    TrimRequest, WindowInfo,
};
use recorder_daemon::{
    cleanup_temp_dir, list_clip_metadata, load_clip_metadata, measure_editor_cache,
    save_clip_metadata,
};
use std::fs;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use tauri::{AppHandle, Manager, State};

#[tauri::command]
pub fn get_daemon_state(state: State<'_, AppState>) -> DaemonStateSnapshot {
    state.state_snapshot()
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> AppSettings {
    state.current_settings()
}

#[tauri::command]
pub fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    mut settings: AppSettings,
) -> Result<AppSettings, String> {
    settings.normalize();
    crate::autostart::sync_windows_autostart(settings.launch_at_startup).map_err(|e| {
        tracing::error!("save_settings autostart sync failed: {e}");
        e
    })?;
    state.persist_settings(&settings).map_err(|e| {
        tracing::error!("save_settings persist_settings failed: {e}");
        e
    })?;
    allow_directory_in_asset_scope(&app, &settings.storage.output_dir);
    allow_directory_in_asset_scope(&app, &settings.storage.temp_dir);
    let lang = settings.language;
    tracing::info!(
        "save_settings: bitrate={} kbps, rate_control={:?}, codec={:?}, fps={}, gop_seconds={}",
        settings.video.bitrate_kbps,
        settings.video.rate_control,
        settings.video.codec,
        settings.video.fps,
        settings.video.gop_seconds
    );
    state
        .send_command(RecorderCommand::UpdateSettings(Box::new(settings.clone())))
        .map_err(|e| {
            tracing::error!("save_settings UpdateSettings failed: {e}");
            e
        })?;
    if let Some(tray) = app.try_state::<crate::tray::TrayController>() {
        tray.refresh_with_language(&app, &state.state_snapshot(), lang);
    }
    Ok(settings)
}

#[tauri::command]
pub fn allow_media_directory(app: AppHandle, dir: String) -> Result<(), String> {
    allow_directory_in_asset_scope(&app, Path::new(&dir));
    Ok(())
}

#[tauri::command]
pub fn list_monitors() -> Result<Vec<MonitorInfo>, String> {
    enumerate_monitors().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_windows() -> Result<Vec<WindowInfo>, String> {
    enumerate_windows().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_audio_output_devices() -> Result<Vec<AudioDeviceInfo>, String> {
    enumerate_render_devices().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_audio_input_devices() -> Result<Vec<AudioDeviceInfo>, String> {
    enumerate_capture_devices().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_active_audio_apps() -> Result<Vec<ActiveAudioAppInfo>, String> {
    list_active_audio_apps_impl().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn send_recorder_command(
    state: State<'_, AppState>,
    command: RecorderCommand,
) -> Result<(), String> {
    state.send_command(command)
}

#[tauri::command]
pub fn set_capture_target(
    state: State<'_, AppState>,
    target: CaptureTarget,
) -> Result<(), String> {
    let mut cfg = state.current_settings();
    cfg.capture.target = target.clone();
    let _ = state.persist_settings(&cfg);
    state.send_command(RecorderCommand::SetCaptureTarget(target))
}

#[tauri::command]
pub fn save_clip_now(state: State<'_, AppState>) -> Result<(), String> {
    state.send_command(RecorderCommand::SaveReplayClip)
}

#[tauri::command]
pub fn toggle_session_now(state: State<'_, AppState>) -> Result<(), String> {
    state.send_command(RecorderCommand::ToggleSession)
}

#[tauri::command]
pub fn add_bookmark_now(state: State<'_, AppState>, label: Option<String>) -> Result<(), String> {
    state.send_command(RecorderCommand::AddBookmark { label })
}

#[tauri::command]
pub fn toggle_mic_mute_now(state: State<'_, AppState>) -> Result<(), String> {
    state.send_command(RecorderCommand::ToggleMicMuted)
}

#[tauri::command]
pub fn toggle_privacy_mode_now(state: State<'_, AppState>) -> Result<(), String> {
    state.send_command(RecorderCommand::TogglePrivacyMode)
}

#[tauri::command]
pub fn play_test_sound(
    state: State<'_, AppState>,
    kind: SoundNotificationKind,
) -> Result<(), String> {
    state.send_command(RecorderCommand::PlayTestSound { kind })
}

#[tauri::command]
pub async fn list_clips(
    state: State<'_, AppState>,
    filter: Option<GalleryFilter>,
) -> Result<Vec<ClipMetadata>, String> {
    let dir = state.current_settings().storage.output_dir;
    tauri::async_runtime::spawn_blocking(move || {
        let mut items = list_clip_metadata(&dir);
        for meta in &mut items {
            if meta.duration_sec <= 0.0 || meta.width == 0 || meta.height == 0 {
                if let Ok(probed) = crate::ffmpeg_editor::probe_media_streams(&meta.file_path) {
                    meta.duration_sec = probed.duration_sec;
                    meta.width = probed.width;
                    meta.height = probed.height;
                    meta.video_codec = match probed.video_codec.to_ascii_lowercase().as_str() {
                        "hevc" | "h265" => core_types::VideoCodec::Hevc,
                        "av1" => core_types::VideoCodec::Av1,
                        _ => core_types::VideoCodec::H264,
                    };
                    meta.audio_tracks = probed
                        .audio_streams
                        .into_iter()
                        .map(|s| core_types::AudioTrackMetadata {
                            track_index: u8::try_from(s.ordinal + 1).unwrap_or(u8::MAX),
                            kind: s.track_kind,
                            label: s.label,
                            codec: s.codec_name,
                            sample_rate: s.sample_rate,
                            channels: s.channels,
                            default_volume: 1.0,
                            cached_audio_path: None,
                        })
                        .collect();
                    let _ = save_clip_metadata(meta);
                }
            }
        }
        let Some(f) = filter else {
            return items;
        };
        items
            .into_iter()
            .filter(|m| {
                if let Some(kind) = f.kind {
                    if m.kind != kind {
                        return false;
                    }
                }
                if f.favorites_only && !m.favorite {
                    return false;
                }
                if let Some(g) = &f.game_name {
                    if !m.game_name.to_ascii_lowercase().contains(&g.to_ascii_lowercase()) {
                        return false;
                    }
                }
                if let Some(q) = &f.search_query {
                    let ql = q.trim().to_ascii_lowercase();
                    if !ql.is_empty() {
                        let hay = format!("{} {} {}", m.title, m.game_name, m.tags.join(" "))
                            .to_ascii_lowercase();
                        if !hay.contains(&ql) {
                            return false;
                        }
                    }
                }
                true
            })
            .collect()
    })
    .await
    .map_err(|e| format!("Join error: {e}"))
}

#[tauri::command]
pub fn toggle_clip_favorite(video_path: String) -> Result<ClipMetadata, String> {
    let sidecar = ClipMetadata::sidecar_path_for(Path::new(&video_path));
    let mut meta = load_clip_metadata(&sidecar)?;
    meta.favorite = !meta.favorite;
    save_clip_metadata(&meta)?;
    Ok(meta)
}

#[tauri::command]
pub fn rename_clip(video_path: String, new_title: String) -> Result<ClipMetadata, String> {
    let sidecar = ClipMetadata::sidecar_path_for(Path::new(&video_path));
    let mut meta = load_clip_metadata(&sidecar)?;
    let trimmed = new_title.trim();
    if trimmed.is_empty() {
        return Err("Title cannot be empty".to_string());
    }
    meta.title = trimmed.to_string();
    save_clip_metadata(&meta)?;
    Ok(meta)
}

#[tauri::command]
pub fn delete_clip(video_path: String) -> Result<(), String> {
    let mp4 = PathBuf::from(&video_path);
    let sidecar = ClipMetadata::sidecar_path_for(&mp4);
    let legacy_sidecar = ClipMetadata::legacy_sidecar_path_for(&mp4);
    if let Ok(meta) = load_clip_metadata(&sidecar).or_else(|_| load_clip_metadata(&legacy_sidecar))
    {
        if let Some(t) = meta.thumbnail_path {
            let _ = fs::remove_file(t);
        }
        if let Some(w) = meta.waveform_path {
            let _ = fs::remove_file(w);
        }
    }
    for path in [
        sidecar,
        legacy_sidecar,
        ClipMetadata::thumbnail_path_for(&mp4),
        ClipMetadata::legacy_thumbnail_path_for(&mp4),
    ] {
        if path.exists() {
            let _ = fs::remove_file(path);
        }
    }
    if mp4.exists() {
        fs::remove_file(&mp4).map_err(|e| format!("Failed to delete '{}': {e}", mp4.display()))?;
    }
    Ok(())
}

#[tauri::command]
pub fn run_storage_cleanup_now(state: State<'_, AppState>) -> Result<(usize, u64), String> {
    state.run_storage_cleanup_now()
}

#[tauri::command]
pub async fn prepare_editor_media(
    app: AppHandle,
    state: State<'_, AppState>,
    video_path: String,
    buckets: Option<usize>,
) -> Result<PreparedEditorMedia, String> {
    let cache_dir = state.current_settings().storage.temp_dir.join("editor_cache");
    allow_directory_in_asset_scope(&app, &cache_dir);
    let path = PathBuf::from(video_path);
    let b = buckets.unwrap_or(140);
    tauri::async_runtime::spawn_blocking(move || prepare_editor_media_impl(&path, &cache_dir, b))
        .await
        .map_err(|e| format!("Join error: {e}"))?
}

#[tauri::command]
pub async fn capture_frame_png(req: FrameScreenshotRequest) -> Result<PathBuf, String> {
    tauri::async_runtime::spawn_blocking(move || capture_frame_png_impl(&req))
        .await
        .map_err(|e| format!("Join error: {e}"))?
}

#[tauri::command]
pub async fn fast_trim_clip(req: TrimRequest) -> Result<PathBuf, String> {
    tauri::async_runtime::spawn_blocking(move || run_fast_trim(&req))
        .await
        .map_err(|e| format!("Join error: {e}"))?
}

#[tauri::command]
pub async fn compress_clip_for_discord(
    req: DiscordCompressRequest,
    duration_sec: f64,
) -> Result<PathBuf, String> {
    tauri::async_runtime::spawn_blocking(move || run_discord_compress(&req, duration_sec))
        .await
        .map_err(|e| format!("Join error: {e}"))?
}

#[tauri::command]
pub async fn export_advanced_clip(req: AdvancedExportRequest) -> Result<PathBuf, String> {
    tauri::async_runtime::spawn_blocking(move || run_advanced_export(&req))
        .await
        .map_err(|e| format!("Join error: {e}"))?
}

#[tauri::command]
pub async fn replace_clip_fast_trim(
    state: State<'_, AppState>,
    req: TrimRequest,
) -> Result<PathBuf, String> {
    let cache_dir = state.current_settings().storage.temp_dir.join("editor_cache");
    tauri::async_runtime::spawn_blocking(move || replace_clip_with_fast_trim(&req, &cache_dir))
        .await
        .map_err(|e| format!("Join error: {e}"))?
}

#[tauri::command]
pub async fn replace_clip_advanced(
    state: State<'_, AppState>,
    req: AdvancedExportRequest,
) -> Result<PathBuf, String> {
    let cache_dir = state.current_settings().storage.temp_dir.join("editor_cache");
    tauri::async_runtime::spawn_blocking(move || replace_clip_with_advanced(&req, &cache_dir))
        .await
        .map_err(|e| format!("Join error: {e}"))?
}

#[tauri::command]
pub fn copy_clip_to_clipboard(path: String) -> Result<(), String> {
    copy_file_to_clipboard_cf_hdrop(Path::new(&path))
}

#[tauri::command]
pub async fn send_to_discord_webhook(
    webhook_url: String,
    file_path: String,
    content: Option<String>,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        send_file_to_discord_webhook(&webhook_url, Path::new(&file_path), content.as_deref())
    })
    .await
    .map_err(|e| format!("Join error: {e}"))?
}

#[tauri::command]
pub fn open_clips_folder(state: State<'_, AppState>) -> Result<(), String> {
    let dir = state.current_settings().storage.output_dir;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Command::new("explorer.exe")
        .creation_flags(0x0800_0000)
        .arg(&dir)
        .spawn()
        .map_err(|e| format!("Failed to open Explorer: {e}"))?;
    Ok(())
}

#[tauri::command]
pub fn reveal_clip_in_explorer(path: String) -> Result<(), String> {
    let target = PathBuf::from(&path);
    if !target.exists() {
        return Err(format!("Файл '{}' не найден", target.display()));
    }
    let resolved = fs::canonicalize(&target).unwrap_or(target);
    let mut display = resolved.to_string_lossy().to_string();
    if let Some(stripped) = display.strip_prefix(r"\\?\") {
        display = stripped.to_string();
    }
    let escaped = display.replace('"', "");
    Command::new("explorer.exe")
        .creation_flags(0x0800_0000)
        .raw_arg(format!("/select,\"{escaped}\""))
        .spawn()
        .map_err(|e| format!("Failed to launch Explorer: {e}"))?;
    Ok(())
}

#[tauri::command]
pub fn get_editor_cache_stats(state: State<'_, AppState>) -> EditorCacheStats {
    let temp_dir = state.current_settings().storage.temp_dir;
    measure_editor_cache(&temp_dir)
}

#[tauri::command]
pub fn clear_editor_cache_now(state: State<'_, AppState>) -> (usize, u64) {
    let temp_dir = state.current_settings().storage.temp_dir;
    cleanup_temp_dir(&temp_dir)
}

#[tauri::command]
pub async fn pick_audio_file() -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let script = r#"
Add-Type -AssemblyName System.Windows.Forms
$dlg = New-Object System.Windows.Forms.OpenFileDialog
$dlg.Title = 'Select Notification Sound'
$dlg.Filter = 'Audio Files (*.wav;*.mp3;*.ogg;*.flac)|*.wav;*.mp3;*.ogg;*.flac|All Files (*.*)|*.*'
$dlg.Multiselect = $false
if ($dlg.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {
    [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
    Write-Output $dlg.FileName
}
"#;
        let out = Command::new("powershell")
            .creation_flags(0x0800_0000)
            .args(["-NoProfile", "-STA", "-Command", script])
            .output()
            .map_err(|e| format!("Failed to open file dialog: {e}"))?;
        if !out.status.success() {
            return Ok(None);
        }
        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if path.is_empty() {
            Ok(None)
        } else {
            Ok(Some(path))
        }
    })
    .await
    .map_err(|e| format!("Join error: {e}"))?
}

#[tauri::command]
pub async fn get_supported_codecs() -> Result<Vec<core_types::VideoCodec>, String> {
    tauri::async_runtime::spawn_blocking(core_encode::detect_available_hardware_codecs)
        .await
        .map_err(|e| format!("Join error: {e}"))
}

#[tauri::command]
pub async fn check_for_updates() -> Result<crate::updater::UpdateInfo, String> {
    tauri::async_runtime::spawn_blocking(|| {
        crate::updater::check_for_updates(env!("CARGO_PKG_VERSION"))
    })
    .await
    .map_err(|e| format!("Join error: {e}"))?
}

#[tauri::command]
pub async fn download_and_install_update(
    app: AppHandle,
    download_url: String,
    installer_name: Option<String>,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::updater::download_and_install_update(
            &app,
            &download_url,
            installer_name.as_deref(),
        )
    })
    .await
    .map_err(|e| format!("Join error: {e}"))?
}
