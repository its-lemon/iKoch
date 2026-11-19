import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import type {
  AdvancedExportRequest,
  AppSettings,
  AudioDeviceInfo,
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
  SoundNotificationKind,
  TrimRequest,
  WindowInfo,
} from "./types";

export function toAssetUrl(filePath: string): string {
  return convertFileSrc(filePath);
}

export const api = {
  getDaemonState: () => invoke<DaemonStateSnapshot>("get_daemon_state"),

  getSettings: () => invoke<AppSettings>("get_settings"),

  saveSettings: (settings: AppSettings) =>
    invoke<AppSettings>("save_settings", { settings }),

  allowMediaDirectory: (dir: string) =>
    invoke<void>("allow_media_directory", { dir }),

  listMonitors: () => invoke<MonitorInfo[]>("list_monitors"),

  listWindows: () => invoke<WindowInfo[]>("list_windows"),

  listAudioOutputDevices: () =>
    invoke<AudioDeviceInfo[]>("list_audio_output_devices"),

  listAudioInputDevices: () =>
    invoke<AudioDeviceInfo[]>("list_audio_input_devices"),

  sendRecorderCommand: (command: RecorderCommand) =>
    invoke<void>("send_recorder_command", { command }),

  setCaptureTarget: (target: CaptureTarget) =>
    invoke<void>("set_capture_target", { target }),

  saveClipNow: () => invoke<void>("save_clip_now"),

  toggleSessionNow: () => invoke<void>("toggle_session_now"),

  addBookmarkNow: (label?: string | null) =>
    invoke<void>("add_bookmark_now", { label: label ?? null }),

  toggleMicMuteNow: () => invoke<void>("toggle_mic_mute_now"),

  togglePrivacyModeNow: () => invoke<void>("toggle_privacy_mode_now"),

  playTestSound: (kind: SoundNotificationKind) =>
    invoke<void>("play_test_sound", { kind }),

  listClips: (filter?: GalleryFilter | null) =>
    invoke<ClipMetadata[]>("list_clips", { filter: filter ?? null }),

  toggleClipFavorite: (videoPath: string) =>
    invoke<ClipMetadata>("toggle_clip_favorite", { videoPath }),

  renameClip: (videoPath: string, newTitle: string) =>
    invoke<ClipMetadata>("rename_clip", { videoPath, newTitle }),

  deleteClip: (videoPath: string) =>
    invoke<void>("delete_clip", { videoPath }),

  runStorageCleanupNow: () =>
    invoke<[number, number]>("run_storage_cleanup_now"),

  prepareEditorMedia: (videoPath: string, buckets?: number) =>
    invoke<PreparedEditorMedia>("prepare_editor_media", {
      videoPath,
      buckets: buckets ?? 180,
    }),

  captureFramePng: (req: FrameScreenshotRequest) =>
    invoke<string>("capture_frame_png", { req }),

  fastTrimClip: (req: TrimRequest) =>
    invoke<string>("fast_trim_clip", { req }),

  compressClipForDiscord: (req: DiscordCompressRequest, durationSec: number) =>
    invoke<string>("compress_clip_for_discord", { req, durationSec }),

  exportAdvancedClip: (req: AdvancedExportRequest) =>
    invoke<string>("export_advanced_clip", { req }),

  copyClipToClipboard: (path: string) =>
    invoke<void>("copy_clip_to_clipboard", { path }),

  sendToDiscordWebhook: (
    webhookUrl: string,
    filePath: string,
    content?: string | null
  ) =>
    invoke<void>("send_to_discord_webhook", {
      webhookUrl,
      filePath,
      content: content ?? null,
    }),

  openClipsFolder: () => invoke<void>("open_clips_folder"),

  revealClipInExplorer: (path: string) =>
    invoke<void>("reveal_clip_in_explorer", { path }),

  getEditorCacheStats: () =>
    invoke<EditorCacheStats>("get_editor_cache_stats"),

  clearEditorCacheNow: () =>
    invoke<[number, number]>("clear_editor_cache_now"),
};
