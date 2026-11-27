import type {
  ActiveAudioAppInfo,
  AdvancedExportRequest,
  AppSettings,
  AudioDeviceInfo,
  AudioTrackMetadata,
  CaptureTarget,
  ClipMetadata,
  DaemonStateSnapshot,
  DiscordCompressRequest,
  EditorCacheStats,
  FrameScreenshotRequest,
  GalleryFilter,
  MonitorInfo,
  PreparedEditorMedia,
  RecorderCommand,
  RecorderEvent,
  SessionMarker,
  SoundNotificationKind,
  TrackWaveform,
  TrimRequest,
  UpdateInfo,
  VideoCodec,
  WindowInfo,
} from "./types";
import { DEFAULT_CUSTOM_THEME, DEFAULT_UI_SETTINGS } from "./utils/theme";

type EventListener = (event: RecorderEvent) => void;
const listeners = new Set<EventListener>();

function emitEvent(event: RecorderEvent) {
  for (const cb of listeners) {
    cb(event);
  }
}

export function subscribeMockRecorderEvents(cb: EventListener): () => void {
  listeners.add(cb);
  return () => {
    listeners.delete(cb);
  };
}

let mockSettings: AppSettings = {
  schema_version: 2,
  launch_at_startup: true,
  check_updates_on_startup: true,
  language: "ru",
  theme: "ikoch_dark",
  custom_theme: { ...DEFAULT_CUSTOM_THEME },
  ui: { ...DEFAULT_UI_SETTINGS },
  capture: {
    target: { kind: "auto_game" },
    games: {
      auto_start: true,
      auto_pause_delay_secs: 5,
      use_builtin_game_list: true,
      custom_whitelist_exes: ["cs2.exe", "VALORANT-Win64-Shipping.exe"],
      blacklist_exes: ["wallpaper32.exe"],
    },
    capture_cursor: true,
    capture_border: false,
    hdr_mode: "auto_tone_map_to_sdr",
  },
  video: {
    codec: "h264",
    gpu_vendor: "nvidia_nvenc",
    resolution: { preset: "p1080" },
    fps: 60,
    bitrate_kbps: 6000,
    rate_control: "vbr",
    gop_seconds: 2,
  },
  audio: {
    audio_output_mode: { mode: "default_device" },
    mic_device: { mode: "default" },
    mic_activation_mode: { mode: "always_on" },
    mic_soft_limiter: true,
    game_enabled: true,
    game_device_id: "default_out",
    game_volume: 1.0,
    game_muted: false,
    mic_enabled: true,
    mic_device_id: "default_in",
    mic_volume: 1.25,
    mic_muted: false,
    secondary_output_device: null,
    secondary_volume: 1.0,
    secondary_muted: false,
    mic_noise_suppression: true,
    mic_noise_suppression_strength: 0.85,
    mic_noise_gate_db: -48,
    sample_rate_hz: 48000,
    channels: 2,
    aac_bitrate_kbps: 192,
    separate_tracks: true,
    apply_volumes_to_editor: true,
    app_audio_slots: [
      {
        id: "discord",
        label: "Discord",
        process_names: ["Discord.exe"],
        volume: 1.4,
        muted: false,
        enabled: true,
      },
      {
        id: "music",
        label: "Spotify / Music",
        process_names: ["Spotify.exe", "limusic-app.exe"],
        volume: 0.75,
        muted: false,
        enabled: true,
      },
    ],
  },
  replay_buffer: {
    enabled: true,
    duration_secs: 30,
    max_ram_mb: 120,
    reset_buffer_on_clip: false,
  },
  hotkeys: {
    save_clip: "F9",
    toggle_session: "F10",
    add_bookmark: "F8",
    toggle_mic_mute: "Ctrl+Shift+M",
    toggle_privacy_mode: "Ctrl+Shift+P",
  },
  notifications: {
    sound_enabled: true,
    sound_volume: 0.7,
    sounds: {
      clip_saved: { source: { kind: "preset", value: "chime" }, volume: 0.8 },
      recording_started: {
        source: { kind: "preset", value: "pop" },
        volume: 0.7,
      },
      recording_stopped: {
        source: { kind: "preset", value: "soft_down" },
        volume: 0.7,
      },
      marker_added: { source: { kind: "preset", value: "tick" }, volume: 0.6 },
    },
    overlay_toast_enabled: true,
    overlay_duration_ms: 2600,
  },
  storage: {
    output_dir: "C:\\Users\\Gamer\\Videos\\iKoch",
    temp_dir: "C:\\Users\\Gamer\\AppData\\Local\\iKoch\\cache",
    auto_remux_fmp4: true,
    max_storage_gb: 50,
    max_age_days: 30,
    discord_webhook_url: null,
    editor_cache: {
      mode: "timer_and_limit",
      interval_minutes: 30,
      max_size_mb: 512,
    },
  },
};

let mockDaemon: DaemonStateSnapshot = {
  tray_state: "buffering",
  capture_target: { kind: "auto_game" },
  active_game: {
    process_id: 14820,
    executable_name: "cs2.exe",
    display_name: "Counter-Strike 2",
    window_title: "Counter-Strike 2",
    hwnd: 98304,
  },
  buffer_fill_sec: 30,
  buffer_capacity_sec: 30,
  buffer_bytes: Math.round(22.4 * 1024 * 1024),
  active_session: null,
  dropped_video_frames: 0,
  dropped_audio_frames: 0,
  game_muted: false,
  mic_muted: false,
  secondary_muted: false,
  mic_noise_suppression: true,
  privacy_mode: false,
};

function makeDemoAudioTracks(): AudioTrackMetadata[] {
  return [
    {
      track_index: 1,
      kind: "audio_game",
      label: "Игра / Система",
      codec: "aac",
      sample_rate: 48000,
      channels: 2,
      default_volume: 1.0,
      cached_audio_path: null,
    },
    {
      track_index: 2,
      kind: "audio_mic",
      label: "Микрофон",
      codec: "aac",
      sample_rate: 48000,
      channels: 2,
      default_volume: 1.25,
      cached_audio_path: null,
    },
    {
      track_index: 3,
      kind: { audio_app: 0 },
      label: "Discord",
      codec: "aac",
      sample_rate: 48000,
      channels: 2,
      default_volume: 1.4,
      cached_audio_path: null,
    },
    {
      track_index: 4,
      kind: { audio_app: 1 },
      label: "Spotify / Music",
      codec: "aac",
      sample_rate: 48000,
      channels: 2,
      default_volume: 0.75,
      cached_audio_path: null,
    },
  ];
}

function makeMarker(id: string, timestamp_sec: number, label: string): SessionMarker {
  return {
    id,
    timestamp_sec,
    label,
    source: "hotkey",
    created_at: new Date().toISOString(),
  };
}

let mockClips: ClipMetadata[] = [
  {
    id: "clip-cs2-1",
    file_path: "demo://cs2-ace-mirage.mp4",
    title: "Mirage A-Site 4K One-Tap",
    game_name: "Counter-Strike 2",
    executable_name: "cs2.exe",
    kind: "clip",
    created_at: new Date(Date.now() - 25 * 60_000).toISOString(),
    duration_sec: 28.4,
    file_size_bytes: 58.2 * 1024 * 1024,
    width: 1920,
    height: 1080,
    fps: 60,
    video_codec: "h264",
    video_bitrate_kbps: 6000,
    audio_tracks: makeDemoAudioTracks(),
    markers: [
      makeMarker("m1", 11.2, "3x Headshot"),
      makeMarker("m2", 22.8, "Clutch"),
    ],
    favorite: true,
    thumbnail_path: "demo://cs2-ace-mirage.jpg",
    waveform_path: null,
    is_edited: false,
    tags: ["ace", "mirage"],
    crash_recovered: false,
  },
  {
    id: "clip-val-2",
    file_path: "demo://valorant-clutch.mp4",
    title: "Ascent 1v4 Spike Defuse",
    game_name: "VALORANT",
    executable_name: "VALORANT-Win64-Shipping.exe",
    kind: "clip",
    created_at: new Date(Date.now() - 3 * 3600_000).toISOString(),
    duration_sec: 24.0,
    file_size_bytes: 46.5 * 1024 * 1024,
    width: 1920,
    height: 1080,
    fps: 60,
    video_codec: "h264",
    video_bitrate_kbps: 6000,
    audio_tracks: makeDemoAudioTracks(),
    markers: [makeMarker("m3", 16.5, "Ace")],
    favorite: false,
    thumbnail_path: "demo://valorant-clutch.jpg",
    waveform_path: null,
    is_edited: true,
    tags: ["clutch"],
    crash_recovered: false,
  },
  {
    id: "clip-apex-3",
    file_path: "demo://apex-movement.mp4",
    title: "World's Edge Superglide Wipe",
    game_name: "Apex Legends",
    executable_name: "r5apex.exe",
    kind: "session",
    created_at: new Date(Date.now() - 26 * 3600_000).toISOString(),
    duration_sec: 42.5,
    file_size_bytes: 91.0 * 1024 * 1024,
    width: 2560,
    height: 1440,
    fps: 120,
    video_codec: "h264",
    video_bitrate_kbps: 8000,
    audio_tracks: makeDemoAudioTracks(),
    markers: [],
    favorite: false,
    thumbnail_path: "demo://apex-movement.jpg",
    waveform_path: null,
    is_edited: false,
    tags: [],
    crash_recovered: false,
  },
  {
    id: "clip-elden-4",
    file_path: "demo://elden-ring-malenia.mp4",
    title: "Malenia Phase 2 No-Hit",
    game_name: "Elden Ring",
    executable_name: "eldenring.exe",
    kind: "clip",
    created_at: new Date(Date.now() - 54 * 3600_000).toISOString(),
    duration_sec: 30.0,
    file_size_bytes: 64.8 * 1024 * 1024,
    width: 1920,
    height: 1080,
    fps: 60,
    video_codec: "h264",
    video_bitrate_kbps: 6000,
    audio_tracks: makeDemoAudioTracks(),
    markers: [makeMarker("m4", 24.1, "Finisher")],
    favorite: true,
    thumbnail_path: "demo://elden-ring-malenia.jpg",
    waveform_path: null,
    is_edited: false,
    tags: ["boss"],
    crash_recovered: false,
  },
];

let mockCacheStats: EditorCacheStats = {
  temp_bytes: 184 * 1024 * 1024,
  temp_file_count: 14,
  metadata_bytes: 1.8 * 1024 * 1024,
  metadata_file_count: 4,
};

let sessionTimerId: number | null = null;

function buildDemoPeaks(buckets: number, seed: number): number[] {
  const peaks: number[] = [];
  for (let i = 0; i < buckets; i++) {
    const wave =
      Math.abs(Math.sin(i * 0.27 + seed) * 0.55) +
      Math.abs(Math.cos(i * 0.63 + seed * 1.7) * 0.35);
    peaks.push(Math.min(1, Math.max(0.06, Number(wave.toFixed(3)))));
  }
  return peaks;
}

export function toAssetUrl(filePath: string): string {
  return filePath;
}

export const api = {
  getDaemonState: async (): Promise<DaemonStateSnapshot> => ({ ...mockDaemon }),

  getSettings: async (): Promise<AppSettings> =>
    JSON.parse(JSON.stringify(mockSettings)) as AppSettings,

  saveSettings: async (settings: AppSettings): Promise<AppSettings> => {
    mockSettings = JSON.parse(JSON.stringify(settings)) as AppSettings;
    mockDaemon = {
      ...mockDaemon,
      buffer_capacity_sec: mockSettings.replay_buffer.duration_secs,
      buffer_fill_sec: mockSettings.replay_buffer.enabled
        ? mockSettings.replay_buffer.duration_secs
        : 0,
      mic_noise_suppression: mockSettings.audio.mic_noise_suppression,
      mic_muted: mockSettings.audio.mic_muted,
    };
    emitEvent({ event: "state_changed", ...mockDaemon });
    return JSON.parse(JSON.stringify(mockSettings)) as AppSettings;
  },

  allowMediaDirectory: async (_dir: string): Promise<void> => {},

  listMonitors: async (): Promise<MonitorInfo[]> => [
    {
      monitor_id: "DISPLAY1",
      name: "Monitor 1 (2560x1440 @ 165Hz)",
      hmonitor: 1,
      width: 2560,
      height: 1440,
      x: 0,
      y: 0,
      is_primary: true,
    },
    {
      monitor_id: "DISPLAY2",
      name: "Monitor 2 (1920x1080 @ 60Hz)",
      hmonitor: 2,
      width: 1920,
      height: 1080,
      x: 2560,
      y: 0,
      is_primary: false,
    },
  ],

  listWindows: async (): Promise<WindowInfo[]> => [
    {
      hwnd: 98304,
      process_id: 14820,
      process_name: "cs2.exe",
      title: "Counter-Strike 2",
      width: 1920,
      height: 1080,
      is_minimized: false,
    },
    {
      hwnd: 104422,
      process_id: 8240,
      process_name: "Discord.exe",
      title: "Discord",
      width: 1280,
      height: 800,
      is_minimized: false,
    },
  ],

  listAudioOutputDevices: async (): Promise<AudioDeviceInfo[]> => [
    {
      id: "default_out",
      name: "Динамики (Realtek High Definition Audio)",
      is_default: true,
    },
    {
      id: "headphones_out",
      name: "HyperX Cloud II Wireless (Game)",
      is_default: false,
    },
  ],

  listAudioInputDevices: async (): Promise<AudioDeviceInfo[]> => [
    {
      id: "default_in",
      name: "Микрофон (HyperX QuadCast S)",
      is_default: true,
    },
    {
      id: "webcam_in",
      name: "Logitech Brio Mic",
      is_default: false,
    },
  ],

  listActiveAudioApps: async (): Promise<ActiveAudioAppInfo[]> => [
    { pid: 14820, exe_name: "cs2.exe", is_active: true, peak: 0.72 },
    { pid: 8240, exe_name: "Discord.exe", is_active: true, peak: 0.48 },
    { pid: 19210, exe_name: "Spotify.exe", is_active: true, peak: 0.35 },
    { pid: 21540, exe_name: "limusic-app.exe", is_active: true, peak: 0.29 },
  ],

  sendRecorderCommand: async (_command: RecorderCommand): Promise<void> => {},

  setCaptureTarget: async (target: CaptureTarget): Promise<void> => {
    mockDaemon = {
      ...mockDaemon,
      capture_target: target,
      tray_state: mockDaemon.active_session ? "recording" : "buffering",
    };
    emitEvent({ event: "state_changed", ...mockDaemon });
  },

  saveClipNow: async (): Promise<void> => {
    if (mockDaemon.privacy_mode) return;
    const gameName = mockDaemon.active_game?.display_name ?? "Counter-Strike 2";
    const id = Date.now();
    const newClip: ClipMetadata = {
      id: `clip-${id}`,
      file_path: `demo://clip-${id}.mp4`,
      title: `${gameName} — Instant Replay`,
      game_name: gameName,
      executable_name: mockDaemon.active_game?.executable_name ?? "cs2.exe",
      kind: "clip",
      created_at: new Date().toISOString(),
      duration_sec: mockDaemon.buffer_capacity_sec,
      file_size_bytes: Math.round(22.4 * 1024 * 1024),
      width: 1920,
      height: 1080,
      fps: 60,
      video_codec: "h264",
      video_bitrate_kbps: 6000,
      audio_tracks: makeDemoAudioTracks(),
      markers: [],
      favorite: false,
      thumbnail_path: `demo://clip-${id}.jpg`,
      waveform_path: null,
      is_edited: false,
      tags: [],
      crash_recovered: false,
    };
    mockClips = [newClip, ...mockClips];
    emitEvent({ event: "clip_saved", ...newClip });
  },

  toggleSessionNow: async (): Promise<void> => {
    if (mockDaemon.active_session) {
      if (sessionTimerId !== null) {
        window.clearInterval(sessionTimerId);
        sessionTimerId = null;
      }
      const dur = Math.max(4, mockDaemon.active_session.elapsed_sec);
      const gameName = mockDaemon.active_game?.display_name ?? "Counter-Strike 2";
      const id = Date.now();
      const newClip: ClipMetadata = {
        id: `session-${id}`,
        file_path: `demo://session-${id}.mp4`,
        title: `${gameName} — Запись сессии`,
        game_name: gameName,
        executable_name: mockDaemon.active_game?.executable_name ?? "cs2.exe",
        kind: "session",
        created_at: new Date().toISOString(),
        duration_sec: dur,
        file_size_bytes: Math.round(dur * 0.8 * 1024 * 1024),
        width: 1920,
        height: 1080,
        fps: 60,
        video_codec: "h264",
        video_bitrate_kbps: 6000,
        audio_tracks: makeDemoAudioTracks(),
        markers: [],
        favorite: false,
        thumbnail_path: `demo://session-${id}.jpg`,
        waveform_path: null,
        is_edited: false,
        tags: [],
        crash_recovered: false,
      };
      mockClips = [newClip, ...mockClips];
      mockDaemon = {
        ...mockDaemon,
        tray_state: "buffering",
        active_session: null,
      };
      emitEvent({ event: "state_changed", ...mockDaemon });
      emitEvent({ event: "session_stopped", ...newClip });
    } else {
      mockDaemon = {
        ...mockDaemon,
        tray_state: "recording",
        active_session: {
          session_id: `sess-${Date.now()}`,
          started_at: new Date().toISOString(),
          elapsed_sec: 0,
          output_path: "demo://live-session.mp4",
          markers_count: 0,
          bytes_written: 0,
        },
      };
      emitEvent({ event: "state_changed", ...mockDaemon });
      sessionTimerId = window.setInterval(() => {
        if (!mockDaemon.active_session) return;
        mockDaemon = {
          ...mockDaemon,
          active_session: {
            ...mockDaemon.active_session,
            elapsed_sec: mockDaemon.active_session.elapsed_sec + 1,
            bytes_written:
              mockDaemon.active_session.bytes_written + 2.2 * 1024 * 1024,
          },
        };
        emitEvent({ event: "state_changed", ...mockDaemon });
      }, 1000);
    }
  },

  addBookmarkNow: async (label?: string | null): Promise<void> => {
    if (mockDaemon.active_session) {
      mockDaemon = {
        ...mockDaemon,
        active_session: {
          ...mockDaemon.active_session,
          markers_count: mockDaemon.active_session.markers_count + 1,
        },
      };
      emitEvent({ event: "state_changed", ...mockDaemon });
    }
    emitEvent({
      event: "bookmark_added",
      session_id: mockDaemon.active_session?.session_id ?? null,
      marker: makeMarker(
        `bm-${Date.now()}`,
        mockDaemon.active_session?.elapsed_sec ?? 14.5,
        label ?? "Highlight"
      ),
    });
  },

  toggleMicMuteNow: async (): Promise<void> => {
    mockDaemon = { ...mockDaemon, mic_muted: !mockDaemon.mic_muted };
    emitEvent({ event: "state_changed", ...mockDaemon });
  },

  togglePrivacyModeNow: async (): Promise<void> => {
    mockDaemon = { ...mockDaemon, privacy_mode: !mockDaemon.privacy_mode };
    emitEvent({ event: "state_changed", ...mockDaemon });
  },

  playTestSound: async (_kind: SoundNotificationKind): Promise<void> => {},

  listClips: async (_filter?: GalleryFilter | null): Promise<ClipMetadata[]> => [
    ...mockClips,
  ],

  toggleClipFavorite: async (videoPath: string): Promise<ClipMetadata> => {
    let updated: ClipMetadata | undefined;
    mockClips = mockClips.map((c) => {
      if (c.file_path === videoPath) {
        updated = { ...c, favorite: !c.favorite };
        return updated;
      }
      return c;
    });
    if (!updated) throw new Error("Clip not found");
    return updated;
  },

  renameClip: async (
    videoPath: string,
    newTitle: string
  ): Promise<ClipMetadata> => {
    let updated: ClipMetadata | undefined;
    mockClips = mockClips.map((c) => {
      if (c.file_path === videoPath) {
        updated = { ...c, title: newTitle };
        return updated;
      }
      return c;
    });
    if (!updated) throw new Error("Clip not found");
    return updated;
  },

  deleteClip: async (videoPath: string): Promise<void> => {
    mockClips = mockClips.filter((c) => c.file_path !== videoPath);
  },

  runStorageCleanupNow: async (): Promise<[number, number]> => [0, 0],

  prepareEditorMedia: async (
    videoPath: string,
    buckets = 180
  ): Promise<PreparedEditorMedia> => {
    const clip = mockClips.find((c) => c.file_path === videoPath) ?? mockClips[0];
    const waveforms: TrackWaveform[] = [
      {
        stream_ordinal: 1,
        kind: "audio_game",
        label: "Игра / Система",
        peaks: buildDemoPeaks(buckets, 1.2),
        stem_audio_path: "",
      },
      {
        stream_ordinal: 2,
        kind: "audio_mic",
        label: "Микрофон",
        peaks: buildDemoPeaks(buckets, 3.1),
        stem_audio_path: "",
      },
      {
        stream_ordinal: 3,
        kind: { audio_app: 0 },
        label: "Discord",
        peaks: buildDemoPeaks(buckets, 5.4),
        stem_audio_path: "",
      },
      {
        stream_ordinal: 4,
        kind: { audio_app: 1 },
        label: "Spotify / Music",
        peaks: buildDemoPeaks(buckets, 7.8),
        stem_audio_path: "",
      },
    ];
    return {
      thumbnail_path: clip.thumbnail_path ?? "",
      probed: {
        duration_sec: clip.duration_sec,
        width: clip.width,
        height: clip.height,
        video_codec: "h264",
        audio_streams: [],
      },
      waveforms,
    };
  },

  captureFramePng: async (req: FrameScreenshotRequest): Promise<string> =>
    req.output_png_path,

  fastTrimClip: async (req: TrimRequest): Promise<string> => req.output_path,

  replaceClipFastTrim: async (req: TrimRequest): Promise<string> =>
    req.source_path,

  compressClipForDiscord: async (
    req: DiscordCompressRequest,
    _durationSec: number
  ): Promise<string> => req.output_path,

  exportAdvancedClip: async (req: AdvancedExportRequest): Promise<string> =>
    req.output_path,

  replaceClipAdvanced: async (req: AdvancedExportRequest): Promise<string> =>
    req.source_path,

  copyClipToClipboard: async (_path: string): Promise<void> => {},

  sendToDiscordWebhook: async (
    _webhookUrl: string,
    _filePath: string,
    _content?: string | null
  ): Promise<void> => {},

  openClipsFolder: async (): Promise<void> => {},

  revealClipInExplorer: async (_path: string): Promise<void> => {},

  getEditorCacheStats: async (): Promise<EditorCacheStats> => ({
    ...mockCacheStats,
  }),

  clearEditorCacheNow: async (): Promise<[number, number]> => {
    const freed = mockCacheStats.temp_bytes;
    const count = mockCacheStats.temp_file_count;
    mockCacheStats = {
      ...mockCacheStats,
      temp_bytes: 0,
      temp_file_count: 0,
    };
    return [count, freed];
  },

  pickAudioFile: async (): Promise<string | null> =>
    "C:\\Users\\Gamer\\Music\\custom_notify.wav",

  getSupportedCodecs: async (): Promise<VideoCodec[]> => ["h264", "hevc", "av1"],

  checkForUpdates: async (): Promise<UpdateInfo> => ({
    has_update: false,
    current_version: __IKOCH_VERSION__,
    latest_version: __IKOCH_VERSION__,
    release_notes: "Zero-Copy GPU & WebView2 memory optimizations.",
    download_url: null,
    installer_name: null,
    html_url: "https://github.com/its-lemon/iKoch/releases/latest",
  }),

  downloadAndInstallUpdate: async (
    _downloadUrl: string,
    _installerName?: string | null
  ): Promise<void> => {},
};
