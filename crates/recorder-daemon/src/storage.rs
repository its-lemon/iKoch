//! Sidecar `.json` persistence (`ClipMetadata`), system-cache migration,
//! `.tmp` cleanup, and Storage Quota cleanup manager.
//!
//! Stores `.json` sidecars and `.jpg` thumbnails inside `%LOCALAPPDATA%\iKoch\cache`
//! so `output_dir` contains only clean `.mp4` recordings and game subfolders.
//! Protects `favorite` and `is_edited` recordings when enforcing disk capacity (`max_storage_gb`)
//! or retention age (`max_age_days`).

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use core_types::{
    ikoch_sidecars_dir, ikoch_thumbnails_dir, CacheCleanupMode, ClipMetadata, EditorCacheStats,
    StorageSettings,
};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tracing::{debug, info, warn};

/// Summary of an automated storage quota / retention cleanup pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StorageCleanupReport {
    pub deleted_recordings: usize,
    pub deleted_files: usize,
    pub freed_bytes: u64,
    pub remaining_bytes: u64,
}

/// Deletes all temporary files and subdirectories inside `temp_dir` (re-creating `temp_dir`),
/// plus any stale `*.json.tmp` staging files in `ikoch_sidecars_dir()`.
/// Returns `(deleted_file_count, freed_bytes)`.
pub fn cleanup_temp_dir(temp_dir: &Path) -> (usize, u64) {
    let mut removed_files = 0usize;
    let mut freed_bytes = 0u64;

    if temp_dir.exists() {
        let (n, b) = clear_directory_contents(temp_dir);
        removed_files += n;
        freed_bytes = freed_bytes.saturating_add(b);
    }
    let _ = fs::create_dir_all(temp_dir);

    let sidecars = ikoch_sidecars_dir();
    if let Ok(entries) = fs::read_dir(&sidecars) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.to_ascii_lowercase().ends_with(".json.tmp"))
            {
                let sz = file_len_or_zero(&p);
                if fs::remove_file(&p).is_ok() {
                    removed_files += 1;
                    freed_bytes = freed_bytes.saturating_add(sz);
                }
            }
        }
    }

    if removed_files > 0 {
        info!(
            "Cleaned up {} temporary file(s) ({:.2} MiB freed) from '{}'",
            removed_files,
            (freed_bytes as f64) / (1024.0 * 1024.0),
            temp_dir.display()
        );
    }
    (removed_files, freed_bytes)
}

/// Applies the user-selected automatic temporary cache cleanup policy (`OnClose`, `TimerAndLimit`,
/// or `Manual`). Never removes persistent `.json` sidecars or `.jpg` thumbnails.
pub fn apply_temp_cache_policy(storage: &StorageSettings) -> (usize, u64) {
    match storage.editor_cache.mode {
        CacheCleanupMode::OnClose => cleanup_temp_dir(&storage.temp_dir),
        CacheCleanupMode::TimerAndLimit => {
            let max_bytes = storage.editor_cache.max_size_mb.saturating_mul(1024 * 1024);
            prune_temp_cache_to_limit(&storage.temp_dir, max_bytes)
        }
        CacheCleanupMode::Manual => (0, 0),
    }
}

fn clear_directory_contents(dir: &Path) -> (usize, u64) {
    let Ok(entries) = fs::read_dir(dir) else {
        return (0, 0);
    };
    let mut count = 0usize;
    let mut freed = 0u64;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let (n, b) = clear_directory_contents(&path);
            count += n;
            freed = freed.saturating_add(b);
            let _ = fs::remove_dir(&path);
        } else {
            let sz = file_len_or_zero(&path);
            if fs::remove_file(&path).is_ok() {
                count += 1;
                freed = freed.saturating_add(sz);
            }
        }
    }
    (count, freed)
}

/// Measures disk usage separately for heavy temporary editor stems (`.m4a` / `.tmp` in `temp_dir`)
/// and persistent lightweight `.json` sidecars / `.jpg` thumbnails.
#[must_use]
pub fn measure_editor_cache(temp_dir: &Path) -> EditorCacheStats {
    let mut temp_files = Vec::new();
    collect_files_with_mtime(temp_dir, &mut temp_files);
    let temp_file_count = temp_files.len();
    let temp_bytes: u64 = temp_files
        .iter()
        .fold(0u64, |acc, (_, sz, _)| acc.saturating_add(*sz));

    let mut meta_files = Vec::new();
    collect_files_with_mtime(&ikoch_sidecars_dir(), &mut meta_files);
    collect_files_with_mtime(&ikoch_thumbnails_dir(), &mut meta_files);
    let metadata_file_count = meta_files.len();
    let metadata_bytes: u64 = meta_files
        .iter()
        .fold(0u64, |acc, (_, sz, _)| acc.saturating_add(*sz));

    EditorCacheStats {
        temp_bytes,
        temp_file_count,
        metadata_bytes,
        metadata_file_count,
    }
}

/// Deletes the oldest heavy temporary files (e.g., extracted `.m4a` stems / `.tmp`) inside
/// `temp_dir` until total size is `<= max_bytes`. Never modifies `.json` sidecars or `.jpg` thumbnails.
pub fn prune_temp_cache_to_limit(temp_dir: &Path, max_bytes: u64) -> (usize, u64) {
    if !temp_dir.exists() {
        return (0, 0);
    }

    let mut files = Vec::new();
    collect_files_with_mtime(temp_dir, &mut files);
    let mut total_bytes: u64 = files
        .iter()
        .fold(0u64, |acc, (_, sz, _)| acc.saturating_add(*sz));
    if total_bytes <= max_bytes {
        return (0, 0);
    }

    // Oldest modified files evicted first.
    files.sort_by_key(|(path, _, mtime)| (*mtime, path.clone()));

    let mut removed_files = 0usize;
    let mut freed_bytes = 0u64;
    for (path, size, _) in files {
        if total_bytes <= max_bytes {
            break;
        }
        if fs::remove_file(&path).is_ok() {
            total_bytes = total_bytes.saturating_sub(size);
            removed_files += 1;
            freed_bytes = freed_bytes.saturating_add(size);
        }
    }

    remove_empty_subdirs(temp_dir, false);

    if removed_files > 0 {
        info!(
            "Pruned {} oldest editor cache file(s) ({:.2} MiB freed, limit {:.1} MiB)",
            removed_files,
            (freed_bytes as f64) / (1024.0 * 1024.0),
            (max_bytes as f64) / (1024.0 * 1024.0)
        );
    }
    (removed_files, freed_bytes)
}

fn collect_files_with_mtime(dir: &Path, out: &mut Vec<(PathBuf, u64, SystemTime)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files_with_mtime(&path, out);
        } else if path.is_file() {
            if let Ok(meta) = entry.metadata() {
                let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                out.push((path, meta.len(), mtime));
            }
        }
    }
}

fn remove_empty_subdirs(dir: &Path, remove_self: bool) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            remove_empty_subdirs(&path, true);
        }
    }
    if remove_self && fs::read_dir(dir).is_ok_and(|mut it| it.next().is_none()) {
        let _ = fs::remove_dir(dir);
    }
}

fn collect_dirs_recursive(root: &Path, out: &mut Vec<PathBuf>) {
    out.push(root.to_path_buf());
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if p.file_name().and_then(|n| n.to_str()) == Some(".tmp") {
                continue;
            }
            collect_dirs_recursive(&p, out);
        }
    }
}

/// Moves any legacy `.json` sidecars and `.thumb.jpg` previews out of `output_dir` (and its
/// game subfolders recursively) into the system cache (`%LOCALAPPDATA%\RustClip\cache`),
/// and removes any legacy `.tmp` directory inside `output_dir`.
pub fn migrate_legacy_files_in_output_dir(output_dir: &Path) {
    if !output_dir.exists() {
        return;
    }

    let legacy_tmp = output_dir.join(".tmp");
    if legacy_tmp.exists() {
        if let Err(err) = fs::remove_dir_all(&legacy_tmp) {
            debug!("Could not remove legacy .tmp '{}': {err}", legacy_tmp.display());
        } else {
            info!("Removed legacy .tmp folder '{}'", legacy_tmp.display());
        }
    }

    let mut dirs_to_scan = Vec::new();
    collect_dirs_recursive(output_dir, &mut dirs_to_scan);

    for dir in dirs_to_scan {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut legacy_jsons = Vec::new();
        let mut legacy_thumbs = Vec::new();

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name_lower) = path.file_name().and_then(|n| n.to_str()).map(str::to_ascii_lowercase) else {
                continue;
            };
            if name_lower.ends_with(".json.tmp") {
                let _ = fs::remove_file(&path);
            } else if name_lower.ends_with(".json") {
                legacy_jsons.push(path);
            } else if name_lower.ends_with(".thumb.jpg") {
                legacy_thumbs.push(path);
            }
        }

        for json_path in legacy_jsons {
            let Ok(mut meta) = load_clip_metadata_direct(&json_path) else {
                continue;
            };
            let video_path = resolve_recording_path(&dir, &json_path, &meta.file_path);
            if !video_path.exists() {
                // Remove orphaned legacy sidecar whose MP4 is already gone.
                let legacy_thumb = ClipMetadata::legacy_thumbnail_path_for(&video_path);
                let _ = fs::remove_file(&legacy_thumb);
                let _ = fs::remove_file(&json_path);
                continue;
            }
            meta.file_path = video_path.clone();

            let dest_thumb = ClipMetadata::thumbnail_path_for(&video_path);
            let legacy_thumb = ClipMetadata::legacy_thumbnail_path_for(&video_path);
            if legacy_thumb.exists() {
                let _ = move_file_replace(&legacy_thumb, &dest_thumb);
                meta.thumbnail_path = Some(dest_thumb);
            } else if let Some(prev_thumb) = meta.thumbnail_path.clone() {
                if paths_share_root(&prev_thumb, output_dir) && prev_thumb.exists() {
                    let _ = move_file_replace(&prev_thumb, &dest_thumb);
                    meta.thumbnail_path = Some(dest_thumb);
                }
            }

            if save_clip_metadata(&meta).is_ok() {
                let _ = fs::remove_file(&json_path);
            }
        }

        for thumb_path in legacy_thumbs {
            let Some(fname) = thumb_path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let stem = &fname[..fname.len().saturating_sub(".thumb.jpg".len())];
            let mp4_path = dir.join(format!("{stem}.mp4"));
            if mp4_path.exists() {
                let dest_thumb = ClipMetadata::thumbnail_path_for(&mp4_path);
                let _ = move_file_replace(&thumb_path, &dest_thumb);
            } else {
                let _ = fs::remove_file(&thumb_path);
            }
        }
    }
}

fn move_file_replace(src: &Path, dst: &Path) -> Result<(), String> {
    if normalize_path_key(src) == normalize_path_key(dst) {
        return Ok(());
    }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create_dir_all failed: {e}"))?;
    }
    if dst.exists() {
        let _ = fs::remove_file(src);
        return Ok(());
    }
    match fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(_) => {
            fs::copy(src, dst).map_err(|e| format!("copy failed: {e}"))?;
            let _ = fs::remove_file(src);
            Ok(())
        }
    }
}

/// Saves `metadata` atomically inside `%LOCALAPPDATA%\RustClip\cache\metadata`.
pub fn save_clip_metadata(metadata: &ClipMetadata) -> Result<PathBuf, String> {
    let sidecar_path = ClipMetadata::sidecar_path_for(&metadata.file_path);
    if let Some(parent) = sidecar_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| format!("create_dir_all failed: {e}"))?;
        }
    }

    let json = serde_json::to_string_pretty(metadata)
        .map_err(|e| format!("ClipMetadata JSON serialize failed: {e}"))?;

    let tmp_path = sidecar_path.with_extension("json.tmp");
    fs::write(&tmp_path, json.as_bytes()).map_err(|e| format!("write sidecar tmp failed: {e}"))?;
    fs::rename(&tmp_path, &sidecar_path).map_err(|e| format!("rename sidecar failed: {e}"))?;
    Ok(sidecar_path)
}

fn load_clip_metadata_direct(sidecar_path: &Path) -> Result<ClipMetadata, String> {
    let raw = fs::read_to_string(sidecar_path)
        .map_err(|e| format!("read sidecar '{}' failed: {e}", sidecar_path.display()))?;
    serde_json::from_str::<ClipMetadata>(&raw)
        .map_err(|e| format!("parse sidecar '{}' failed: {e}", sidecar_path.display()))
}

/// Loads a single `ClipMetadata` sidecar from disk (with transparent lookup between
/// system-cache sidecar path and legacy adjacent `.json` path).
pub fn load_clip_metadata(sidecar_path: &Path) -> Result<ClipMetadata, String> {
    if sidecar_path.exists() {
        return load_clip_metadata_direct(sidecar_path);
    }
    let mp4_guess = sidecar_path.with_extension("mp4");
    let cached_path = ClipMetadata::sidecar_path_for(&mp4_guess);
    if cached_path != sidecar_path && cached_path.exists() {
        return load_clip_metadata_direct(&cached_path);
    }
    load_clip_metadata_direct(sidecar_path)
}

/// Lists all valid `ClipMetadata` entries for `.mp4` recordings under `output_dir`, sorted newest-first.
#[must_use]
pub fn list_clip_metadata(output_dir: &Path) -> Vec<ClipMetadata> {
    let mut items: Vec<ClipMetadata> = collect_managed_recordings(output_dir)
        .into_iter()
        .map(|rec| rec.metadata)
        .collect();
    items.sort_by_key(|a| std::cmp::Reverse(a.created_at));
    items
}

struct ManagedRecording {
    sidecar_path: PathBuf,
    metadata: ClipMetadata,
    total_bytes: u64,
}

/// Enforces `StorageSettings` retention rules (`max_storage_gb` & `max_age_days`).
pub struct StorageQuotaManager;

impl StorageQuotaManager {
    /// Enforces GiB quota and age limit in `output_dir` while strictly preserving
    /// any recording marked `favorite` or `is_edited`.
    pub fn enforce_quota(
        output_dir: &Path,
        max_storage_gb: Option<u64>,
        max_age_days: Option<u32>,
    ) -> Result<StorageCleanupReport, String> {
        let max_bytes = max_storage_gb.map(|gb| gb.saturating_mul(1024 * 1024 * 1024));
        Self::enforce_quota_bytes(output_dir, max_bytes, max_age_days, Utc::now())
    }

    /// Enforces an exact byte budget and age limit against a reference timestamp `now`.
    pub fn enforce_quota_bytes(
        output_dir: &Path,
        max_bytes: Option<u64>,
        max_age_days: Option<u32>,
        now: DateTime<Utc>,
    ) -> Result<StorageCleanupReport, String> {
        if !output_dir.exists() {
            return Ok(StorageCleanupReport::default());
        }

        let mut items = collect_managed_recordings(output_dir);
        // Oldest first so eviction removes stale clips first.
        items.sort_by_key(|a| a.metadata.created_at);

        let mut total_bytes: u64 = items.iter().map(|i| i.total_bytes).sum();
        let mut deleted_recordings = 0usize;
        let mut deleted_files = 0usize;
        let mut freed_bytes = 0u64;

        let cutoff = max_age_days
            .filter(|&days| days > 0)
            .map(|days| now - ChronoDuration::days(i64::from(days)));

        // Phase 1: Age-based eviction on non-protected entries.
        let mut kept: Vec<ManagedRecording> = Vec::with_capacity(items.len());
        for item in items {
            let expired = cutoff.is_some_and(|cut| item.metadata.created_at < cut);
            if expired && !item.metadata.is_protected() {
                let (files_removed, bytes_freed) = delete_recording_bundle(&item);
                if files_removed > 0 {
                    deleted_recordings += 1;
                    deleted_files += files_removed;
                    freed_bytes = freed_bytes.saturating_add(bytes_freed);
                    total_bytes = total_bytes.saturating_sub(item.total_bytes);
                }
            } else {
                kept.push(item);
            }
        }

        // Phase 2: Capacity-based eviction (oldest non-protected entries first) until total_bytes <= max_bytes.
        if let Some(limit_bytes) = max_bytes {
            for item in kept {
                if total_bytes <= limit_bytes {
                    continue;
                }
                if item.metadata.is_protected() {
                    continue;
                }
                let (files_removed, bytes_freed) = delete_recording_bundle(&item);
                if files_removed > 0 {
                    deleted_recordings += 1;
                    deleted_files += files_removed;
                    freed_bytes = freed_bytes.saturating_add(bytes_freed);
                    total_bytes = total_bytes.saturating_sub(item.total_bytes);
                }
            }
        }

        if deleted_recordings > 0 {
            info!(
                "Storage quota cleanup removed {} recording(s) ({} file(s), {:.2} MiB freed)",
                deleted_recordings,
                deleted_files,
                (freed_bytes as f64) / (1024.0 * 1024.0)
            );
        }

        Ok(StorageCleanupReport {
            deleted_recordings,
            deleted_files,
            freed_bytes,
            remaining_bytes: total_bytes,
        })
    }
}

fn normalize_path_key(path: &Path) -> String {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path))
    };
    let s = abs.to_string_lossy().replace('/', "\\");
    let stripped = s.strip_prefix(r"\\?\").unwrap_or(&s);
    stripped.trim_end_matches('\\').to_ascii_lowercase()
}

fn paths_share_root(path: &Path, root: &Path) -> bool {
    let p = normalize_path_key(path);
    let r = normalize_path_key(root);
    p == r || p.starts_with(&format!("{r}\\"))
}

fn infer_imported_game_name(output_dir: &Path, mp4_path: &Path) -> (String, bool, bool) {
    let rel_parent = mp4_path
        .parent()
        .and_then(|p| p.strip_prefix(output_dir).ok());
    let mut game_candidate: Option<String> = None;
    let mut is_session = false;
    let mut is_edited = false;

    if let Some(rel) = rel_parent {
        for comp in rel.components() {
            let name = comp.as_os_str().to_string_lossy().trim().to_string();
            if name.is_empty() {
                continue;
            }
            let lower = name.to_ascii_lowercase();
            if lower == "sessions" {
                is_session = true;
                continue;
            }
            if lower == "edited" {
                is_edited = true;
                continue;
            }
            if matches!(lower.as_str(), "clips" | "screen" | "imported") {
                continue;
            }
            game_candidate = Some(name);
        }
    }

    (
        game_candidate.unwrap_or_else(|| "Imported".to_string()),
        is_session,
        is_edited,
    )
}

fn build_imported_clip_metadata(output_dir: &Path, mp4_path: &Path) -> ClipMetadata {
    let title = mp4_path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Imported Clip")
        .to_string();
    let (game_name, is_session, is_edited) = infer_imported_game_name(output_dir, mp4_path);
    let fs_meta = fs::metadata(mp4_path).ok();
    let file_size_bytes = fs_meta.as_ref().map_or(0, |m| m.len());
    let created_at = fs_meta
        .as_ref()
        .and_then(|m| m.modified().ok().or_else(|| m.created().ok()))
        .map_or_else(Utc::now, DateTime::<Utc>::from);

    ClipMetadata {
        id: uuid::Uuid::new_v4(),
        title,
        game_name,
        executable_name: None,
        kind: if is_session {
            core_types::RecordingKind::Session
        } else {
            core_types::RecordingKind::Clip
        },
        file_path: mp4_path.to_path_buf(),
        thumbnail_path: None,
        waveform_path: None,
        created_at,
        duration_sec: 0.0,
        file_size_bytes,
        width: 0,
        height: 0,
        fps: 60,
        video_codec: core_types::VideoCodec::H264,
        video_bitrate_kbps: 0,
        audio_tracks: Vec::new(),
        markers: Vec::new(),
        favorite: false,
        is_edited,
        tags: Vec::new(),
        crash_recovered: false,
    }
}

fn normalize_legacy_metadata_labels(meta: &mut ClipMetadata) -> bool {
    let mut changed = false;

    let gn_lower = meta.game_name.trim().to_ascii_lowercase();
    let is_legacy_screen = gn_lower == "desktop"
        || gn_lower.starts_with("monitor")
        || gn_lower.starts_with("display");
    if is_legacy_screen {
        meta.game_name = "Screen".to_string();
        changed = true;
    }

    let t_lower = meta.title.trim().to_ascii_lowercase();
    if t_lower.starts_with("monitor ")
        || t_lower.starts_with("screen —")
        || t_lower.starts_with("screen -")
        || t_lower.starts_with("desktop —")
    {
        let mon_digits: String = meta
            .title
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        let mon_num = if mon_digits.is_empty() {
            "1"
        } else {
            mon_digits.as_str()
        };
        let suffix = if t_lower.contains("edited") || t_lower.contains("монтаж") {
            " • Edited"
        } else if t_lower.contains("sub-clip") {
            " • Sub-clip"
        } else if t_lower.contains("session") || meta.kind == core_types::RecordingKind::Session {
            " • Session"
        } else {
            ""
        };
        let new_title = if t_lower.starts_with("monitor") {
            format!("Screen (Monitor {mon_num}){suffix}")
        } else {
            format!("Screen{suffix}")
        };
        if meta.title != new_title {
            meta.title = new_title;
            changed = true;
        }
    }

    changed
}

fn collect_managed_recordings(output_dir: &Path) -> Vec<ManagedRecording> {
    if !output_dir.exists() {
        return Vec::new();
    }

    migrate_legacy_files_in_output_dir(output_dir);

    let mut out = Vec::new();
    let mut seen_videos: HashSet<String> = HashSet::new();

    // 1. Scan `output_dir` and all game subdirectories recursively for `.mp4` files and match cache sidecars.
    let mut dirs_to_scan = Vec::new();
    collect_dirs_recursive(output_dir, &mut dirs_to_scan);

    for dir in dirs_to_scan {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let mp4_path = entry.path();
            let file_name_lower = mp4_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            if !mp4_path.is_file()
                || !file_name_lower.ends_with(".mp4")
                || file_name_lower.ends_with(".tmp.mp4")
                || file_name_lower.ends_with(".fmp4")
                || file_len_or_zero(&mp4_path) == 0
            {
                continue;
            }
            let sidecar_path = ClipMetadata::sidecar_path_for(&mp4_path);
            let mut metadata = match load_clip_metadata_direct(&sidecar_path) {
                Ok(m) => m,
                Err(_) => {
                    let imported = build_imported_clip_metadata(output_dir, &mp4_path);
                    let _ = save_clip_metadata(&imported);
                    imported
                }
            };
            metadata.file_path = mp4_path.clone();
            if normalize_legacy_metadata_labels(&mut metadata) {
                let _ = save_clip_metadata(&metadata);
            }
            seen_videos.insert(normalize_path_key(&mp4_path));

            let mut total_bytes = file_len_or_zero(&sidecar_path);
            total_bytes = total_bytes.saturating_add(file_len_or_zero(&mp4_path));
            if let Some(thumb) = &metadata.thumbnail_path {
                total_bytes = total_bytes.saturating_add(file_len_or_zero(thumb));
            }
            if let Some(wave) = &metadata.waveform_path {
                total_bytes = total_bytes.saturating_add(file_len_or_zero(wave));
            }

            out.push(ManagedRecording {
                sidecar_path,
                metadata,
                total_bytes,
            });
        }
    }

    // 2. Also inspect system cache sidecars belonging to `output_dir` (and prune orphans whose `.mp4` was removed).
    let sidecars_dir = ikoch_sidecars_dir();
    if let Ok(entries) = fs::read_dir(&sidecars_dir) {
        for entry in entries.flatten() {
            let sidecar_path = entry.path();
            if !sidecar_path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("json"))
            {
                continue;
            }
            let Ok(mut metadata) = load_clip_metadata_direct(&sidecar_path) else {
                continue;
            };
            if !paths_share_root(&metadata.file_path, output_dir) {
                continue;
            }
            let key = normalize_path_key(&metadata.file_path);
            if seen_videos.contains(&key) {
                continue;
            }
            if !metadata.file_path.exists() {
                // Clean up orphaned metadata and cached thumbnail for externally deleted `.mp4`.
                let _ = fs::remove_file(&sidecar_path);
                let _ = fs::remove_file(ClipMetadata::thumbnail_path_for(&metadata.file_path));
                if let Some(t) = &metadata.thumbnail_path {
                    if paths_share_root(t, &ikoch_thumbnails_dir()) {
                        let _ = fs::remove_file(t);
                    }
                }
                continue;
            }
            if normalize_legacy_metadata_labels(&mut metadata) {
                let _ = save_clip_metadata(&metadata);
            }

            seen_videos.insert(key);
            let mut total_bytes = file_len_or_zero(&sidecar_path);
            total_bytes = total_bytes.saturating_add(file_len_or_zero(&metadata.file_path));
            if let Some(thumb) = &metadata.thumbnail_path {
                total_bytes = total_bytes.saturating_add(file_len_or_zero(thumb));
            }
            if let Some(wave) = &metadata.waveform_path {
                total_bytes = total_bytes.saturating_add(file_len_or_zero(wave));
            }

            out.push(ManagedRecording {
                sidecar_path,
                metadata,
                total_bytes,
            });
        }
    }

    out
}

fn resolve_recording_path(output_dir: &Path, sidecar_path: &Path, stored_path: &Path) -> PathBuf {
    if stored_path.exists() {
        stored_path.to_path_buf()
    } else if let Some(name) = stored_path.file_name() {
        let in_dir = output_dir.join(name);
        if in_dir.exists() {
            return in_dir;
        }
        sidecar_path.with_extension("mp4")
    } else {
        sidecar_path.with_extension("mp4")
    }
}

fn file_len_or_zero(path: &Path) -> u64 {
    fs::metadata(path).map_or(0, |m| m.len())
}

fn delete_recording_bundle(item: &ManagedRecording) -> (usize, u64) {
    let mut deleted_files = 0usize;
    let mut freed = 0u64;

    let mut targets = vec![
        item.sidecar_path.clone(),
        ClipMetadata::sidecar_path_for(&item.metadata.file_path),
        ClipMetadata::legacy_sidecar_path_for(&item.metadata.file_path),
        item.metadata.file_path.clone(),
        ClipMetadata::thumbnail_path_for(&item.metadata.file_path),
        ClipMetadata::legacy_thumbnail_path_for(&item.metadata.file_path),
    ];
    if let Some(thumb) = &item.metadata.thumbnail_path {
        targets.push(thumb.clone());
    }
    if let Some(wave) = &item.metadata.waveform_path {
        targets.push(wave.clone());
    }

    targets.sort();
    targets.dedup();

    for path in targets {
        if path.exists() {
            let sz = file_len_or_zero(&path);
            match fs::remove_file(&path) {
                Ok(()) => {
                    deleted_files += 1;
                    freed = freed.saturating_add(sz);
                }
                Err(err) => warn!("Failed to remove '{}': {err}", path.display()),
            }
        }
    }
    (deleted_files, freed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_types::{MarkerSource, RecordingKind, SessionMarker, VideoCodec};
    use uuid::Uuid;

    fn make_dummy_clip(
        dir: &Path,
        name: &str,
        size: usize,
        age_days: i64,
        favorite: bool,
        is_edited: bool,
    ) -> ClipMetadata {
        let mp4_path = dir.join(format!("{name}.mp4"));
        fs::write(&mp4_path, vec![0xAA; size]).unwrap_or(());
        if let Ok(file) = fs::File::options().write(true).open(&mp4_path) {
            let mtime = std::time::SystemTime::now()
                - std::time::Duration::from_secs(u64::try_from(age_days.max(0)).unwrap_or(0) * 86_400);
            let _ = file.set_modified(mtime);
        }
        let meta = ClipMetadata {
            id: Uuid::new_v4(),
            title: name.to_string(),
            game_name: "Test Game".to_string(),
            executable_name: Some("game.exe".to_string()),
            kind: RecordingKind::Clip,
            file_path: mp4_path,
            thumbnail_path: None,
            waveform_path: None,
            created_at: Utc::now() - ChronoDuration::days(age_days),
            duration_sec: 15.0,
            file_size_bytes: size as u64,
            width: 1920,
            height: 1080,
            fps: 60,
            video_codec: VideoCodec::H264,
            video_bitrate_kbps: 15_000,
            audio_tracks: Vec::new(),
            markers: vec![SessionMarker {
                id: Uuid::new_v4(),
                timestamp_sec: 5.5,
                label: "Bookmark (F8)".to_string(),
                source: MarkerSource::Hotkey,
                created_at: Utc::now(),
            }],
            favorite,
            is_edited,
            tags: Vec::new(),
            crash_recovered: false,
        };
        let _ = save_clip_metadata(&meta);
        meta
    }

    #[test]
    fn quota_cleanup_preserves_favorite_and_edited_and_deletes_oldest_regular_clips()
    -> Result<(), String> {
        let temp_dir = std::env::temp_dir().join(format!("rustclip_quota_{}", Uuid::new_v4()));
        fs::create_dir_all(&temp_dir).map_err(|e| e.to_string())?;

        // Create 4 recordings (~10 KB MP4 each):
        // 1) Oldest regular (40 days old) -> expired AND evictable
        // 2) Old favorite (45 days old, favorite=true) -> MUST survive both age and quota!
        // 3) Old edited (42 days old, is_edited=true) -> MUST survive both age and quota!
        // 4) Newer regular (5 days old) -> evicted if byte limit < 2 protected + 1 regular, else survives
        let old_regular = make_dummy_clip(&temp_dir, "old_regular", 10_000, 40, false, false);
        let old_fav = make_dummy_clip(&temp_dir, "old_fav", 10_000, 45, true, false);
        let old_edited = make_dummy_clip(&temp_dir, "old_edited", 10_000, 42, false, true);
        let mid_regular = make_dummy_clip(&temp_dir, "mid_regular", 10_000, 5, false, false);
        let newest_regular = make_dummy_clip(&temp_dir, "newest_regular", 10_000, 1, false, false);

        // Enforce 30-day max age AND 35 KB byte limit (~32 KB for protected fav+edited + newest_regular).
        let report = StorageQuotaManager::enforce_quota_bytes(
            &temp_dir,
            Some(35_000),
            Some(30),
            Utc::now(),
        )?;

        assert!(!old_regular.file_path.exists(), "expired regular clip must be deleted");
        assert!(!mid_regular.file_path.exists(), "oldest remaining regular clip over byte cap must be deleted");
        assert!(old_fav.file_path.exists(), "favorite clip must NEVER be deleted");
        assert!(old_edited.file_path.exists(), "edited clip must NEVER be deleted");
        assert!(newest_regular.file_path.exists(), "newest clip within quota must stay");
        assert_eq!(report.deleted_recordings, 2);

        // Verify sidecar lives in system cache (not inside `temp_dir`), and F8 markers survived cleanly.
        assert!(!ClipMetadata::legacy_sidecar_path_for(&old_fav.file_path).exists());
        let fav_sidecar = ClipMetadata::sidecar_path_for(&old_fav.file_path);
        assert!(fav_sidecar.exists());
        let reloaded = load_clip_metadata(&fav_sidecar)?;
        assert_eq!(reloaded.markers.len(), 1);
        assert!(reloaded.favorite);

        // Clean up test artifacts and sidecars inside cache.
        let _ = fs::remove_dir_all(&temp_dir);
        for meta in [&old_fav, &old_edited, &newest_regular] {
            let _ = fs::remove_file(ClipMetadata::sidecar_path_for(&meta.file_path));
        }
        Ok(())
    }

    #[test]
    fn bare_mp4_without_sidecar_is_auto_indexed_with_game_folder_name() -> Result<(), String> {
        let temp_dir = std::env::temp_dir().join(format!("ikoch_import_{}", Uuid::new_v4()));
        let game_dir = temp_dir.join("Valorant");
        fs::create_dir_all(&game_dir).map_err(|e| e.to_string())?;
        let mp4_path = game_dir.join("clip.mp4");
        fs::write(&mp4_path, vec![0xAB; 4096]).map_err(|e| e.to_string())?;

        let listed = list_clip_metadata(&temp_dir);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].game_name, "Valorant");
        assert_eq!(listed[0].title, "clip");
        assert_eq!(listed[0].file_size_bytes, 4096);

        let sidecar = ClipMetadata::sidecar_path_for(&mp4_path);
        assert!(sidecar.exists(), "auto-indexed clip must persist JSON sidecar in cache");

        let _ = fs::remove_file(&sidecar);
        let _ = fs::remove_dir_all(&temp_dir);
        Ok(())
    }

    #[test]
    fn migrate_legacy_files_and_cleanup_temp_dir_keep_output_clean() -> Result<(), String> {
        let base = std::env::temp_dir().join(format!("rustclip_clean_{}", Uuid::new_v4()));
        let out_dir = base.join("clips");
        let old_tmp = out_dir.join(".tmp");
        let sys_tmp = base.join("sys_tmp");
        fs::create_dir_all(&old_tmp).map_err(|e| e.to_string())?;
        fs::create_dir_all(sys_tmp.join("editor_cache")).map_err(|e| e.to_string())?;
        fs::write(old_tmp.join("stale.m4a"), b"1234").map_err(|e| e.to_string())?;
        fs::write(sys_tmp.join("editor_cache").join("stem.m4a"), b"123456").map_err(|e| e.to_string())?;

        let mp4 = out_dir.join("Screen_2026-11-18_22-32-10.mp4");
        fs::write(&mp4, vec![0xAB; 256]).map_err(|e| e.to_string())?;
        let legacy_thumb = ClipMetadata::legacy_thumbnail_path_for(&mp4);
        fs::write(&legacy_thumb, b"jpgdata").map_err(|e| e.to_string())?;
        let legacy_json = ClipMetadata::legacy_sidecar_path_for(&mp4);
        let meta = ClipMetadata {
            id: Uuid::new_v4(),
            title: "Screen".to_string(),
            game_name: "Screen".to_string(),
            executable_name: None,
            kind: RecordingKind::Clip,
            file_path: mp4.clone(),
            thumbnail_path: Some(legacy_thumb.clone()),
            waveform_path: None,
            created_at: Utc::now(),
            duration_sec: 5.0,
            file_size_bytes: 256,
            width: 1920,
            height: 1080,
            fps: 60,
            video_codec: VideoCodec::H264,
            video_bitrate_kbps: 20_000,
            audio_tracks: Vec::new(),
            markers: Vec::new(),
            favorite: false,
            is_edited: false,
            tags: Vec::new(),
            crash_recovered: false,
        };
        let raw_json = serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())?;
        fs::write(&legacy_json, raw_json).map_err(|e| e.to_string())?;

        let listed = list_clip_metadata(&out_dir);
        assert_eq!(listed.len(), 1);
        assert!(!old_tmp.exists(), "legacy .tmp must be deleted");
        assert!(!legacy_json.exists(), "legacy .json must be moved out of clips dir");
        assert!(!legacy_thumb.exists(), "legacy .thumb.jpg must be moved out of clips dir");
        assert!(ClipMetadata::sidecar_path_for(&mp4).exists());
        assert!(ClipMetadata::thumbnail_path_for(&mp4).exists());

        let (removed, freed) = cleanup_temp_dir(&sys_tmp);
        assert!(removed >= 1 && freed >= 6);
        assert!(sys_tmp.exists());
        assert_eq!(fs::read_dir(&sys_tmp).map_err(|e| e.to_string())?.count(), 0);

        let _ = fs::remove_file(ClipMetadata::sidecar_path_for(&mp4));
        let _ = fs::remove_file(ClipMetadata::thumbnail_path_for(&mp4));
        let _ = fs::remove_dir_all(&base);
        Ok(())
    }

    #[test]
    fn prune_temp_cache_to_limit_evicts_oldest_stems_and_keeps_metadata() -> Result<(), String> {
        let base = std::env::temp_dir().join(format!("ferrix_prune_{}", Uuid::new_v4()));
        let sys_tmp = base.join("sys_tmp");
        let editor_dir = sys_tmp.join("editor_cache");
        fs::create_dir_all(&editor_dir).map_err(|e| e.to_string())?;

        let old_stem = editor_dir.join("old_track0.m4a");
        fs::write(&old_stem, vec![1u8; 600]).map_err(|e| e.to_string())?;
        std::thread::sleep(std::time::Duration::from_millis(25));
        let mid_stem = editor_dir.join("mid_track0.m4a");
        fs::write(&mid_stem, vec![2u8; 500]).map_err(|e| e.to_string())?;
        std::thread::sleep(std::time::Duration::from_millis(25));
        let new_stem = editor_dir.join("new_track0.m4a");
        fs::write(&new_stem, vec![3u8; 400]).map_err(|e| e.to_string())?;

        let (removed, freed) = prune_temp_cache_to_limit(&sys_tmp, 950);
        assert_eq!(removed, 1);
        assert_eq!(freed, 600);
        assert!(!old_stem.exists(), "oldest stem should be evicted first");
        assert!(mid_stem.exists());
        assert!(new_stem.exists());

        let stats = measure_editor_cache(&sys_tmp);
        assert_eq!(stats.temp_file_count, 2);
        assert_eq!(stats.temp_bytes, 900);

        let _ = fs::remove_dir_all(&base);
        Ok(())
    }

    #[test]
    fn recursive_listing_and_quota_cover_clips_edited_sessions_and_root() -> Result<(), String> {
        let out_dir = std::env::temp_dir().join(format!("ikoch_folders_{}", Uuid::new_v4()));
        let clips_screen = out_dir.join("Clips").join("Screen");
        let sessions_cs2 = out_dir.join("Sessions").join("Counter-Strike 2");
        let edited_dir = out_dir.join("Edited");
        fs::create_dir_all(&clips_screen).map_err(|e| e.to_string())?;
        fs::create_dir_all(&sessions_cs2).map_err(|e| e.to_string())?;
        fs::create_dir_all(&edited_dir).map_err(|e| e.to_string())?;

        // 1) Legacy root clip (oldest, unprotected)
        let root_clip = make_dummy_clip(&out_dir, "Legacy_2026-11-19_09-00", 10_000, 12, false, false);
        // 2) Normal clip inside Clips\Screen\ (second oldest, unprotected)
        let folder_clip = make_dummy_clip(&clips_screen, "Screen_2026-11-19_10-00", 10_000, 6, false, false);
        // 3) Full session inside Sessions\Counter-Strike 2\ (newer, unprotected)
        let session_mp4 = sessions_cs2.join("Counter-Strike 2_2026-11-19_10-05.mp4");
        fs::write(&session_mp4, vec![0xBB; 10_000]).map_err(|e| e.to_string())?;
        let mut session_meta = make_dummy_clip(&sessions_cs2, "tmp_sess", 0, 2, false, false);
        let _ = fs::remove_file(&session_meta.file_path);
        let _ = fs::remove_file(ClipMetadata::sidecar_path_for(&session_meta.file_path));
        session_meta.file_path = session_mp4;
        session_meta.kind = RecordingKind::Session;
        session_meta.game_name = "Counter-Strike 2".to_string();
        session_meta.title = "Counter-Strike 2 • Сессия".to_string();
        session_meta.file_size_bytes = 10_000;
        save_clip_metadata(&session_meta)?;
        // 4) Edited export inside Edited\ (is_edited = true -> protected from quota cleanup)
        let edited_clip = make_dummy_clip(&edited_dir, "Screen_2026-11-19_10-10_edited", 10_000, 20, false, true);

        // Verify list_clip_metadata scans root + Clips\ + Sessions\ + Edited\ recursively
        let listed = list_clip_metadata(&out_dir);
        assert_eq!(listed.len(), 4);

        // Verify sidecar load/update works for nested clip paths (rename flow)
        let nested_sidecar = ClipMetadata::sidecar_path_for(&folder_clip.file_path);
        let mut reloaded = load_clip_metadata(&nested_sidecar)?;
        reloaded.title = "Renamed Clutch".to_string();
        save_clip_metadata(&reloaded)?;
        let after_rename = list_clip_metadata(&out_dir);
        assert!(after_rename.iter().any(|m| m.title == "Renamed Clutch"));

        // Verify zero .json / .jpg / .tmp files exist inside output_dir
        let mut dirs = Vec::new();
        collect_dirs_recursive(&out_dir, &mut dirs);
        for d in dirs {
            if let Ok(entries) = fs::read_dir(d) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
                    assert!(
                        !ext.eq_ignore_ascii_case("json")
                            && !ext.eq_ignore_ascii_case("jpg")
                            && !ext.eq_ignore_ascii_case("tmp"),
                        "unexpected cache file in video folder: {}",
                        p.display()
                    );
                }
            }
        }

        // Enforce tight quota (~25 KB): evicts oldest unprotected (root + Clips\Screen) while keeping Edited + newest Session
        let report = StorageQuotaManager::enforce_quota_bytes(&out_dir, Some(25_000), None, Utc::now())?;
        assert!(report.deleted_recordings >= 2);
        assert!(!root_clip.file_path.exists());
        assert!(!folder_clip.file_path.exists());
        assert!(edited_clip.file_path.exists(), "edited clip in Edited\\ must be protected");
        assert!(session_meta.file_path.exists(), "newest session should remain within budget");

        let _ = fs::remove_dir_all(&out_dir);
        for meta in [&root_clip, &folder_clip, &session_meta, &edited_clip] {
            let _ = fs::remove_file(ClipMetadata::sidecar_path_for(&meta.file_path));
        }
        Ok(())
    }
}
