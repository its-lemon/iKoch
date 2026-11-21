//! Application configuration contracts (`AppSettings` and sub-configs).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Current configuration schema version.
pub const SETTINGS_SCHEMA_VERSION: u32 = 1;

/// Minimum replay buffer length in seconds.
pub const MIN_REPLAY_DURATION_SECS: u32 = 15;

/// Maximum replay buffer length in seconds (5 minutes).
pub const MAX_REPLAY_DURATION_SECS: u32 = 300;

/// UI language applied across Ferrix frontend, system tray, and overlay toasts.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppLanguage {
    #[default]
    Ru,
    En,
}

impl AppLanguage {
    /// Returns the short language code (`"ru"` or `"en"`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ru => "ru",
            Self::En => "en",
        }
    }
}

/// Visual color theme preset applied across the iKoch UI and Toast overlays.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppTheme {
    #[default]
    #[serde(alias = "ferrix_dark")]
    IkochDark,
    OledMidnight,
    CrimsonPro,
    NordicSlate,
    AmethystNeon,
    SunsetAmber,
    EmeraldMatrix,
    FrostSlate,
    Custom,
}

impl AppTheme {
    /// Returns the `data-theme` HTML attribute slug for this theme.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IkochDark => "ikoch_dark",
            Self::OledMidnight => "oled_midnight",
            Self::CrimsonPro => "crimson_pro",
            Self::NordicSlate => "nordic_slate",
            Self::AmethystNeon => "amethyst_neon",
            Self::SunsetAmber => "sunset_amber",
            Self::EmeraldMatrix => "emerald_matrix",
            Self::FrostSlate => "frost_slate",
            Self::Custom => "custom",
        }
    }
}

/// Editable color palette for [`AppTheme::Custom`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomThemeColors {
    pub bg_app: String,
    pub bg_panel: String,
    pub accent: String,
    pub text_primary: String,
}

impl Default for CustomThemeColors {
    fn default() -> Self {
        Self {
            bg_app: "#090B12".to_string(),
            bg_panel: "#111522".to_string(),
            accent: "#6366F1".to_string(),
            text_primary: "#F1F5F9".to_string(),
        }
    }
}

fn normalize_hex_color(raw: &mut String, fallback: &str) {
    let trimmed = raw.trim();
    let valid = trimmed.len() == 7
        && trimmed.starts_with('#')
        && trimmed[1..].bytes().all(|b| b.is_ascii_hexdigit());
    if valid {
        *raw = trimmed.to_ascii_uppercase();
    } else {
        *raw = fallback.to_string();
    }
}

/// Root persistent settings persisted to disk and synced with the Tauri UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub schema_version: u32,
    pub launch_at_startup: bool,
    pub language: AppLanguage,
    pub theme: AppTheme,
    pub custom_theme: CustomThemeColors,
    pub capture: CaptureSettings,
    pub video: VideoSettings,
    pub audio: AudioSettings,
    pub replay_buffer: ReplayBufferSettings,
    pub hotkeys: HotkeySettings,
    pub notifications: NotificationSettings,
    pub storage: StorageSettings,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            launch_at_startup: false,
            language: AppLanguage::Ru,
            theme: AppTheme::IkochDark,
            custom_theme: CustomThemeColors::default(),
            capture: CaptureSettings::default(),
            video: VideoSettings::default(),
            audio: AudioSettings::default(),
            replay_buffer: ReplayBufferSettings::default(),
            hotkeys: HotkeySettings::default(),
            notifications: NotificationSettings::default(),
            storage: StorageSettings::default(),
        }
    }
}

impl AppSettings {
    /// Clamps configurable ranges to safe operational bounds.
    pub fn normalize(&mut self) {
        self.video.fps = self.video.fps.clamp(15, 240);
        self.video.bitrate_kbps = self.video.bitrate_kbps.clamp(1_000, 150_000);
        self.video.gop_seconds = 2;
        self.audio.game_volume = self.audio.game_volume.clamp(0.0, 2.0);
        self.audio.mic_volume = self.audio.mic_volume.clamp(0.0, 2.0);
        self.audio.secondary_volume = self.audio.secondary_volume.clamp(0.0, 2.0);
        self.audio.mic_noise_suppression_strength =
            self.audio.mic_noise_suppression_strength.clamp(0.0, 1.0);
        if let Some(gate_db) = self.audio.mic_noise_gate_db.as_mut() {
            *gate_db = gate_db.clamp(-90.0, -10.0);
        }
        match &mut self.audio.mic_activation_mode {
            MicActivationMode::AlwaysOn => {}
            MicActivationMode::PushToTalk {
                release_delay_ms, ..
            }
            | MicActivationMode::PushToMute {
                release_delay_ms, ..
            } => {
                *release_delay_ms = (*release_delay_ms).clamp(0, 2_000);
            }
        }
        self.audio.sample_rate_hz = 48_000;
        self.audio.channels = 2;
        self.audio.aac_bitrate_kbps = self.audio.aac_bitrate_kbps.clamp(64, 320);
        self.replay_buffer.duration_secs = self
            .replay_buffer
            .duration_secs
            .clamp(MIN_REPLAY_DURATION_SECS, MAX_REPLAY_DURATION_SECS);
        self.replay_buffer.max_ram_mb = self.replay_buffer.max_ram_mb.clamp(64, 4096);
        self.notifications.sound_volume = self.notifications.sound_volume.clamp(0.0, 1.0);
        self.notifications.sounds.clip_saved.volume =
            self.notifications.sounds.clip_saved.volume.clamp(0.0, 1.0);
        self.notifications.sounds.recording_started.volume =
            self.notifications.sounds.recording_started.volume.clamp(0.0, 1.0);
        self.notifications.sounds.recording_stopped.volume =
            self.notifications.sounds.recording_stopped.volume.clamp(0.0, 1.0);
        self.notifications.sounds.marker_added.volume =
            self.notifications.sounds.marker_added.volume.clamp(0.0, 1.0);
        self.notifications.overlay_duration_ms =
            self.notifications.overlay_duration_ms.clamp(500, 10_000);
        self.capture.games.auto_pause_delay_secs =
            self.capture.games.auto_pause_delay_secs.clamp(0, 60);
        self.storage.editor_cache.interval_minutes =
            self.storage.editor_cache.interval_minutes.clamp(5, 1440);
        self.storage.editor_cache.max_size_mb =
            self.storage.editor_cache.max_size_mb.clamp(50, 4096);
        let fallback = CustomThemeColors::default();
        normalize_hex_color(&mut self.custom_theme.bg_app, &fallback.bg_app);
        normalize_hex_color(&mut self.custom_theme.bg_panel, &fallback.bg_panel);
        normalize_hex_color(&mut self.custom_theme.accent, &fallback.accent);
        normalize_hex_color(&mut self.custom_theme.text_primary, &fallback.text_primary);
    }

    /// Estimates the compressed ring buffer memory footprint in bytes for the current bitrate and duration.
    #[must_use]
    pub fn estimated_ring_buffer_bytes(&self) -> usize {
        let video_bps = u64::from(self.video.bitrate_kbps) * 1_000;
        let has_any_audio = self.audio.game_enabled
            || self.audio.mic_enabled
            || self.audio.secondary_output_device.is_some();
        let mixed_kbps = if has_any_audio {
            u64::from(self.audio.aac_bitrate_kbps.min(128))
        } else {
            0
        };
        let stem_kbps = if self.audio.separate_tracks {
            let game_kbps = if self.audio.game_enabled {
                u64::from(self.audio.aac_bitrate_kbps.min(128))
            } else {
                0
            };
            let mic_kbps = if self.audio.mic_enabled {
                u64::from(self.audio.aac_bitrate_kbps.min(96))
            } else {
                0
            };
            let sec_kbps = if self.audio.secondary_output_device.is_some() {
                u64::from(self.audio.aac_bitrate_kbps.min(128))
            } else {
                0
            };
            game_kbps + mic_kbps + sec_kbps
        } else {
            0
        };
        let audio_bps = (mixed_kbps + stem_kbps) * 1_000;
        let total_bps = video_bps + audio_bps;
        // Include ~10% headroom for GOP alignment beyond the target window.
        let estimated_bytes = (total_bps / 8) * u64::from(self.replay_buffer.duration_secs);
        let with_headroom = estimated_bytes + estimated_bytes / 10;
        let max_bytes = (self.replay_buffer.max_ram_mb as u64).saturating_mul(1024 * 1024);
        usize::try_from(with_headroom.min(max_bytes)).unwrap_or(usize::MAX)
    }
}

/// Capture target selection mode supporting live hot-switching without process restart.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CaptureTarget {
    /// Automatically track the active fullscreen/borderless game window (sleeps on desktop).
    #[default]
    AutoGame,
    /// Capture strictly a selected window via WGC (persists across Alt+Tab; waits while minimized).
    SpecificWindow {
        hwnd: isize,
        process_name: String,
        title: String,
    },
    /// Capture an entire monitor continuously (Monitor 1, 2, etc.).
    Monitor {
        monitor_id: String,
        name: String,
    },
}

/// Compatibility alias for `CaptureTarget`.
pub type CaptureMode = CaptureTarget;

/// Connected display output metadata exposed to the UI and tray menu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorInfo {
    pub monitor_id: String,
    pub name: String,
    pub hmonitor: isize,
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub is_primary: bool,
}

/// Capturable top-level window metadata exposed to the UI and tray menu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowInfo {
    pub hwnd: isize,
    pub process_id: u32,
    pub process_name: String,
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub is_minimized: bool,
}

/// Game auto-detection settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GameDetectionConfig {
    pub auto_start: bool,
    pub auto_pause_delay_secs: u32,
    pub use_builtin_game_list: bool,
    pub custom_whitelist_exes: Vec<String>,
    pub blacklist_exes: Vec<String>,
}

impl Default for GameDetectionConfig {
    fn default() -> Self {
        Self {
            auto_start: true,
            auto_pause_delay_secs: 5,
            use_builtin_game_list: true,
            custom_whitelist_exes: Vec::new(),
            #[cfg(windows)]
            blacklist_exes: vec![
                "explorer.exe".to_string(),
                "chrome.exe".to_string(),
                "msedge.exe".to_string(),
                "firefox.exe".to_string(),
                "discord.exe".to_string(),
                "ikoch.exe".to_string(),
            ],
            #[cfg(not(windows))]
            blacklist_exes: vec![
                "explorer".to_string(),
                "ikoch".to_string(),
                "chrome".to_string(),
                "chromium".to_string(),
                "firefox".to_string(),
                "discord".to_string(),
                "steamwebhelper".to_string(),
            ],
        }
    }
}

/// Strategy for HDR (`RGBA16F` / scRGB) capture sources.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HdrMode {
    /// Tone-map HDR surfaces to SDR BT.709 on GPU before NV12 encoding.
    #[default]
    AutoToneMapToSdr,
    /// Force SDR `B8G8R8A8Unorm` WGC capture pixel format when possible.
    ForceSdrClamp,
    /// Preserve 10-bit PQ HDR pipeline (requires HEVC Main10 hardware encoder).
    PreserveHdr10,
}

/// Capture subsystem configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureSettings {
    #[serde(alias = "mode")]
    pub target: CaptureTarget,
    pub games: GameDetectionConfig,
    pub capture_cursor: bool,
    pub capture_border: bool,
    pub hdr_mode: HdrMode,
}

impl Default for CaptureSettings {
    fn default() -> Self {
        Self {
            target: CaptureTarget::AutoGame,
            games: GameDetectionConfig::default(),
            capture_cursor: true,
            capture_border: false,
            hdr_mode: HdrMode::AutoToneMapToSdr,
        }
    }
}

/// Hardware video codec selection.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoCodec {
    #[default]
    H264,
    Hevc,
    Av1,
}

/// Preferred hardware GPU encoder vendor for Media Foundation MFT enumeration.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuVendorPreference {
    #[default]
    Auto,
    NvidiaNvenc,
    AmdAmf,
    IntelQsv,
    SoftwareFallback,
}

/// Output resolution target applied on GPU via `ID3D11VideoProcessor`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "preset", rename_all = "snake_case")]
pub enum ResolutionPreset {
    Native,
    #[default]
    P1080,
    P720,
    Custom { width: u32, height: u32 },
}

impl ResolutionPreset {
    /// Resolves even-aligned `(width, height)` given the captured source dimensions.
    #[must_use]
    pub fn resolve(self, src_width: u32, src_height: u32) -> (u32, u32) {
        let src_w = src_width.max(2);
        let src_h = src_height.max(2);

        let (raw_w, raw_h) = match self {
            Self::Native => (src_w, src_h),
            Self::P1080 => scale_to_height(src_w, src_h, 1080),
            Self::P720 => scale_to_height(src_w, src_h, 720),
            Self::Custom { width, height } => (width.max(2), height.max(2)),
        };

        // NV12 chroma subsampling (4:2:0) and H.264/HEVC encoders require even dimensions.
        (raw_w & !1, raw_h & !1)
    }
}

fn scale_to_height(src_w: u32, src_h: u32, target_h: u32) -> (u32, u32) {
    if src_h <= target_h {
        return (src_w, src_h);
    }
    let scaled_w = ((u64::from(src_w) * u64::from(target_h)) / u64::from(src_h)).max(2);
    (u32::try_from(scaled_w).unwrap_or(src_w), target_h)
}

/// Encoder rate control mode.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RateControlMode {
    Cbr,
    #[default]
    Vbr,
    Cqp,
}

/// Hardware video encoder settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoSettings {
    pub codec: VideoCodec,
    pub gpu_vendor: GpuVendorPreference,
    pub resolution: ResolutionPreset,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub rate_control: RateControlMode,
    /// Fixed keyframe interval in seconds (1 or 2 seconds for crisp GOP buffer eviction).
    pub gop_seconds: u32,
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self {
            codec: VideoCodec::H264,
            gpu_vendor: GpuVendorPreference::Auto,
            resolution: ResolutionPreset::P1080,
            fps: 60,
            bitrate_kbps: 20_000,
            rate_control: RateControlMode::Vbr,
            gop_seconds: 2,
        }
    }
}

/// Audio endpoint descriptor exposed to the UI and CLI device selectors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioDeviceInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// System/Game audio loopback capture mode.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", content = "device_id", rename_all = "snake_case")]
pub enum AudioOutputMode {
    /// Captures the default Windows audio render device via WASAPI loopback.
    #[default]
    DefaultDevice,
    /// Captures a specific Windows audio render endpoint by device ID.
    SpecificDevice(String),
    /// Captures audio only from the detected/captured game PID (Win10 20348+ / Win11),
    /// automatically falling back to `DefaultDevice` if unsupported or capturing a monitor.
    GameProcessOnly,
}

/// Microphone input selection mode.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", content = "device_id", rename_all = "snake_case")]
pub enum MicDeviceConfig {
    /// Microphone track disabled (writes silent Track 2 only if `separate_tracks` forces it, or omitted).
    None,
    /// Captures the default Windows microphone endpoint.
    #[default]
    Default,
    /// Captures a specific microphone endpoint by device ID.
    SpecificDevice(String),
}

const fn default_ptt_release_delay_ms() -> u32 {
    150
}

/// Microphone activation / transmission mode (hotkey gating without global hooks).
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum MicActivationMode {
    #[default]
    AlwaysOn,
    PushToTalk {
        key: String,
        #[serde(default = "default_ptt_release_delay_ms")]
        release_delay_ms: u32,
    },
    PushToMute {
        key: String,
        #[serde(default = "default_ptt_release_delay_ms")]
        release_delay_ms: u32,
    },
}

/// Multi-track WASAPI + RNNoise + AAC audio pipeline settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    pub audio_output_mode: AudioOutputMode,
    pub mic_device: MicDeviceConfig,
    pub mic_activation_mode: MicActivationMode,
    pub mic_soft_limiter: bool,
    pub game_enabled: bool,
    pub game_device_id: Option<String>,
    pub game_volume: f32,
    pub game_muted: bool,
    pub mic_enabled: bool,
    pub mic_device_id: Option<String>,
    pub mic_volume: f32,
    pub mic_muted: bool,
    /// Optional secondary output device ID (e.g., Discord / Voicemod virtual cable) captured as Track 4.
    pub secondary_output_device: Option<String>,
    pub secondary_volume: f32,
    pub secondary_muted: bool,
    /// Enables pure-Rust RNNoise microphone noise suppression (only affects Microphone, never Game Audio).
    pub mic_noise_suppression: bool,
    /// Wet/dry blend ratio for microphone noise suppression (`0.0` = raw, `1.0` = full RNNoise).
    pub mic_noise_suppression_strength: f32,
    /// Smooth attack/release noise gate threshold in dBFS applied only to Microphone (`None` = disabled).
    pub mic_noise_gate_db: Option<f32>,
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub aac_bitrate_kbps: u32,
    /// Records clean stem tracks (`Game`, `Mic`, optional `Secondary`) alongside the default `Mixed` track.
    pub separate_tracks: bool,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            audio_output_mode: AudioOutputMode::DefaultDevice,
            mic_device: MicDeviceConfig::Default,
            mic_activation_mode: MicActivationMode::AlwaysOn,
            mic_soft_limiter: true,
            game_enabled: true,
            game_device_id: None,
            game_volume: 1.0,
            game_muted: false,
            mic_enabled: true,
            mic_device_id: None,
            mic_volume: 1.0,
            mic_muted: false,
            secondary_output_device: None,
            secondary_volume: 1.0,
            secondary_muted: false,
            mic_noise_suppression: true,
            mic_noise_suppression_strength: 1.0,
            mic_noise_gate_db: Some(-50.0),
            sample_rate_hz: 48_000,
            channels: 2,
            aac_bitrate_kbps: 128,
            separate_tracks: true,
        }
    }
}

/// GOP-aware replay ring buffer settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReplayBufferSettings {
    pub enabled: bool,
    /// Replay window duration in seconds (`15..=300`).
    pub duration_secs: u32,
    /// Hard upper bound on compressed packet RAM usage in MiB.
    pub max_ram_mb: usize,
}

impl Default for ReplayBufferSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            duration_secs: 30,
            max_ram_mb: 512,
        }
    }
}

/// Global hotkey bindings persisted as string descriptors (e.g., `"F9"`, `"Ctrl+Shift+M"`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeySettings {
    pub save_clip: String,
    pub toggle_session: String,
    pub add_bookmark: String,
    pub toggle_mic_mute: String,
    pub toggle_privacy_mode: String,
}

impl Default for HotkeySettings {
    fn default() -> Self {
        Self {
            save_clip: "F9".to_string(),
            toggle_session: "F10".to_string(),
            add_bookmark: "F8".to_string(),
            toggle_mic_mute: "Ctrl+Shift+M".to_string(),
            toggle_privacy_mode: "Ctrl+Shift+P".to_string(),
        }
    }
}

/// Sound source selection for daemon audio notifications.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum SoundSource {
    Preset(String),
    CustomFile(PathBuf),
    Silent,
}

/// Per-event notification sound configuration (source + volume `0.0..=1.0`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotificationSoundEntry {
    pub source: SoundSource,
    pub volume: f32,
}

/// Notification sound mapping for all key recording lifecycle events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationSoundConfig {
    pub clip_saved: NotificationSoundEntry,
    pub recording_started: NotificationSoundEntry,
    pub recording_stopped: NotificationSoundEntry,
    pub marker_added: NotificationSoundEntry,
}

impl Default for NotificationSoundConfig {
    fn default() -> Self {
        Self {
            clip_saved: NotificationSoundEntry {
                source: SoundSource::Preset("chime".to_string()),
                volume: 0.7,
            },
            recording_started: NotificationSoundEntry {
                source: SoundSource::Preset("start".to_string()),
                volume: 0.65,
            },
            recording_stopped: NotificationSoundEntry {
                source: SoundSource::Preset("stop".to_string()),
                volume: 0.65,
            },
            marker_added: NotificationSoundEntry {
                source: SoundSource::Preset("tick".to_string()),
                volume: 0.6,
            },
        }
    }
}

/// Audio cue & transparent overlay toast settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationSettings {
    pub sound_enabled: bool,
    pub sound_volume: f32,
    pub sounds: NotificationSoundConfig,
    pub overlay_toast_enabled: bool,
    pub overlay_duration_ms: u32,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            sound_enabled: true,
            sound_volume: 0.7,
            sounds: NotificationSoundConfig::default(),
            overlay_toast_enabled: true,
            overlay_duration_ms: 2500,
        }
    }
}

/// Cleanup policy for heavy temporary editor files (`.m4a` stems and `.tmp` work files).
/// Light `.json` sidecars (bookmarks, favorites) and `.jpg` thumbnails are never deleted here.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheCleanupMode {
    #[default]
    OnClose,
    TimerAndLimit,
    Manual,
}

/// Configuration for automatic cleanup of heavy editor cache files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorCacheSettings {
    pub mode: CacheCleanupMode,
    pub interval_minutes: u32,
    pub max_size_mb: u64,
}

impl Default for EditorCacheSettings {
    fn default() -> Self {
        Self {
            mode: CacheCleanupMode::OnClose,
            interval_minutes: 30,
            max_size_mb: 250,
        }
    }
}

/// Output paths and crash-safe MP4 storage settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageSettings {
    pub output_dir: PathBuf,
    pub temp_dir: PathBuf,
    /// Remux finalized fMP4 segments into standard `faststart` MP4.
    pub auto_remux_fmp4: bool,
    pub max_storage_gb: Option<u64>,
    pub max_age_days: Option<u32>,
    #[serde(default)]
    pub discord_webhook_url: Option<String>,
    pub editor_cache: EditorCacheSettings,
}

impl Default for StorageSettings {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from("iKoch"),
            temp_dir: crate::metadata::ikoch_temp_dir(),
            auto_remux_fmp4: true,
            max_storage_gb: None,
            max_age_days: Some(30),
            discord_webhook_url: None,
            editor_cache: EditorCacheSettings::default(),
        }
    }
}
