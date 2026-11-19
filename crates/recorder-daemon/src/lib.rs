//! Stage 4 Background Recording Daemon:
//! - Foreground game auto-detection (`AutoGame`) & hotkey listening (`global-hotkey`)
//! - Embedded & custom WAV/MP3/OGG audio cues (`rodio`)
//! - Coordinated zero-copy video + multi-track AAC audio muxing with Privacy Mode & `F8` bookmarks
//! - Persistent `.json` sidecars (`ClipMetadata`) & Storage Quota eviction protecting favorites/edits

pub mod daemon;
pub mod game_detector;
pub mod hotkeys;
pub mod sounds;
pub mod storage;

pub use core_types::{
    AppSettings, ClipMetadata, DaemonStateSnapshot, EditorCacheStats, RecorderCommand,
    RecorderEvent,
};
pub use daemon::{DaemonError, RecorderDaemon};
pub use game_detector::{classify_executable, GameDetectionTransition, GameDetector};
pub use hotkeys::{parse_hotkey_binding, GlobalHotkeyService, HotkeyAction};
pub use sounds::{verify_embedded_presets_decode, SoundCuePlayer};
pub use storage::{
    apply_temp_cache_policy, cleanup_temp_dir, list_clip_metadata, load_clip_metadata,
    measure_editor_cache, migrate_legacy_files_in_output_dir, prune_temp_cache_to_limit,
    save_clip_metadata, StorageCleanupReport, StorageQuotaManager,
};
