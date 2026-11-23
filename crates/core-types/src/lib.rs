//! Core shared data contracts, settings, media packets, metadata, clock, and error types
//! for the iKoch recording pipeline and UI.

pub mod clock;
pub mod error;
pub mod events;
pub mod metadata;
pub mod packet;
pub mod settings;

pub use clock::{
    hns_to_us, timescale_to_us, us_to_hns, us_to_timescale, MonotonicClock,
    AUDIO_TIMESCALE_48KHZ, HNS_PER_MICROSECOND, VIDEO_TIMESCALE_90KHZ,
};
pub use error::{
    AudioError, CaptureError, CoreError, CoreResult, EncodeError, MuxError, SidecarError,
    StorageError,
};
pub use events::{
    ActiveSessionInfo, DaemonStateSnapshot, DetectedGameInfo, PipelineComponent, RecorderCommand,
    RecorderEvent, SoundNotificationKind, ToastKind, TrayIndicatorState,
};
pub use metadata::{
    ikoch_cache_root_dir, ikoch_logs_dir, ikoch_sidecars_dir, ikoch_temp_dir,
    ikoch_thumbnails_dir,
    AdvancedExportRequest, AspectRatioPreset, AudioTrackMetadata, ClipMetadata,
    DiscordCompressRequest, DiscordPreset, EditorCacheStats, FrameScreenshotRequest,
    GalleryFilter, MarkerSource, RecordingKind, SegmentRange, SessionMarker, TextOverlaySpec,
    TrackMixSpec, TrimAudioExportMode, TrimRequest,
};
pub use packet::{
    AudioStreamHeader, GpuFrameDescriptor, MediaPacket, PixelFormat, StreamHeaders, TrackKind,
    VideoStreamHeader, AUDIO_CHUNK_CHANNEL_CAPACITY, DISK_MUX_CHANNEL_CAPACITY,
    ENCODED_PACKET_CHANNEL_CAPACITY, VIDEO_FRAME_CHANNEL_CAPACITY,
};
pub use settings::{
    AppLanguage, AppSettings, AppTheme, AudioDeviceInfo, AudioOutputMode, AudioSettings,
    CacheCleanupMode, CaptureMode, CaptureSettings, CaptureTarget, CustomThemeColors,
    EditorCacheSettings, GameDetectionConfig, GpuVendorPreference, HdrMode, HotkeySettings,
    MicActivationMode, MicDeviceConfig, MonitorInfo, NotificationSettings, NotificationSoundConfig,
    NotificationSoundEntry, RateControlMode, ReplayBufferSettings, ResolutionPreset, SoundSource,
    StorageSettings, UiCornerRadius, UiDensity, UiSettings, VideoCodec, VideoSettings, WindowInfo,
    MAX_REPLAY_DURATION_SECS, MIN_REPLAY_DURATION_SECS, SETTINGS_SCHEMA_VERSION,
};

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::path::PathBuf;
    use std::sync::Arc;
    use uuid::Uuid;

    #[test]
    fn packet_rebasing_zeroes_idr_and_shares_payload_arc() {
        let packet = MediaPacket::new(
            TrackKind::Video,
            12_500_000,
            12_483_333,
            16_667,
            true,
            42,
            vec![0x00, 0x00, 0x01, 0x65, 0x88],
        );

        let rebased = packet.with_rebased_timestamps(12_500_000);
        assert_eq!(rebased.pts_us, 0);
        assert_eq!(rebased.dts_us, -16_667);
        assert_eq!(rebased.size_bytes(), 5);
        assert!(Arc::ptr_eq(&packet.payload, &rebased.payload));
    }

    #[test]
    fn discord_target_bitrate_formula_matches_spec() {
        // 10 MiB safe target (~10_171_187 bytes), 30s clip, 128 kbps audio
        let target_bytes = DiscordPreset::Mb10.target_bytes();
        let video_kbps =
            DiscordCompressRequest::calculate_target_video_bitrate_kbps(target_bytes, 30.0, 128);
        assert!(video_kbps.is_some());
        let kbps = video_kbps.unwrap_or(0);
        // (10_171_187 * 8 / 30) / 1000 - 128 = ~2584 kbps
        assert!((2500..=2700).contains(&kbps), "unexpected kbps: {kbps}");

        // Too long for 10 MiB -> None
        let impossible =
            DiscordCompressRequest::calculate_target_video_bitrate_kbps(target_bytes, 3600.0, 192);
        assert!(impossible.is_none());
    }

    #[test]
    fn settings_and_metadata_roundtrip_json() -> Result<(), serde_json::Error> {
        let mut settings = AppSettings::default();
        settings.replay_buffer.duration_secs = 999;
        settings.normalize();
        assert_eq!(settings.replay_buffer.duration_secs, MAX_REPLAY_DURATION_SECS);
        assert!(settings.estimated_ring_buffer_bytes() > 0);

        let json = serde_json::to_string(&settings)?;
        let parsed: AppSettings = serde_json::from_str(&json)?;
        assert_eq!(settings, parsed);

        let meta = ClipMetadata {
            id: Uuid::new_v4(),
            title: "Ace on Mirage".to_string(),
            game_name: "Counter-Strike 2".to_string(),
            executable_name: Some("cs2.exe".to_string()),
            kind: RecordingKind::Clip,
            file_path: PathBuf::from("clips/cs2_01.mp4"),
            thumbnail_path: None,
            waveform_path: None,
            created_at: Utc::now(),
            duration_sec: 30.5,
            file_size_bytes: 42_000_000,
            width: 1920,
            height: 1080,
            fps: 60,
            video_codec: VideoCodec::H264,
            video_bitrate_kbps: 20_000,
            audio_tracks: vec![
                AudioTrackMetadata {
                    track_index: 1,
                    kind: TrackKind::AudioGame,
                    label: "Game Audio".to_string(),
                    codec: "aac".to_string(),
                    sample_rate: 48_000,
                    channels: 2,
                    default_volume: 1.0,
                    cached_audio_path: None,
                },
                AudioTrackMetadata {
                    track_index: 2,
                    kind: TrackKind::AudioMic,
                    label: "Microphone".to_string(),
                    codec: "aac".to_string(),
                    sample_rate: 48_000,
                    channels: 2,
                    default_volume: 1.0,
                    cached_audio_path: None,
                },
            ],
            markers: vec![SessionMarker {
                id: Uuid::new_v4(),
                timestamp_sec: 12.4,
                label: "Bookmark".to_string(),
                source: MarkerSource::Hotkey,
                created_at: Utc::now(),
            }],
            favorite: false,
            is_edited: false,
            tags: vec!["fps".to_string()],
            crash_recovered: false,
        };

        let meta_json = serde_json::to_string(&meta)?;
        let meta_parsed: ClipMetadata = serde_json::from_str(&meta_json)?;
        assert_eq!(meta.id, meta_parsed.id);
        assert!(meta_parsed.markers.len() == 1);
        assert!(!meta_parsed.is_protected());
        Ok(())
    }

    #[test]
    fn clock_conversions_and_recoverable_classification() {
        assert_eq!(us_to_hns(1_000), 10_000);
        assert_eq!(hns_to_us(10_000), 1_000);
        assert_eq!(us_to_timescale(1_000_000, VIDEO_TIMESCALE_90KHZ), 90_000);
        assert_eq!(timescale_to_us(48_000, AUDIO_TIMESCALE_48KHZ), 1_000_000);

        let err = CoreError::Capture(CaptureError::DeviceRemoved {
            hresult: 0x887A_0005,
        });
        assert!(err.is_recoverable());

        let fatal = CoreError::Encode(EncodeError::HardwareEncoderUnavailable {
            codec: "H.264".to_string(),
        });
        assert!(!fatal.is_recoverable());
    }

    #[test]
    fn capture_target_and_command_roundtrip_json() -> Result<(), serde_json::Error> {
        let targets = vec![
            CaptureTarget::AutoGame,
            CaptureTarget::SpecificWindow {
                hwnd: 0x0012_3456,
                process_name: "cs2.exe".to_string(),
                title: "Counter-Strike 2".to_string(),
            },
            CaptureTarget::Monitor {
                monitor_id: r"\\.\DISPLAY1".to_string(),
                name: "Primary Monitor (1920x1080)".to_string(),
            },
        ];

        for target in targets {
            let cmd = RecorderCommand::SetCaptureTarget(target.clone());
            let json = serde_json::to_string(&cmd)?;
            let decoded: RecorderCommand = serde_json::from_str(&json)?;
            assert_eq!(cmd, decoded);

            let evt = RecorderEvent::CaptureTargetChanged { target };
            let evt_json = serde_json::to_string(&evt)?;
            let evt_decoded: RecorderEvent = serde_json::from_str(&evt_json)?;
            assert_eq!(evt, evt_decoded);
        }
        Ok(())
    }

    #[test]
    fn audio_and_notification_sound_contracts_roundtrip() -> Result<(), serde_json::Error> {
        let dev = AudioDeviceInfo {
            id: "{0.0.0.00000000}.{1234}".to_string(),
            name: "Headphones (HyperX Cloud)".to_string(),
            is_default: true,
        };
        let dev_json = serde_json::to_string(&dev)?;
        let dev_back: AudioDeviceInfo = serde_json::from_str(&dev_json)?;
        assert_eq!(dev, dev_back);

        let cmds = vec![
            RecorderCommand::SetAudioOutputMode(AudioOutputMode::DefaultDevice),
            RecorderCommand::SetAudioOutputMode(AudioOutputMode::SpecificDevice(dev.id.clone())),
            RecorderCommand::SetGameMuted { muted: true },
            RecorderCommand::ToggleGameMuted,
            RecorderCommand::SetMicDevice(MicDeviceConfig::None),
            RecorderCommand::SetMicDevice(MicDeviceConfig::SpecificDevice("mic-1".to_string())),
            RecorderCommand::SetSecondaryOutputDevice {
                device_id: Some("line-1".to_string()),
            },
            RecorderCommand::SetSecondaryOutputDevice { device_id: None },
            RecorderCommand::SetSecondaryMuted { muted: true },
            RecorderCommand::SetAudioVolumes {
                game_volume: 0.85,
                mic_volume: 1.15,
                secondary_volume: Some(0.9),
            },
            RecorderCommand::SetMicNoiseSuppression {
                enabled: true,
                strength: 0.8,
                noise_gate_db: Some(-48.0),
            },
            RecorderCommand::ToggleMicNoiseSuppression,
            RecorderCommand::SetMicActivationMode(MicActivationMode::PushToTalk {
                key: "V".to_string(),
                release_delay_ms: 200,
            }),
            RecorderCommand::SetMicSoftLimiter { enabled: true },
            RecorderCommand::SetPrivacyMode { enabled: true },
            RecorderCommand::TogglePrivacyMode,
            RecorderCommand::PlayTestSound {
                kind: SoundNotificationKind::ClipSaved,
            },
            RecorderCommand::RunStorageCleanup,
        ];
        for cmd in cmds {
            let j = serde_json::to_string(&cmd)?;
            let d: RecorderCommand = serde_json::from_str(&j)?;
            assert_eq!(cmd, d);
        }

        assert_eq!(TrackKind::Video.mp4_track_id(), 1);
        assert_eq!(TrackKind::AudioMixed.mp4_track_id(), 2);
        assert_eq!(TrackKind::AudioGame.mp4_track_id(), 3);
        assert_eq!(TrackKind::AudioMic.mp4_track_id(), 4);
        assert_eq!(TrackKind::AudioSecondary.mp4_track_id(), 5);
        assert!(TrackKind::AudioMixed.is_audio());
        assert!(TrackKind::AudioSecondary.is_audio());

        let mut settings = AppSettings::default();
        settings.audio.game_volume = 3.5;
        settings.audio.secondary_volume = -1.0;
        settings.audio.mic_noise_suppression_strength = 2.0;
        settings.audio.mic_noise_gate_db = Some(-5.0);
        settings.notifications.sounds.clip_saved = NotificationSoundEntry {
            source: SoundSource::CustomFile(PathBuf::from("sounds/ding.wav")),
            volume: 2.5,
        };
        settings.notifications.sounds.marker_added = NotificationSoundEntry {
            source: SoundSource::Silent,
            volume: -0.3,
        };
        settings.normalize();
        assert_eq!(settings.audio.game_volume, 2.0);
        assert_eq!(settings.audio.secondary_volume, 0.0);
        assert_eq!(settings.audio.mic_noise_suppression_strength, 1.0);
        assert_eq!(settings.audio.mic_noise_gate_db, Some(-10.0));
        assert_eq!(settings.notifications.sounds.clip_saved.volume, 1.0);
        assert_eq!(settings.notifications.sounds.marker_added.volume, 0.0);

        let sj = serde_json::to_string(&settings.notifications.sounds)?;
        let sb: NotificationSoundConfig = serde_json::from_str(&sj)?;
        assert_eq!(settings.notifications.sounds, sb);
        Ok(())
    }

    #[test]
    fn legacy_settings_json_deserializes_with_ferrix_defaults() -> Result<(), serde_json::Error> {
        let legacy_json = r#"{
            "schema_version": 1,
            "launch_at_startup": true,
            "storage": {
                "output_dir": "C:\\Videos\\RustClip",
                "auto_remux_fmp4": true
            }
        }"#;
        let mut parsed: AppSettings = serde_json::from_str(legacy_json)?;
        assert!(parsed.launch_at_startup);
        assert_eq!(parsed.language, AppLanguage::Ru);
        assert_eq!(parsed.theme, AppTheme::IkochDark);
        assert_eq!(parsed.theme.as_str(), "ikoch_dark");
        assert_eq!(parsed.custom_theme, CustomThemeColors::default());
        assert_eq!(parsed.storage.editor_cache.mode, CacheCleanupMode::OnClose);
        assert_eq!(parsed.storage.editor_cache.interval_minutes, 30);
        assert_eq!(parsed.storage.editor_cache.max_size_mb, 250);

        parsed.storage.editor_cache.interval_minutes = 1;
        parsed.storage.editor_cache.max_size_mb = 99_999;
        parsed.custom_theme.bg_app = "not-a-hex".to_string();
        parsed.custom_theme.accent = "#abcdef".to_string();
        parsed.normalize();
        assert_eq!(parsed.storage.editor_cache.interval_minutes, 5);
        assert_eq!(parsed.storage.editor_cache.max_size_mb, 4096);
        assert_eq!(parsed.custom_theme.bg_app, "#090B12");
        assert_eq!(parsed.custom_theme.accent, "#ABCDEF");

        parsed.language = AppLanguage::En;
        parsed.theme = AppTheme::Custom;
        let roundtrip: AppSettings = serde_json::from_str(&serde_json::to_string(&parsed)?)?;
        assert_eq!(roundtrip.language, AppLanguage::En);
        assert_eq!(roundtrip.theme, AppTheme::Custom);
        assert_eq!(AppTheme::SunsetAmber.as_str(), "sunset_amber");
        Ok(())
    }
}
