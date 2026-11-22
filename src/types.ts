export type CaptureTarget =
  | { kind: "auto_game" }
  | {
      kind: "specific_window";
      hwnd: number;
      process_name: string;
      title: string;
    }
  | { kind: "monitor"; monitor_id: string; name: string };

export interface MonitorInfo {
  monitor_id: string;
  name: string;
  hmonitor: number;
  width: number;
  height: number;
  x: number;
  y: number;
  is_primary: boolean;
}

export interface WindowInfo {
  hwnd: number;
  process_id: number;
  process_name: string;
  title: string;
  width: number;
  height: number;
  is_minimized: boolean;
}

export interface AudioDeviceInfo {
  id: string;
  name: string;
  is_default: boolean;
}

export type TrayIndicatorState = "idle" | "buffering" | "recording" | "error";

export interface DetectedGameInfo {
  process_id: number;
  executable_name: string;
  display_name: string;
  window_title: string;
  hwnd: number;
}

export interface ActiveSessionInfo {
  session_id: string;
  started_at: string;
  elapsed_sec: number;
  output_path: string;
  markers_count: number;
  bytes_written: number;
}

export interface DaemonStateSnapshot {
  tray_state: TrayIndicatorState;
  capture_target: CaptureTarget;
  active_game: DetectedGameInfo | null;
  buffer_fill_sec: number;
  buffer_capacity_sec: number;
  buffer_bytes: number;
  active_session: ActiveSessionInfo | null;
  dropped_video_frames: number;
  dropped_audio_frames: number;
  game_muted: boolean;
  mic_muted: boolean;
  secondary_muted: boolean;
  mic_noise_suppression: boolean;
  privacy_mode: boolean;
}

export type HdrMode =
  | "auto_tone_map_to_sdr"
  | "force_sdr_clamp"
  | "preserve_hdr10";

export type VideoCodec = "h264" | "hevc" | "av1";

export type GpuVendorPreference =
  | "auto"
  | "nvidia_nvenc"
  | "amd_amf"
  | "intel_qsv"
  | "software_fallback";

export type ResolutionPreset =
  | { preset: "native" }
  | { preset: "p1080" }
  | { preset: "p720" }
  | { preset: "custom"; width: number; height: number };

export type RateControlMode = "cbr" | "vbr" | "cqp";

export interface GameDetectionConfig {
  auto_start: boolean;
  auto_pause_delay_secs: number;
  use_builtin_game_list: boolean;
  custom_whitelist_exes: string[];
  blacklist_exes: string[];
}

export interface CaptureSettings {
  target: CaptureTarget;
  games: GameDetectionConfig;
  capture_cursor: boolean;
  capture_border: boolean;
  hdr_mode: HdrMode;
}

export interface VideoSettings {
  codec: VideoCodec;
  gpu_vendor: GpuVendorPreference;
  resolution: ResolutionPreset;
  fps: number;
  bitrate_kbps: number;
  rate_control: RateControlMode;
  gop_seconds: number;
}

export type AudioOutputMode =
  | { mode: "default_device" }
  | { mode: "specific_device"; device_id: string }
  | { mode: "game_process_only" };

export type MicDeviceConfig =
  | { mode: "none" }
  | { mode: "default" }
  | { mode: "specific_device"; device_id: string };

export type MicActivationMode =
  | { mode: "always_on" }
  | { mode: "push_to_talk"; key: string; release_delay_ms: number }
  | { mode: "push_to_mute"; key: string; release_delay_ms: number };

export interface AudioSettings {
  audio_output_mode: AudioOutputMode;
  mic_device: MicDeviceConfig;
  mic_activation_mode: MicActivationMode;
  mic_soft_limiter: boolean;
  game_enabled: boolean;
  game_device_id: string | null;
  game_volume: number;
  game_muted: boolean;
  mic_enabled: boolean;
  mic_device_id: string | null;
  mic_volume: number;
  mic_muted: boolean;
  secondary_output_device: string | null;
  secondary_volume: number;
  secondary_muted: boolean;
  mic_noise_suppression: boolean;
  mic_noise_suppression_strength: number;
  mic_noise_gate_db: number | null;
  sample_rate_hz: number;
  channels: number;
  aac_bitrate_kbps: number;
  separate_tracks: boolean;
}

export interface ReplayBufferSettings {
  enabled: boolean;
  duration_secs: number;
  max_ram_mb: number;
}

export interface HotkeySettings {
  save_clip: string;
  toggle_session: string;
  add_bookmark: string;
  toggle_mic_mute: string;
  toggle_privacy_mode: string;
}

export type SoundSource =
  | { kind: "preset"; value: string }
  | { kind: "custom_file"; value: string }
  | { kind: "silent" };

export interface NotificationSoundEntry {
  source: SoundSource;
  volume: number;
}

export interface NotificationSoundConfig {
  clip_saved: NotificationSoundEntry;
  recording_started: NotificationSoundEntry;
  recording_stopped: NotificationSoundEntry;
  marker_added: NotificationSoundEntry;
}

export interface NotificationSettings {
  sound_enabled: boolean;
  sound_volume: number;
  sounds: NotificationSoundConfig;
  overlay_toast_enabled: boolean;
  overlay_duration_ms: number;
}

export type AppLanguage = "ru" | "en";

export type AppTheme =
  | "ikoch_dark"
  | "oled_midnight"
  | "crimson_pro"
  | "nordic_slate"
  | "amethyst_neon"
  | "sunset_amber"
  | "emerald_matrix"
  | "frost_slate"
  | "custom";

export interface CustomThemeColors {
  bg_app: string;
  bg_panel: string;
  accent: string;
  text_primary: string;
}

export type CacheCleanupMode = "on_close" | "timer_and_limit" | "manual";

export interface EditorCacheSettings {
  mode: CacheCleanupMode;
  interval_minutes: number;
  max_size_mb: number;
}

export interface EditorCacheStats {
  temp_bytes: number;
  temp_file_count: number;
  metadata_bytes: number;
  metadata_file_count: number;
}

export interface StorageSettings {
  output_dir: string;
  temp_dir: string;
  auto_remux_fmp4: boolean;
  max_storage_gb: number | null;
  max_age_days: number | null;
  discord_webhook_url: string | null;
  editor_cache: EditorCacheSettings;
}

export interface AppSettings {
  schema_version: number;
  launch_at_startup: boolean;
  language: AppLanguage;
  theme: AppTheme;
  custom_theme: CustomThemeColors;
  capture: CaptureSettings;
  video: VideoSettings;
  audio: AudioSettings;
  replay_buffer: ReplayBufferSettings;
  hotkeys: HotkeySettings;
  notifications: NotificationSettings;
  storage: StorageSettings;
}

export type SoundNotificationKind =
  | "clip_saved"
  | "recording_started"
  | "recording_stopped"
  | "marker_added";

export type RecordingKind = "clip" | "session";

export type MarkerSource = "hotkey" | "manual_ui" | "system_event";

export interface SessionMarker {
  id: string;
  timestamp_sec: number;
  label: string;
  source: MarkerSource;
  created_at: string;
}

export type TrackKind =
  | "video"
  | "audio_mixed"
  | "audio_game"
  | "audio_mic"
  | "audio_secondary";

export interface AudioTrackMetadata {
  track_index: number;
  kind: TrackKind;
  label: string;
  codec: string;
  sample_rate: number;
  channels: number;
  default_volume: number;
  cached_audio_path: string | null;
}

export interface ClipMetadata {
  id: string;
  title: string;
  game_name: string;
  executable_name: string | null;
  kind: RecordingKind;
  file_path: string;
  thumbnail_path: string | null;
  waveform_path: string | null;
  created_at: string;
  duration_sec: number;
  file_size_bytes: number;
  width: number;
  height: number;
  fps: number;
  video_codec: VideoCodec;
  video_bitrate_kbps: number;
  audio_tracks: AudioTrackMetadata[];
  markers: SessionMarker[];
  favorite: boolean;
  is_edited: boolean;
  tags: string[];
  crash_recovered: boolean;
}

export interface GalleryFilter {
  kind?: RecordingKind | null;
  game_name?: string | null;
  favorites_only: boolean;
  search_query?: string | null;
}

export interface ProbedAudioStream {
  stream_index: number;
  ordinal: number;
  track_kind: TrackKind;
  label: string;
  codec_name: string;
  sample_rate: number;
  channels: number;
}

export interface ProbedMediaInfo {
  duration_sec: number;
  width: number;
  height: number;
  video_codec: string;
  audio_streams: ProbedAudioStream[];
}

export interface TrackWaveform {
  stream_ordinal: number;
  kind: TrackKind;
  label: string;
  peaks: number[];
  stem_audio_path: string;
}

export interface PreparedEditorMedia {
  thumbnail_path: string;
  probed: ProbedMediaInfo;
  waveforms: TrackWaveform[];
}

export type TrimAudioExportMode =
  | "keep_separate_tracks"
  | "mix_to_single_track"
  | "game_track_only"
  | "mic_track_only";

export interface TrimRequest {
  clip_id: string;
  source_path: string;
  output_path: string;
  start_sec: number;
  end_sec: number;
  audio_mode: TrimAudioExportMode;
  game_volume: number;
  mic_volume: number;
}

export type DiscordPreset =
  | { preset: "mb10" }
  | { preset: "mb50" }
  | { preset: "custom_mb"; size_mb: number };

export interface DiscordCompressRequest {
  clip_id: string;
  source_path: string;
  output_path: string;
  preset: DiscordPreset;
  start_sec: number | null;
  end_sec: number | null;
  audio_bitrate_kbps: number;
  mix_audio_tracks: boolean;
  game_volume: number;
  mic_volume: number;
}

export interface SegmentRange {
  start_sec: number;
  end_sec: number;
}

export type AspectRatioPreset =
  | "original"
  | "vertical9x16_blurred"
  | "square1x1";

export interface TextOverlaySpec {
  text: string;
  x_norm: number;
  y_norm: number;
  font_size: number;
  color_hex: string;
  start_sec: number | null;
  end_sec: number | null;
}

export interface TrackMixSpec {
  kind: TrackKind;
  volume: number;
  muted: boolean;
}

export interface AdvancedExportRequest {
  clip_id: string;
  source_path: string;
  output_path: string;
  segments: SegmentRange[];
  playback_speed: number;
  aspect_ratio: AspectRatioPreset;
  text_overlays: TextOverlaySpec[];
  fade_in_sec: number;
  fade_out_sec: number;
  track_mix: TrackMixSpec[];
  discord_preset: DiscordPreset | null;
  export_gif: boolean;
  subclip_from_session: boolean;
  discord_webhook_url: string | null;
}

export interface FrameScreenshotRequest {
  clip_id: string;
  source_path: string;
  output_png_path: string;
  timestamp_sec: number;
}

export interface MergeClipItem {
  source_path: string;
  start_sec: number;
  end_sec: number;
}

export interface MergeClipsRequest {
  items: MergeClipItem[];
  output_path: string;
  playback_speed: number;
  fade_in_sec: number;
  fade_out_sec: number;
}

export type RecorderCommand =
  | { cmd: "start_buffering" }
  | { cmd: "stop_buffering" }
  | { cmd: "save_replay_clip" }
  | { cmd: "start_session" }
  | { cmd: "stop_session" }
  | { cmd: "toggle_session" }
  | { cmd: "add_bookmark"; label: string | null }
  | { cmd: "set_game_muted"; muted: boolean }
  | { cmd: "toggle_game_muted" }
  | { cmd: "set_mic_muted"; muted: boolean }
  | { cmd: "toggle_mic_muted" }
  | { cmd: "set_secondary_output_device"; device_id: string | null }
  | { cmd: "set_secondary_muted"; muted: boolean }
  | ({ cmd: "set_audio_output_mode" } & AudioOutputMode)
  | ({ cmd: "set_mic_device" } & MicDeviceConfig)
  | {
      cmd: "set_audio_volumes";
      game_volume: number;
      mic_volume: number;
      secondary_volume?: number | null;
    }
  | {
      cmd: "set_mic_noise_suppression";
      enabled: boolean;
      strength: number;
      noise_gate_db: number | null;
    }
  | { cmd: "toggle_mic_noise_suppression" }
  | ({ cmd: "set_mic_activation_mode" } & MicActivationMode)
  | { cmd: "set_mic_soft_limiter"; enabled: boolean }
  | { cmd: "set_privacy_mode"; enabled: boolean }
  | { cmd: "toggle_privacy_mode" }
  | { cmd: "play_test_sound"; kind: SoundNotificationKind }
  | { cmd: "run_storage_cleanup" }
  | ({ cmd: "update_settings" } & AppSettings);

export type ToastSeverity = "info" | "success" | "warning" | "error";

export type RecorderEvent =
  | ({ event: "state_changed" } & DaemonStateSnapshot)
  | { event: "capture_target_changed"; target: CaptureTarget }
  | ({ event: "game_detected" } & DetectedGameInfo)
  | { event: "game_lost"; executable_name: string }
  | { event: "privacy_mode_changed"; enabled: boolean }
  | {
      event: "storage_cleanup_completed";
      deleted_files: number;
      freed_bytes: number;
    }
  | ({ event: "clip_saved" } & ClipMetadata)
  | ({ event: "session_started" } & ActiveSessionInfo)
  | ({ event: "session_stopped" } & ClipMetadata)
  | {
      event: "bookmark_added";
      session_id: string | null;
      marker: SessionMarker;
    }
  | {
      event: "show_overlay_toast";
      title: string;
      message: string;
      kind: ToastSeverity;
    }
  | { event: "pipeline_recovering"; component: string; reason: string }
  | {
      event: "pipeline_error";
      component: string;
      message: string;
      fatal: boolean;
    };
