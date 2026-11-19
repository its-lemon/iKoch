//! Daemon control commands, pipeline status snapshots, and event bus contracts.

use crate::metadata::{ClipMetadata, SessionMarker};
use crate::settings::{
    AppSettings, AudioOutputMode, CaptureTarget, MicActivationMode, MicDeviceConfig,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// Subsystem identifier used in diagnostic events and recovery notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PipelineComponent {
    Capture,
    VideoProcessor,
    VideoEncoder,
    AudioLoopback,
    AudioMic,
    AudioEncoder,
    PacketRouter,
    RingBuffer,
    SessionMuxer,
    GameDetector,
    SidecarFfmpeg,
}

/// System tray icon visual state.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrayIndicatorState {
    #[default]
    Idle,
    Buffering,
    Recording,
    Error,
}

/// Metadata for the currently focused/captured game process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectedGameInfo {
    pub process_id: u32,
    pub executable_name: String,
    pub display_name: String,
    pub window_title: String,
    pub hwnd: isize,
}

/// Live telemetry for an ongoing full-session recording.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActiveSessionInfo {
    pub session_id: Uuid,
    pub started_at: DateTime<Utc>,
    pub elapsed_sec: f64,
    pub output_path: PathBuf,
    pub markers_count: usize,
    pub bytes_written: u64,
}

/// Snapshot of daemon and pipeline state pushed to the UI and tray icon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DaemonStateSnapshot {
    pub tray_state: TrayIndicatorState,
    pub capture_target: CaptureTarget,
    pub active_game: Option<DetectedGameInfo>,
    pub buffer_fill_sec: f64,
    pub buffer_capacity_sec: u32,
    pub buffer_bytes: usize,
    pub active_session: Option<ActiveSessionInfo>,
    pub dropped_video_frames: u64,
    pub dropped_audio_frames: u64,
    pub game_muted: bool,
    pub mic_muted: bool,
    pub secondary_muted: bool,
    pub mic_noise_suppression: bool,
    pub privacy_mode: bool,
}

impl Default for DaemonStateSnapshot {
    fn default() -> Self {
        Self {
            tray_state: TrayIndicatorState::Idle,
            capture_target: CaptureTarget::AutoGame,
            active_game: None,
            buffer_fill_sec: 0.0,
            buffer_capacity_sec: 30,
            buffer_bytes: 0,
            active_session: None,
            dropped_video_frames: 0,
            dropped_audio_frames: 0,
            game_muted: false,
            mic_muted: false,
            secondary_muted: false,
            mic_noise_suppression: true,
            privacy_mode: false,
        }
    }
}

/// Embedded audio cues triggered by `rodio` in `recorder-daemon`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SoundNotificationKind {
    ClipSaved,
    #[serde(alias = "session_started")]
    RecordingStarted,
    #[serde(alias = "session_stopped")]
    RecordingStopped,
    #[serde(alias = "bookmark_added")]
    MarkerAdded,
}

/// Visual severity for the transparent click-through overlay toast.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToastKind {
    Info,
    Success,
    Warning,
    Error,
}

/// Commands sent from Tauri IPC or global hotkeys to `recorder-daemon`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum RecorderCommand {
    StartBuffering,
    StopBuffering,
    SaveReplayClip,
    StartSession,
    StopSession,
    ToggleSession,
    AddBookmark { label: Option<String> },
    SetGameMuted { muted: bool },
    ToggleGameMuted,
    SetMicMuted { muted: bool },
    ToggleMicMuted,
    SetSecondaryOutputDevice { device_id: Option<String> },
    SetSecondaryMuted { muted: bool },
    SetAudioOutputMode(AudioOutputMode),
    SetMicDevice(MicDeviceConfig),
    SetAudioVolumes {
        game_volume: f32,
        mic_volume: f32,
        #[serde(default)]
        secondary_volume: Option<f32>,
    },
    SetMicNoiseSuppression {
        enabled: bool,
        strength: f32,
        noise_gate_db: Option<f32>,
    },
    ToggleMicNoiseSuppression,
    SetMicActivationMode(MicActivationMode),
    SetMicSoftLimiter { enabled: bool },
    SetPrivacyMode { enabled: bool },
    TogglePrivacyMode,
    PlayTestSound { kind: SoundNotificationKind },
    RunStorageCleanup,
    SetCaptureTarget(CaptureTarget),
    UpdateSettings(Box<AppSettings>),
    Shutdown,
}

/// Events published by `recorder-daemon` to Tauri UI, Tray controller, Sound cue player, and Toast overlay.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum RecorderEvent {
    StateChanged(DaemonStateSnapshot),
    CaptureTargetChanged {
        target: CaptureTarget,
    },
    GameDetected(DetectedGameInfo),
    GameLost {
        executable_name: String,
    },
    PrivacyModeChanged {
        enabled: bool,
    },
    StorageCleanupCompleted {
        deleted_files: usize,
        freed_bytes: u64,
    },
    ClipSaved(Box<ClipMetadata>),
    SessionStarted(ActiveSessionInfo),
    SessionStopped(Box<ClipMetadata>),
    BookmarkAdded {
        session_id: Option<Uuid>,
        marker: SessionMarker,
    },
    PlaySound(SoundNotificationKind),
    ShowOverlayToast {
        title: String,
        message: String,
        kind: ToastKind,
    },
    PipelineRecovering {
        component: PipelineComponent,
        reason: String,
    },
    PipelineError {
        component: PipelineComponent,
        message: String,
        fatal: bool,
    },
}
