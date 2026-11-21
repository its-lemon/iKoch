//! Clip/session sidecar metadata, bookmarks, gallery filters, and offline edit requests.

use crate::packet::TrackKind;
use crate::settings::VideoCodec;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Distinguishes replay-buffer clips from full-length session recordings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingKind {
    Clip,
    Session,
}

/// Origin of a session/clip bookmark marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkerSource {
    Hotkey,
    ManualUi,
    SystemEvent,
}

/// Timeline bookmark saved in sidecar JSON and rendered as flags in the trimmer UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionMarker {
    pub id: Uuid,
    /// Offset from the start of the video (`t = 0.0`) in seconds.
    pub timestamp_sec: f64,
    pub label: String,
    pub source: MarkerSource,
    pub created_at: DateTime<Utc>,
}

/// Metadata describing one audio track inside an MP4 container.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioTrackMetadata {
    pub track_index: u8,
    pub kind: TrackKind,
    pub label: String,
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub default_volume: f32,
    /// Optional cached extracted audio asset used by the dual-track Web Audio player.
    pub cached_audio_path: Option<PathBuf>,
}

/// Persistent `.json` sidecar schema stored inside system cache (`%LOCALAPPDATA%\RustClip\cache\metadata`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipMetadata {
    pub id: Uuid,
    pub title: String,
    pub game_name: String,
    pub executable_name: Option<String>,
    pub kind: RecordingKind,
    pub file_path: PathBuf,
    pub thumbnail_path: Option<PathBuf>,
    pub waveform_path: Option<PathBuf>,
    pub created_at: DateTime<Utc>,
    pub duration_sec: f64,
    pub file_size_bytes: u64,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub video_codec: VideoCodec,
    pub video_bitrate_kbps: u32,
    pub audio_tracks: Vec<AudioTrackMetadata>,
    pub markers: Vec<SessionMarker>,
    #[serde(alias = "is_favorite")]
    pub favorite: bool,
    #[serde(default)]
    pub is_edited: bool,
    pub tags: Vec<String>,
    pub crash_recovered: bool,
}

/// Resolves the root hidden/system cache directory (`%LOCALAPPDATA%\iKoch\cache`).
#[must_use]
pub fn ikoch_cache_root_dir() -> PathBuf {
    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
        let trimmed = local_app_data.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed).join("iKoch").join("cache");
        }
    }
    if let Ok(app_data) = std::env::var("APPDATA") {
        let trimmed = app_data.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed).join("iKoch").join("cache");
        }
    }
    if let Ok(xdg_cache) = std::env::var("XDG_CACHE_HOME") {
        let trimmed = xdg_cache.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed).join("iKoch").join("cache");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed).join(".cache").join("iKoch").join("cache");
        }
    }
    std::env::temp_dir().join("iKoch").join("cache")
}

/// Resolves the directory where `.json` clip sidecars are stored out-of-band (`%LOCALAPPDATA%\iKoch\cache\metadata`).
#[must_use]
pub fn ikoch_sidecars_dir() -> PathBuf {
    ikoch_cache_root_dir().join("metadata")
}

/// Resolves the directory where `.jpg` clip thumbnails are cached (`%LOCALAPPDATA%\iKoch\cache\thumbnails`).
#[must_use]
pub fn ikoch_thumbnails_dir() -> PathBuf {
    ikoch_cache_root_dir().join("thumbnails")
}

/// Resolves the system temporary editor cache directory (`%LOCALAPPDATA%\iKoch\cache\tmp`).
#[must_use]
pub fn ikoch_temp_dir() -> PathBuf {
    ikoch_cache_root_dir().join("tmp")
}

/// Resolves the persistent log directory (`%LOCALAPPDATA%\iKoch\logs`).
#[must_use]
pub fn ikoch_logs_dir() -> PathBuf {
    ikoch_cache_root_dir()
        .parent()
        .map_or_else(|| PathBuf::from("logs"), |p| p.join("logs"))
}

fn path_parent_hash8(video_path: &Path) -> String {
    let parent_norm = video_path
        .parent()
        .map(|p| p.to_string_lossy().replace('\\', "/").to_ascii_lowercase())
        .unwrap_or_default();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in parent_norm.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{:08x}", (hash ^ (hash >> 32)) as u32)
}

impl ClipMetadata {
    /// Returns the canonical out-of-band sidecar `.json` path in system cache paired with an `.mp4` recording.
    #[must_use]
    pub fn sidecar_path_for(video_path: &Path) -> PathBuf {
        let stem = video_path
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("clip");
        let hash8 = path_parent_hash8(video_path);
        ikoch_sidecars_dir().join(format!("{stem}_{hash8}.json"))
    }

    /// Returns the legacy sidecar `.json` path alongside the `.mp4` file (used for seamless migration).
    #[must_use]
    pub fn legacy_sidecar_path_for(video_path: &Path) -> PathBuf {
        video_path.with_extension("json")
    }

    /// Returns the canonical out-of-band thumbnail `.jpg` path in system cache paired with an `.mp4` recording.
    #[must_use]
    pub fn thumbnail_path_for(video_path: &Path) -> PathBuf {
        let stem = video_path
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("clip");
        let hash8 = path_parent_hash8(video_path);
        ikoch_thumbnails_dir().join(format!("{stem}_{hash8}_thumb.jpg"))
    }

    /// Returns the legacy `.thumb.jpg` path alongside the `.mp4` file (used for seamless migration).
    #[must_use]
    pub fn legacy_thumbnail_path_for(video_path: &Path) -> PathBuf {
        video_path.with_extension("thumb.jpg")
    }

    /// Returns `true` if storage quota eviction must protect this file (`favorite` or `is_edited`).
    #[must_use]
    pub const fn is_protected(&self) -> bool {
        self.favorite || self.is_edited
    }
}

/// Query filter used by the gallery UI to filter clips and sessions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GalleryFilter {
    pub kind: Option<RecordingKind>,
    pub game_name: Option<String>,
    pub favorites_only: bool,
    pub search_query: Option<String>,
}

/// How multi-track audio should be handled when trimming via FFmpeg sidecar.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrimAudioExportMode {
    /// Lossless `stream copy` preserving both Track 1 (Game) and Track 2 (Mic).
    #[default]
    KeepSeparateTracks,
    /// Mix Game and Mic tracks into a single stereo track for broad player compatibility.
    MixToSingleTrack,
    /// Keep only Track 1 (Game audio).
    GameTrackOnly,
    /// Keep only Track 2 (Microphone audio).
    MicTrackOnly,
}

/// Request payload for fast trim (`stream copy` or audio mixdown) via FFmpeg sidecar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrimRequest {
    pub clip_id: Uuid,
    pub source_path: PathBuf,
    pub output_path: PathBuf,
    pub start_sec: f64,
    pub end_sec: f64,
    pub audio_mode: TrimAudioExportMode,
    pub game_volume: f32,
    pub mic_volume: f32,
}

/// Target size presets for the "Compress for Discord" FFmpeg workflow.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "preset", rename_all = "snake_case")]
pub enum DiscordPreset {
    Mb10,
    Mb50,
    CustomMb { size_mb: f64 },
}

impl DiscordPreset {
    /// Returns the target size budget in bytes (with a small 3% container overhead margin).
    #[must_use]
    pub fn target_bytes(self) -> u64 {
        let mb = match self {
            Self::Mb10 => 10.0,
            Self::Mb50 => 50.0,
            Self::CustomMb { size_mb } => size_mb.max(1.0),
        };
        let raw_bytes = mb * 1024.0 * 1024.0;
        // Reserve 3% headroom for MP4 muxing container overhead so the final file stays strictly under limit.
        let safe_bytes = raw_bytes * 0.97;
        safe_bytes.max(1.0) as u64
    }
}

/// Request payload for compressing a clip/selection under Discord upload size limits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscordCompressRequest {
    pub clip_id: Uuid,
    pub source_path: PathBuf,
    pub output_path: PathBuf,
    pub preset: DiscordPreset,
    pub start_sec: Option<f64>,
    pub end_sec: Option<f64>,
    pub audio_bitrate_kbps: u32,
    pub mix_audio_tracks: bool,
    pub game_volume: f32,
    pub mic_volume: f32,
}

impl DiscordCompressRequest {
    /// Computes target video bitrate in kbps:
    /// `((target_bytes * 8) / duration_sec) / 1000 - audio_bitrate_kbps`.
    ///
    /// Returns `None` if `duration_sec <= 0.0` or the budget cannot fit even a minimal video stream.
    #[must_use]
    pub fn calculate_target_video_bitrate_kbps(
        target_bytes: u64,
        duration_sec: f64,
        audio_bitrate_kbps: u32,
    ) -> Option<u32> {
        if !duration_sec.is_finite() || duration_sec <= 0.0 || target_bytes == 0 {
            return None;
        }
        let total_bps = (target_bytes as f64 * 8.0) / duration_sec;
        let total_kbps = total_bps / 1_000.0;
        let video_kbps = total_kbps - f64::from(audio_bitrate_kbps);
        if video_kbps < 100.0 {
            return None;
        }
        Some(video_kbps.floor() as u32)
    }
}

/// Inclusive `[start_sec, end_sec]` time span used for multi-segment Split & Cut editing.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SegmentRange {
    pub start_sec: f64,
    pub end_sec: f64,
}

/// Target frame aspect ratio preset for Medal+ exports (stages 5–6).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AspectRatioPreset {
    #[default]
    Original,
    /// Vertical 9:16 portrait canvas (e.g., 1080x1920) with blurred/darkened ambient background.
    Vertical9x16Blurred,
    /// Square 1:1 canvas.
    Square1x1,
}

/// Text annotation rendered via FFmpeg `drawtext`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextOverlaySpec {
    pub text: String,
    /// Normalized horizontal anchor (`0.0..=1.0`).
    pub x_norm: f32,
    /// Normalized vertical anchor (`0.0..=1.0`).
    pub y_norm: f32,
    pub font_size: u32,
    pub color_hex: String,
    pub start_sec: Option<f64>,
    pub end_sec: Option<f64>,
}

/// Per-track gain/mute override applied by the Stage 5–6 multi-track FFmpeg mixer.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TrackMixSpec {
    pub kind: TrackKind,
    /// Linear gain multiplier (`0.0..=2.0` -> `0%..200%`).
    pub volume: f32,
    pub muted: bool,
}

/// Full Medal+ offline export request for Stage 5–6 (`FFmpeg` sidecar + optional Discord webhook).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdvancedExportRequest {
    pub clip_id: Uuid,
    pub source_path: PathBuf,
    pub output_path: PathBuf,
    /// Ordered kept segments stitched sequentially via FFmpeg `concat`.
    pub segments: Vec<SegmentRange>,
    /// Playback speed multiplier (`0.25..=4.0`).
    pub playback_speed: f32,
    pub aspect_ratio: AspectRatioPreset,
    pub text_overlays: Vec<TextOverlaySpec>,
    pub fade_in_sec: f32,
    pub fade_out_sec: f32,
    pub track_mix: Vec<TrackMixSpec>,
    pub discord_preset: Option<DiscordPreset>,
    pub export_gif: bool,
    pub subclip_from_session: bool,
    pub discord_webhook_url: Option<String>,
}

/// Request to extract a full-resolution `PNG` still frame from a clip via FFmpeg sidecar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrameScreenshotRequest {
    pub clip_id: Uuid,
    pub source_path: PathBuf,
    pub output_png_path: PathBuf,
    pub timestamp_sec: f64,
}

/// Live disk usage breakdown separating heavy temporary editor stems (`.m4a`/`.tmp`)
/// from persistent `.json` sidecars and `.jpg` thumbnails.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorCacheStats {
    pub temp_bytes: u64,
    pub temp_file_count: usize,
    pub metadata_bytes: u64,
    pub metadata_file_count: usize,
}
