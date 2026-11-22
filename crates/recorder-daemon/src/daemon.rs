//! Core background daemon orchestrator connecting capture (`core-capture`),
//! hardware encoding (`core-encode`), multi-track audio + RNNoise + PTT (`core-audio`),
//! replay ring buffer / fMP4 disk session writer (`core-mux`), game auto-detection,
//! global hotkeys, `rodio` sound notifications, JSON sidecars, and storage quota eviction.

use crate::game_detector::{GameDetectionTransition, GameDetector};
use crate::hotkeys::{GlobalHotkeyService, HotkeyAction};
use crate::sounds::SoundCuePlayer;
use crate::storage::{
    apply_temp_cache_policy, migrate_legacy_files_in_output_dir, prune_temp_cache_to_limit,
    save_clip_metadata, StorageQuotaManager,
};
use chrono::{DateTime, Duration as ChronoDuration, Local, Utc};
use core_audio::{start_audio_pipeline, AudioOutputEvent, AudioPipelineHandle};
use core_capture::{
    start_capture_thread, CaptureEngineConfig, CaptureOutputEvent, CaptureSessionHandle,
    D3d11SharedContext,
};
use core_encode::{EncoderOutput, MfHardwareEncoder, MfVideoEncoderConfig};
use core_mux::PacketRouter;
use core_types::{
    ActiveSessionInfo, AppLanguage, AppSettings, AudioOutputMode, AudioTrackMetadata,
    CacheCleanupMode, CaptureTarget, ClipMetadata, DaemonStateSnapshot, DetectedGameInfo,
    MarkerSource, MediaPacket, MonotonicClock, MuxError, PipelineComponent, RecorderCommand,
    RecorderEvent, RecordingKind, SessionMarker, SoundNotificationKind, ToastKind, TrackKind,
    TrayIndicatorState, VideoStreamHeader, ENCODED_PACKET_CHANNEL_CAPACITY,
};
use crossbeam_channel::{bounded, select, tick, Receiver, Sender};
use parking_lot::RwLock;
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use thiserror::Error;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Errors raised during daemon lifecycle initialization or command dispatch.
#[derive(Debug, Error)]
pub enum DaemonError {
    #[error("Daemon initialization failed: {0}")]
    InitFailed(String),
    #[error("Recorder daemon command channel closed")]
    ChannelClosed,
}

enum PipelineMsg {
    SourceReset { epoch: u64, width: u32, height: u32 },
    VideoHeader { epoch: u64, header: VideoStreamHeader },
    VideoPacket { epoch: u64, packet: MediaPacket },
    AudioHeader(core_types::AudioStreamHeader),
    AudioPacket(MediaPacket),
    Warning(PipelineComponent, String),
}

struct ActiveSessionTracker {
    info: ActiveSessionInfo,
    started_utc: DateTime<Utc>,
    markers: Vec<SessionMarker>,
}

struct StampedReplayMarker {
    created_at: DateTime<Utc>,
    label: String,
    source: MarkerSource,
}

/// Top-level background recording service handle owned by the Tauri app or standalone CLI.
pub struct RecorderDaemon {
    cmd_tx: Sender<RecorderCommand>,
    state: Arc<RwLock<DaemonStateSnapshot>>,
    settings: Arc<RwLock<AppSettings>>,
    stop_flag: Arc<AtomicBool>,
    coord_thread: Option<JoinHandle<()>>,
}

impl RecorderDaemon {
    /// Starts the complete recording daemon, spawning audio, hotkey, sound, and orchestrator threads.
    pub fn start(mut initial_settings: AppSettings) -> Result<(Self, Receiver<RecorderEvent>), DaemonError> {
        initial_settings.normalize();
        ensure_output_subfolders(&initial_settings.storage.output_dir)
            .map_err(|e| DaemonError::InitFailed(format!("Create output_dir failed: {e}")))?;
        migrate_legacy_files_in_output_dir(&initial_settings.storage.output_dir);
        apply_temp_cache_policy(&initial_settings.storage);

        let (cmd_tx, cmd_rx) = bounded::<RecorderCommand>(64);
        let (evt_tx, evt_rx) = bounded::<RecorderEvent>(256);

        let initial_snap = DaemonStateSnapshot {
            tray_state: TrayIndicatorState::Idle,
            capture_target: initial_settings.capture.target.clone(),
            active_game: None,
            buffer_fill_sec: 0.0,
            buffer_capacity_sec: initial_settings.replay_buffer.duration_secs,
            buffer_bytes: 0,
            active_session: None,
            dropped_video_frames: 0,
            dropped_audio_frames: 0,
            game_muted: initial_settings.audio.game_muted,
            mic_muted: initial_settings.audio.mic_muted,
            secondary_muted: initial_settings.audio.secondary_muted,
            mic_noise_suppression: initial_settings.audio.mic_noise_suppression,
            privacy_mode: false,
        };

        let state = Arc::new(RwLock::new(initial_snap));
        let settings_lock = Arc::new(RwLock::new(initial_settings.clone()));
        let stop_flag = Arc::new(AtomicBool::new(false));

        let state_c = Arc::clone(&state);
        let settings_c = Arc::clone(&settings_lock);
        let stop_c = Arc::clone(&stop_flag);

        let coord_thread = thread::Builder::new()
            .name("rustclip-daemon-coord".to_string())
            .spawn(move || {
                run_coordinator_loop(
                    initial_settings,
                    cmd_rx,
                    evt_tx,
                    state_c,
                    settings_c,
                    stop_c,
                );
            })
            .map_err(|e| DaemonError::InitFailed(format!("Spawn coordinator failed: {e}")))?;

        Ok((
            Self {
                cmd_tx,
                state,
                settings: settings_lock,
                stop_flag,
                coord_thread: Some(coord_thread),
            },
            evt_rx,
        ))
    }

    /// Sends a command to the running daemon without blocking.
    pub fn send_command(&self, cmd: RecorderCommand) -> Result<(), DaemonError> {
        self.cmd_tx
            .send(cmd)
            .map_err(|_| DaemonError::ChannelClosed)
    }

    /// Clones the command sender for multi-producer UI/tray callers.
    #[must_use]
    pub fn command_sender(&self) -> Sender<RecorderCommand> {
        self.cmd_tx.clone()
    }

    /// Returns a consistent snapshot of the current daemon telemetry & toggle flags.
    #[must_use]
    pub fn state_snapshot(&self) -> DaemonStateSnapshot {
        self.state.read().clone()
    }

    /// Returns a clone of the active normalized [`AppSettings`].
    #[must_use]
    pub fn settings(&self) -> AppSettings {
        self.settings.read().clone()
    }

    /// Gracefully stops the daemon, flushing any open session recording and joining all threads.
    pub fn shutdown(mut self) {
        self.shutdown_internal();
    }

    fn shutdown_internal(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        let _ = self.cmd_tx.send(RecorderCommand::Shutdown);
        if let Some(h) = self.coord_thread.take() {
            let _ = h.join();
        }
    }
}

impl Drop for RecorderDaemon {
    fn drop(&mut self) {
        self.shutdown_internal();
    }
}

struct ActiveVideoCapture {
    handle: CaptureSessionHandle,
    encode_thread: Option<JoinHandle<()>>,
    stop_flag: Arc<AtomicBool>,
    force_idr: Arc<AtomicBool>,
}

impl ActiveVideoCapture {
    fn start(
        capture_target: CaptureTarget,
        settings: &AppSettings,
        clock: MonotonicClock,
        epoch_base: u64,
        pipe_tx: Sender<PipelineMsg>,
    ) -> Result<Self, String> {
        let shared_d3d =
            D3d11SharedContext::new().map_err(|e| format!("D3D11 init failed: {e}"))?;
        let autogame_fallback = !matches!(capture_target, CaptureTarget::AutoGame);
        let cap_cfg = CaptureEngineConfig {
            initial_target: capture_target,
            resolution: settings.video.resolution,
            fps: settings.video.fps,
            capture_cursor: settings.capture.capture_cursor,
            capture_border: settings.capture.capture_border,
            hdr_mode: settings.capture.hdr_mode,
            autogame_fallback_to_monitor: autogame_fallback,
        };

        let (handle, frame_rx) =
            start_capture_thread(shared_d3d.clone(), cap_cfg, clock)
                .map_err(|e| format!("start_capture_thread failed: {e}"))?;

        let stop_flag = Arc::new(AtomicBool::new(false));
        let enc_stop = Arc::clone(&stop_flag);
        let force_idr = Arc::new(AtomicBool::new(false));
        let enc_force_idr = Arc::clone(&force_idr);
        let enc_cfg_template = MfVideoEncoderConfig {
            codec: settings.video.codec,
            vendor: settings.video.gpu_vendor,
            rate_control: settings.video.rate_control,
            width: 1920,
            height: 1080,
            fps: settings.video.fps,
            bitrate_kbps: settings.video.bitrate_kbps,
            gop_seconds: settings.video.gop_seconds,
        };

        let encode_thread = thread::Builder::new()
            .name("rustclip-daemon-encode".to_string())
            .spawn(move || {
                run_video_encode_loop(
                    shared_d3d,
                    enc_cfg_template,
                    epoch_base,
                    frame_rx,
                    pipe_tx,
                    enc_stop,
                    enc_force_idr,
                );
            })
            .map_err(|e| format!("Spawn daemon video encoder failed: {e}"))?;

        Ok(Self {
            handle,
            encode_thread: Some(encode_thread),
            stop_flag,
            force_idr,
        })
    }

    fn request_idr(&self) {
        self.force_idr.store(true, Ordering::Release);
    }

    fn stop(mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        self.handle.stop();
        if let Some(h) = self.encode_thread.take() {
            let _ = h.join();
        }
    }
}

fn run_video_encode_loop(
    mut shared_d3d: D3d11SharedContext,
    mut cfg_template: MfVideoEncoderConfig,
    epoch_base: u64,
    frame_rx: Receiver<CaptureOutputEvent>,
    pipe_tx: Sender<PipelineMsg>,
    stop_flag: Arc<AtomicBool>,
    force_idr: Arc<AtomicBool>,
) {
    let mut encoder: Option<MfHardwareEncoder> = None;
    let mut active_epoch: u64 = 0;

    while !stop_flag.load(Ordering::Relaxed) {
        let Ok(evt) = frame_rx.recv_timeout(Duration::from_millis(50)) else {
            continue;
        };
        match evt {
            CaptureOutputEvent::SourceChanged {
                epoch,
                display_label,
                width,
                height,
                ..
            } => {
                let global_epoch = epoch_base.saturating_add(epoch);
                debug!(
                    "Daemon capture source changed [epoch={global_epoch}]: '{display_label}' ({width}x{height})"
                );
                if let Some(mut old) = encoder.take() {
                    let _ = old.flush();
                }
                active_epoch = global_epoch;
                let _ = pipe_tx.send(PipelineMsg::SourceReset {
                    epoch: global_epoch,
                    width,
                    height,
                });
                cfg_template.width = width;
                cfg_template.height = height;
                match MfHardwareEncoder::new(&shared_d3d.device, cfg_template.clone()) {
                    Ok(enc) => encoder = Some(enc),
                    Err(err) => {
                        let _ = pipe_tx.try_send(PipelineMsg::Warning(
                            PipelineComponent::VideoEncoder,
                            format!("Hardware encoder init failed: {err}"),
                        ));
                    }
                }
            }
            CaptureOutputEvent::DeviceRecreated(new_d3d) => {
                shared_d3d = new_d3d;
                encoder = None;
            }
            CaptureOutputEvent::RecoverableWarning(msg) => {
                let _ = pipe_tx.try_send(PipelineMsg::Warning(PipelineComponent::Capture, msg));
            }
            CaptureOutputEvent::Nv12Frame(frame) => {
                if epoch_base.saturating_add(frame.source_epoch) != active_epoch {
                    continue;
                }
                if force_idr.swap(false, Ordering::AcqRel) {
                    if let Some(mut old) = encoder.take() {
                        let _ = old.flush();
                    }
                    if let Ok(new_enc) =
                        MfHardwareEncoder::new(&shared_d3d.device, cfg_template.clone())
                    {
                        encoder = Some(new_enc);
                    }
                }
                let Some(enc) = encoder.as_mut() else {
                    continue;
                };
                match enc.encode_gpu_frame(&frame) {
                    Ok(outputs) => {
                        for out in outputs {
                            match out {
                                EncoderOutput::StreamHeader(header) => {
                                    let _ = pipe_tx.send(PipelineMsg::VideoHeader {
                                        epoch: active_epoch,
                                        header,
                                    });
                                }
                                EncoderOutput::Packet(packet) => {
                                    let _ = pipe_tx.try_send(PipelineMsg::VideoPacket {
                                        epoch: active_epoch,
                                        packet,
                                    });
                                }
                            }
                        }
                    }
                    Err(err) if err.is_recoverable() => {
                        encoder = None;
                    }
                    Err(err) => {
                        let _ = pipe_tx.try_send(PipelineMsg::Warning(
                            PipelineComponent::VideoEncoder,
                            err.to_string(),
                        ));
                    }
                }
            }
        }
    }

    if let Some(mut enc) = encoder.take() {
        if let Ok(outputs) = enc.flush() {
            for out in outputs {
                if let EncoderOutput::Packet(packet) = out {
                    let _ = pipe_tx.try_send(PipelineMsg::VideoPacket {
                        epoch: active_epoch,
                        packet,
                    });
                }
            }
        }
    }
}

#[must_use]
fn video_pipeline_needs_restart(old: &AppSettings, new: &AppSettings) -> bool {
    old.video != new.video
        || old.capture.capture_cursor != new.capture.capture_cursor
        || old.capture.capture_border != new.capture.capture_border
        || old.capture.hdr_mode != new.capture.hdr_mode
}

struct DaemonRuntime {
    settings: AppSettings,
    clock: MonotonicClock,
    router: PacketRouter,
    router_has_headers: bool,
    current_video_epoch: u64,
    next_video_epoch_base: u64,
    min_accepted_video_epoch: u64,
    pending_video_restart: bool,
    current_width: u32,
    current_height: u32,
    audio_mixed_present: bool,
    audio_game_present: bool,
    audio_mic_present: bool,
    audio_sec_present: bool,
    audio_app_slots: Vec<u8>,
    buffering_requested: bool,
    pending_session_start: bool,
    privacy_mode: bool,
    active_game: Option<DetectedGameInfo>,
    active_session: Option<ActiveSessionTracker>,
    recent_replay_markers: VecDeque<StampedReplayMarker>,
    video_capture: Option<ActiveVideoCapture>,
    audio: Option<AudioPipelineHandle>,
    audio_bridge_stop: Arc<AtomicBool>,
    audio_bridge_thread: Option<JoinHandle<()>>,
    game_detector: GameDetector,
    hotkeys: Option<GlobalHotkeyService>,
    sound_player: Option<SoundCuePlayer>,
    pipe_tx: Sender<PipelineMsg>,
    evt_tx: Sender<RecorderEvent>,
    state_lock: Arc<RwLock<DaemonStateSnapshot>>,
    settings_lock: Arc<RwLock<AppSettings>>,
}

impl DaemonRuntime {
    fn sync_audio_bridge(
        &mut self,
    ) {
        // Already running?
        if self.audio.is_some() {
            return;
        }
        let labels: Vec<(u8, String)> = self
            .settings
            .audio
            .app_channels
            .iter()
            .take(4)
            .enumerate()
            .map(|(i, c)| (i as u8, c.name.clone()))
            .collect();
        self.router.set_app_audio_labels(labels);
        let target_pid = self.active_game.as_ref().map(|g| g.process_id);
        match start_audio_pipeline(self.settings.audio.clone(), target_pid, self.clock) {
            Ok((handle, rx)) => {
                let stop_flag = Arc::new(AtomicBool::new(false));
                let stop_c = Arc::clone(&stop_flag);
                let pipe_tx_c = self.pipe_tx.clone();
                let bridge = thread::Builder::new()
                    .name("rustclip-daemon-audio-bridge".to_string())
                    .spawn(move || {
                        while !stop_c.load(Ordering::Relaxed) {
                            let Ok(evt) = rx.recv_timeout(Duration::from_millis(50)) else {
                                continue;
                            };
                            match evt {
                                AudioOutputEvent::StreamHeader(h) => {
                                    let _ = pipe_tx_c.send(PipelineMsg::AudioHeader(h));
                                }
                                AudioOutputEvent::Packet(p) => {
                                    let _ = pipe_tx_c.try_send(PipelineMsg::AudioPacket(p));
                                }
                                AudioOutputEvent::RecoverableWarning(msg) => {
                                    let _ = pipe_tx_c.try_send(PipelineMsg::Warning(
                                        PipelineComponent::AudioLoopback,
                                        msg,
                                    ));
                                }
                            }
                        }
                    });
                match bridge {
                    Ok(h) => {
                        self.audio = Some(handle);
                        self.audio_bridge_stop = stop_flag;
                        self.audio_bridge_thread = Some(h);
                    }
                    Err(err) => {
                        warn!("Spawn daemon audio bridge failed: {err}");
                        handle.stop();
                    }
                }
            }
            Err(err) => {
                warn!("Audio pipeline start warning: {err}");
            }
        }
    }

    fn desired_capture_target(&self) -> Option<CaptureTarget> {
        if self.privacy_mode {
            return None;
        }
        let need_video = self.buffering_requested || self.active_session.is_some();
        if !need_video {
            return None;
        }

        match &self.settings.capture.target {
            CaptureTarget::Monitor { .. } | CaptureTarget::SpecificWindow { .. } => {
                Some(self.settings.capture.target.clone())
            }
            CaptureTarget::AutoGame => self.active_game.as_ref().map(|g| {
                CaptureTarget::SpecificWindow {
                    hwnd: g.hwnd,
                    process_name: g.executable_name.clone(),
                    title: if g.window_title.is_empty() {
                        g.display_name.clone()
                    } else {
                        g.window_title.clone()
                    },
                }
            }),
        }
    }

    fn reconcile_capture_state(&mut self) {
        let desired = self.desired_capture_target();
        match (desired, self.video_capture.is_some()) {
            (None, true) => {
                if let Some(cap) = self.video_capture.take() {
                    cap.stop();
                }
                self.router.on_source_reset();
                self.router_has_headers = false;
            }
            (Some(target), false) => {
                self.sync_audio_bridge();
                match ActiveVideoCapture::start(
                    target,
                    &self.settings,
                    self.clock,
                    self.next_video_epoch_base,
                    self.pipe_tx.clone(),
                ) {
                    Ok(cap) => self.video_capture = Some(cap),
                    Err(err) => warn!("Capture start deferred: {err}"),
                }
            }
            (Some(target), true) => {
                if let Some(cap) = self.video_capture.as_ref() {
                    // Only send switch_target if user explicitly requested or AutoGame picked a window;
                    // caller explicitly triggers switch_target on change to avoid churn every tick.
                    let _ = (cap, target);
                }
            }
            (None, false) => {}
        }
    }

    fn switch_capture_target(&mut self, new_target: CaptureTarget) {
        self.settings.capture.target = new_target.clone();
        *self.settings_lock.write() = self.settings.clone();
        if !matches!(new_target, CaptureTarget::AutoGame) {
            self.buffering_requested = true;
        }
        let _ = self.evt_tx.try_send(RecorderEvent::CaptureTargetChanged {
            target: new_target.clone(),
        });

        match (&new_target, self.desired_capture_target(), self.video_capture.as_ref()) {
            (_, Some(resolved), Some(cap)) => {
                if let Err(err) = cap.handle.switch_target(resolved) {
                    warn!("Live capture target switch failed: {err}");
                }
            }
            _ => {
                self.reconcile_capture_state();
            }
        }
    }

    fn set_privacy_mode(&mut self, enabled: bool) {
        if self.privacy_mode == enabled {
            return;
        }
        self.privacy_mode = enabled;
        self.pending_session_start = false;
        if enabled {
            // Stop active session safely if one was recording, and immediately wipe the replay buffer.
            if self.active_session.is_some() {
                self.stop_session_internal();
            }
            if let Some(cap) = self.video_capture.take() {
                cap.stop();
            }
            self.router.on_source_reset();
            self.router_has_headers = false;
            self.recent_replay_markers.clear();
            info!("Privacy Mode ENABLED: capture paused and replay buffer wiped");
            let is_en = self.settings.language == AppLanguage::En;
            let _ = self.evt_tx.try_send(RecorderEvent::ShowOverlayToast {
                kind: ToastKind::Warning,
                title: if is_en {
                    "Privacy Mode Enabled"
                } else {
                    "Режим приватности включён"
                }
                .to_string(),
                message: if is_en {
                    "Capture paused and replay buffer wiped"
                } else {
                    "Захват приостановлен, буфер повтора очищен"
                }
                .to_string(),
            });
        } else {
            info!("Privacy Mode DISABLED: resuming configured capture");
            self.reconcile_capture_state();
            let is_en = self.settings.language == AppLanguage::En;
            let _ = self.evt_tx.try_send(RecorderEvent::ShowOverlayToast {
                kind: ToastKind::Info,
                title: if is_en {
                    "Privacy Mode Disabled"
                } else {
                    "Режим приватности выключен"
                }
                .to_string(),
                message: if is_en {
                    "Background buffering resumed"
                } else {
                    "Фоновая буферизация возобновлена"
                }
                .to_string(),
            });
        }
        let _ = self
            .evt_tx
            .try_send(RecorderEvent::PrivacyModeChanged { enabled });
        self.publish_state_snapshot();
    }

    fn add_bookmark(&mut self, label: Option<String>, source: MarkerSource) {
        let now = Utc::now();
        let is_en = self.settings.language == AppLanguage::En;
        let clean_label = label.unwrap_or_else(|| {
            if is_en {
                "Bookmark (F8)".to_string()
            } else {
                "Закладка (F8)".to_string()
            }
        });

        self.recent_replay_markers.push_back(StampedReplayMarker {
            created_at: now,
            label: clean_label.clone(),
            source,
        });
        let retention = ChronoDuration::seconds(
            i64::from(self.settings.replay_buffer.duration_secs).saturating_add(60),
        );
        while let Some(front) = self.recent_replay_markers.front() {
            if now - front.created_at > retention {
                self.recent_replay_markers.pop_front();
            } else {
                break;
            }
        }

        let timestamp_sec = if let Some(sess) = self.active_session.as_mut() {
            let elapsed = (now - sess.started_utc).num_milliseconds().max(0) as f64 / 1000.0;
            let marker = SessionMarker {
                id: Uuid::new_v4(),
                timestamp_sec: elapsed,
                label: clean_label.clone(),
                source,
                created_at: now,
            };
            sess.markers.push(marker.clone());
            sess.info.markers_count = sess.markers.len();
            let session_id = Some(sess.info.session_id);
            let _ = self
                .evt_tx
                .try_send(RecorderEvent::BookmarkAdded { session_id, marker });
            elapsed
        } else {
            let (fill_sec, _, _, _, _) = self.router.stats();
            let marker = SessionMarker {
                id: Uuid::new_v4(),
                timestamp_sec: fill_sec,
                label: clean_label.clone(),
                source,
                created_at: now,
            };
            let _ = self.evt_tx.try_send(RecorderEvent::BookmarkAdded {
                session_id: None,
                marker,
            });
            fill_sec
        };

        self.play_sound(SoundNotificationKind::MarkerAdded);
        let _ = self.evt_tx.try_send(RecorderEvent::ShowOverlayToast {
            kind: ToastKind::Info,
            title: if is_en {
                "Bookmark Added (F8)"
            } else {
                "Маркер добавлен (F8)"
            }
            .to_string(),
            message: if is_en {
                format!("{clean_label} • {timestamp_sec:.1}s")
            } else {
                format!("{clean_label} • {timestamp_sec:.1} сек")
            },
        });
        info!("Added bookmark '{}' at {:.2}s", clean_label, timestamp_sec);
    }

    fn save_replay_clip_internal(&mut self) {
        if self.privacy_mode {
            warn!("Ignored SaveReplayClip while Privacy Mode is active");
            return;
        }
        let (game_name, title_base, exe_name) = self.current_game_labels();
        let path = build_output_mp4_path(&self.settings.storage.output_dir, &game_name, "clip");

        match self.router.save_replay_clip(&path) {
            Ok(summary) => {
                let (game_peak_dbfs, mic_peak_dbfs, output_device_name) =
                    if let Some(tel) = self.audio.as_ref().map(AudioPipelineHandle::telemetry) {
                        (tel.game.peak_dbfs, tel.mic.peak_dbfs, tel.game.active_device)
                    } else {
                        (-100.0, -100.0, "None".to_string())
                    };
                let active_mode = match &self.settings.audio.audio_output_mode {
                    AudioOutputMode::DefaultDevice => "Default",
                    AudioOutputMode::SpecificDevice(_) => "Specific",
                    AudioOutputMode::GameProcessOnly => "GameProcessOnly",
                };
                info!(
                    "Clip audio summary '{}' -> game_pkts={}, mic_pkts={}, game_peak_dbfs={:.1}, mic_peak_dbfs={:.1}, output_device_name='{}', active_mode={}",
                    summary.path.display(),
                    summary.game_pkts,
                    summary.mic_pkts,
                    game_peak_dbfs,
                    mic_peak_dbfs,
                    output_device_name,
                    active_mode
                );
                if summary.mic_pkts > 0 && game_peak_dbfs < -95.0 {
                    warn!(
                        "Clip '{}' has silent Game Audio (game_peak_dbfs={:.1} < -95.0) while mic_pkts={} (output_device_name='{}', active_mode={})",
                        summary.path.display(),
                        game_peak_dbfs,
                        summary.mic_pkts,
                        output_device_name,
                        active_mode
                    );
                }

                let now = Utc::now();
                let clip_start = now
                    - ChronoDuration::milliseconds((summary.duration_sec * 1000.0).round() as i64);
                let markers: Vec<SessionMarker> = self
                    .recent_replay_markers
                    .iter()
                    .filter(|m| m.created_at >= clip_start && m.created_at <= now)
                    .map(|m| SessionMarker {
                        id: Uuid::new_v4(),
                        timestamp_sec: ((m.created_at - clip_start).num_milliseconds().max(0)
                            as f64)
                            / 1000.0,
                        label: m.label.clone(),
                        source: m.source,
                        created_at: m.created_at,
                    })
                    .collect();

                let meta = self.build_clip_metadata(
                    RecordingKind::Clip,
                    &game_name,
                    &title_base,
                    exe_name,
                    summary.path.clone(),
                    summary.duration_sec,
                    summary.bytes_written,
                    markers,
                    now,
                );
                if let Err(err) = save_clip_metadata(&meta) {
                    warn!("Failed to write clip sidecar JSON: {err}");
                }

                self.play_sound(SoundNotificationKind::ClipSaved);
                let _ = self
                    .evt_tx
                    .try_send(RecorderEvent::ClipSaved(Box::new(meta)));
                if self.settings.replay_buffer.reset_buffer_on_clip {
                    self.router.clear_replay_buffer();
                    if let Some(cap) = self.video_capture.as_ref() {
                        cap.request_idr();
                    }
                    self.publish_state_snapshot();
                    info!("Replay RAM buffer cleared after clip save (reset_buffer_on_clip=true)");
                }
                self.enforce_storage_quota();
            }
            Err(err) => {
                warn!("SaveReplayClip failed: {err}");
                let is_en = self.settings.language == AppLanguage::En;
                let (title, message) = if self.video_capture.is_none() {
                    if is_en {
                        (
                            "Buffer Is Empty".to_string(),
                            "Launch a game or pick a monitor from the top bar".to_string(),
                        )
                    } else {
                        (
                            "Буфер ещё пуст".to_string(),
                            "Запустите игру или выберите экран в списке сверху".to_string(),
                        )
                    }
                } else if is_en {
                    (
                        "Buffer Priming".to_string(),
                        "Wait 1–2 seconds and press F9 again".to_string(),
                    )
                } else {
                    (
                        "Буфер ещё наполняется".to_string(),
                        "Подождите 1–2 секунды и нажмите F9 снова".to_string(),
                    )
                };
                let _ = self.evt_tx.try_send(RecorderEvent::ShowOverlayToast {
                    kind: ToastKind::Warning,
                    title,
                    message,
                });
            }
        }
    }

    fn start_session_internal(&mut self) {
        if self.privacy_mode {
            warn!("Cannot start session recording while Privacy Mode is ON");
            return;
        }
        if self.active_session.is_some() {
            return;
        }
        let is_en = self.settings.language == AppLanguage::En;
        self.reconcile_capture_state();
        if self.video_capture.is_none() && self.desired_capture_target().is_none() {
            let _ = self.evt_tx.try_send(RecorderEvent::ShowOverlayToast {
                kind: ToastKind::Warning,
                title: if is_en {
                    "No Capture Source"
                } else {
                    "Источник захвата не выбран"
                }
                .to_string(),
                message: if is_en {
                    "Launch a game or pick a monitor from the top bar"
                } else {
                    "Запустите игру или выберите экран в списке сверху"
                }
                .to_string(),
            });
            return;
        }
        let (game_name, _, _) = self.current_game_labels();
        let path = build_output_mp4_path(&self.settings.storage.output_dir, &game_name, "session");

        match self.router.start_session(&path) {
            Ok(()) => {
                self.pending_session_start = false;
                let now = Utc::now();
                let info = ActiveSessionInfo {
                    session_id: Uuid::new_v4(),
                    started_at: now,
                    elapsed_sec: 0.0,
                    output_path: path.clone(),
                    markers_count: 0,
                    bytes_written: 0,
                };
                self.active_session = Some(ActiveSessionTracker {
                    info: info.clone(),
                    started_utc: now,
                    markers: Vec::new(),
                });
                self.play_sound(SoundNotificationKind::RecordingStarted);
                let _ = self.evt_tx.try_send(RecorderEvent::SessionStarted(info));
                self.publish_state_snapshot();
            }
            Err(MuxError::MissingStreamHeaders) if self.video_capture.is_some() => {
                self.pending_session_start = true;
                let _ = self.evt_tx.try_send(RecorderEvent::ShowOverlayToast {
                    kind: ToastKind::Info,
                    title: if is_en {
                        "Preparing Recording..."
                    } else {
                        "Подготовка записи..."
                    }
                    .to_string(),
                    message: if is_en {
                        "Waiting for first keyframe from video encoder"
                    } else {
                        "Ожидание первого ключевого кадра от видеоэнкодера"
                    }
                    .to_string(),
                });
            }
            Err(err) => {
                warn!("StartSession failed: {err}");
                let _ = self.evt_tx.try_send(RecorderEvent::ShowOverlayToast {
                    kind: ToastKind::Warning,
                    title: if is_en {
                        "Could Not Start Recording"
                    } else {
                        "Не удалось начать запись"
                    }
                    .to_string(),
                    message: if is_en {
                        "Launch a game or pick a monitor from the top bar"
                    } else {
                        "Запустите игру или выберите экран в списке сверху"
                    }
                    .to_string(),
                });
            }
        }
    }

    fn stop_session_internal(&mut self) {
        let Some(tracker) = self.active_session.take() else {
            return;
        };
        let remux = self.settings.storage.auto_remux_fmp4;
        match self.router.stop_session(remux) {
            Ok(summary) => {
                let (game_name, title_base, exe_name) = self.current_game_labels();
                let meta = self.build_clip_metadata(
                    RecordingKind::Session,
                    &game_name,
                    &title_base,
                    exe_name,
                    summary.path.clone(),
                    summary.duration_sec,
                    summary.bytes_written,
                    tracker.markers,
                    Utc::now(),
                );
                if let Err(err) = save_clip_metadata(&meta) {
                    warn!("Failed to write session sidecar JSON: {err}");
                }
                self.play_sound(SoundNotificationKind::RecordingStopped);
                let _ = self
                    .evt_tx
                    .try_send(RecorderEvent::SessionStopped(Box::new(meta)));
                self.enforce_storage_quota();
            }
            Err(err) => warn!("StopSession failed: {err}"),
        }
        if self.pending_video_restart {
            self.pending_video_restart = false;
            info!("Applying deferred live video encoder reinit after session stop");
            self.restart_video_capture();
        } else {
            self.reconcile_capture_state();
        }
        self.publish_state_snapshot();
    }

    fn current_game_labels(&self) -> (String, String, Option<String>) {
        if let Some(g) = &self.active_game {
            let name = g.display_name.clone();
            (name.clone(), name, Some(g.executable_name.clone()))
        } else {
            match &self.settings.capture.target {
                CaptureTarget::Monitor { name, .. } => {
                    let mon_idx: String = name
                        .chars()
                        .skip_while(|c| !c.is_ascii_digit())
                        .take_while(|c| c.is_ascii_digit())
                        .collect();
                    let label = if mon_idx.is_empty() {
                        "Экран (Монитор 1)".to_string()
                    } else {
                        format!("Экран (Монитор {mon_idx})")
                    };
                    ("Экран".to_string(), label, None)
                }
                CaptureTarget::AutoGame => ("Экран".to_string(), "Экран".to_string(), None),
                CaptureTarget::SpecificWindow {
                    process_name,
                    title,
                    ..
                } => {
                    let label =
                        crate::game_detector::resolve_human_game_name(None, process_name, title);
                    (label.clone(), label, Some(process_name.clone()))
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_clip_metadata(
        &self,
        kind: RecordingKind,
        game_name: &str,
        title_base: &str,
        executable_name: Option<String>,
        file_path: PathBuf,
        duration_sec: f64,
        file_size_bytes: u64,
        markers: Vec<SessionMarker>,
        created_at: DateTime<Utc>,
    ) -> ClipMetadata {
        let title = match kind {
            RecordingKind::Clip => title_base.to_string(),
            RecordingKind::Session => format!("{title_base} • Сессия"),
        };

        let mut audio_tracks = Vec::new();
        if self.audio_mixed_present {
            audio_tracks.push(AudioTrackMetadata {
                track_index: 1,
                kind: TrackKind::AudioMixed,
                label: "Mixed Audio".to_string(),
                codec: "aac".to_string(),
                sample_rate: 48_000,
                channels: 2,
                default_volume: 1.0,
                cached_audio_path: None,
            });
        }
        if self.audio_game_present {
            audio_tracks.push(AudioTrackMetadata {
                track_index: (audio_tracks.len() + 1) as u8,
                kind: TrackKind::AudioGame,
                label: "Game Audio".to_string(),
                codec: "aac".to_string(),
                sample_rate: 48_000,
                channels: 2,
                default_volume: self.settings.audio.game_volume,
                cached_audio_path: None,
            });
        }
        if self.audio_mic_present {
            audio_tracks.push(AudioTrackMetadata {
                track_index: (audio_tracks.len() + 1) as u8,
                kind: TrackKind::AudioMic,
                label: "Microphone".to_string(),
                codec: "aac".to_string(),
                sample_rate: 48_000,
                channels: 2,
                default_volume: self.settings.audio.mic_volume,
                cached_audio_path: None,
            });
        }
        if self.audio_sec_present {
            audio_tracks.push(AudioTrackMetadata {
                track_index: (audio_tracks.len() + 1) as u8,
                kind: TrackKind::AudioSecondary,
                label: "Secondary Audio".to_string(),
                codec: "aac".to_string(),
                sample_rate: 48_000,
                channels: 2,
                default_volume: self.settings.audio.secondary_volume,
                cached_audio_path: None,
            });
        }
        for &slot_idx in &self.audio_app_slots {
            if let Some(ch) = self.settings.audio.app_channels.get(usize::from(slot_idx)) {
                audio_tracks.push(AudioTrackMetadata {
                    track_index: (audio_tracks.len() + 1) as u8,
                    kind: TrackKind::AudioApp(slot_idx),
                    label: ch.name.clone(),
                    codec: "aac".to_string(),
                    sample_rate: 48_000,
                    channels: 2,
                    default_volume: ch.volume,
                    cached_audio_path: None,
                });
            }
        }

        ClipMetadata {
            id: Uuid::new_v4(),
            title,
            game_name: game_name.to_string(),
            executable_name,
            kind,
            file_path,
            thumbnail_path: None,
            waveform_path: None,
            created_at,
            duration_sec,
            file_size_bytes,
            width: self.current_width,
            height: self.current_height,
            fps: self.settings.video.fps,
            video_codec: self.settings.video.codec,
            video_bitrate_kbps: self.settings.video.bitrate_kbps,
            audio_tracks,
            markers,
            favorite: false,
            is_edited: false,
            tags: Vec::new(),
            crash_recovered: false,
        }
    }

    fn enforce_storage_quota(&self) {
        match StorageQuotaManager::enforce_quota(
            &self.settings.storage.output_dir,
            self.settings.storage.max_storage_gb,
            self.settings.storage.max_age_days,
        ) {
            Ok(report) if report.deleted_files > 0 => {
                let _ = self
                    .evt_tx
                    .try_send(RecorderEvent::StorageCleanupCompleted {
                        deleted_files: report.deleted_files,
                        freed_bytes: report.freed_bytes,
                    });
            }
            Ok(_) => {}
            Err(err) => warn!("Storage quota check failed: {err}"),
        }
    }

    fn play_sound(&self, kind: SoundNotificationKind) {
        if let Some(player) = &self.sound_player {
            player.play_event(kind, &self.settings.notifications);
        }
    }

    fn restart_video_capture(&mut self) {
        if let Some(cap) = self.video_capture.take() {
            cap.stop();
        }
        self.next_video_epoch_base = self.next_video_epoch_base.saturating_add(1_000_000);
        self.min_accepted_video_epoch = self.next_video_epoch_base;
        self.current_video_epoch = 0;
        self.router.on_source_reset();
        self.router_has_headers = false;
        self.reconcile_capture_state();
    }

    fn restart_audio_bridge(&mut self) {
        self.audio_bridge_stop.store(true, Ordering::SeqCst);
        if let Some(aud) = self.audio.take() {
            aud.stop();
        }
        if let Some(h) = self.audio_bridge_thread.take() {
            let _ = h.join();
        }
        self.audio_mixed_present = false;
        self.audio_game_present = false;
        self.audio_mic_present = false;
        self.audio_sec_present = false;
        self.audio_app_slots.clear();
        self.router.clear_audio_headers();
        if self.video_capture.is_some() {
            self.sync_audio_bridge();
        }
    }

    fn apply_settings(&mut self, mut new_settings: AppSettings) {
        new_settings.normalize();
        let _ = ensure_output_subfolders(&new_settings.storage.output_dir);
        let target_changed = new_settings.capture.target != self.settings.capture.target;
        let hotkeys_changed = new_settings.hotkeys != self.settings.hotkeys;
        let video_restart_needed = video_pipeline_needs_restart(&self.settings, &new_settings);
        let audio_pipeline_changed = new_settings.audio.separate_tracks
            != self.settings.audio.separate_tracks
            || new_settings.audio.aac_bitrate_kbps != self.settings.audio.aac_bitrate_kbps
            || new_settings.audio.app_channels.len() != self.settings.audio.app_channels.len()
            || new_settings
                .audio
                .app_channels
                .iter()
                .zip(self.settings.audio.app_channels.iter())
                .any(|(a, b)| {
                    a.enabled != b.enabled || a.process_names != b.process_names || a.name != b.name
                });
        let replay_enabled_changed =
            new_settings.replay_buffer.enabled != self.settings.replay_buffer.enabled;

        self.router.update_replay_limits(
            new_settings.replay_buffer.duration_secs,
            new_settings
                .replay_buffer
                .max_ram_mb
                .saturating_mul(1024 * 1024),
        );
        self.game_detector
            .update_config(new_settings.capture.games.clone());

        if let Some(aud) = &self.audio {
            aud.set_output_mode(new_settings.audio.audio_output_mode.clone());
            aud.set_mic_device(new_settings.audio.mic_device.clone());
            aud.set_mic_activation_mode(new_settings.audio.mic_activation_mode.clone());
            aud.set_mic_soft_limiter(new_settings.audio.mic_soft_limiter);
            aud.set_secondary_output_device(new_settings.audio.secondary_output_device.clone());
            aud.set_volumes(
                new_settings.audio.game_volume,
                new_settings.audio.mic_volume,
                Some(new_settings.audio.secondary_volume),
            );
            aud.set_game_muted(new_settings.audio.game_muted);
            aud.set_mic_muted(new_settings.audio.mic_muted);
            aud.set_secondary_muted(new_settings.audio.secondary_muted);
            aud.set_mic_noise_suppression(
                new_settings.audio.mic_noise_suppression,
                new_settings.audio.mic_noise_suppression_strength,
                new_settings.audio.mic_noise_gate_db,
            );
            for (idx, ch) in new_settings.audio.app_channels.iter().take(4).enumerate() {
                aud.set_app_channel_mix(idx as u8, ch.volume, ch.muted);
            }
        }

        if hotkeys_changed {
            if let Some(hk) = &self.hotkeys {
                hk.rebind(new_settings.hotkeys.clone());
            }
        }

        if replay_enabled_changed {
            self.buffering_requested = !matches!(new_settings.capture.target, CaptureTarget::AutoGame)
                || new_settings.replay_buffer.enabled;
        }

        let target_to_switch = new_settings.capture.target.clone();
        self.settings = new_settings.clone();
        *self.settings_lock.write() = new_settings;

        if video_restart_needed {
            if target_changed {
                if !matches!(target_to_switch, CaptureTarget::AutoGame) {
                    self.buffering_requested = true;
                }
                let _ = self.evt_tx.try_send(RecorderEvent::CaptureTargetChanged {
                    target: target_to_switch,
                });
            }
            if self.active_session.is_some() {
                self.pending_video_restart = true;
                info!(
                    "Deferred live video encoder reinit until active session ends (bitrate={} kbps, rc={:?}, codec={:?}, fps={}, res={:?}, gop={}s)",
                    self.settings.video.bitrate_kbps,
                    self.settings.video.rate_control,
                    self.settings.video.codec,
                    self.settings.video.fps,
                    self.settings.video.resolution,
                    self.settings.video.gop_seconds,
                );
            } else {
                info!(
                    "Reinitializing live video encoder without restart (bitrate={} kbps, rc={:?}, codec={:?}, fps={}, res={:?}, gop={}s)",
                    self.settings.video.bitrate_kbps,
                    self.settings.video.rate_control,
                    self.settings.video.codec,
                    self.settings.video.fps,
                    self.settings.video.resolution,
                    self.settings.video.gop_seconds,
                );
                self.restart_video_capture();
            }
        } else if target_changed {
            self.switch_capture_target(target_to_switch);
        } else {
            self.reconcile_capture_state();
        }

        if audio_pipeline_changed && self.active_session.is_none() && self.audio.is_some() {
            info!(
                "Reinitializing live audio AAC pipeline without restart (aac={} kbps, separate_tracks={})",
                self.settings.audio.aac_bitrate_kbps,
                self.settings.audio.separate_tracks,
            );
            self.restart_audio_bridge();
        }
        self.publish_state_snapshot();
    }

    fn handle_command(&mut self, cmd: RecorderCommand) -> bool {
        match cmd {
            RecorderCommand::StartBuffering => {
                self.buffering_requested = true;
                self.reconcile_capture_state();
            }
            RecorderCommand::StopBuffering => {
                self.buffering_requested = false;
                self.reconcile_capture_state();
            }
            RecorderCommand::SaveReplayClip => self.save_replay_clip_internal(),
            RecorderCommand::StartSession => self.start_session_internal(),
            RecorderCommand::StopSession => self.stop_session_internal(),
            RecorderCommand::ToggleSession => {
                if self.active_session.is_some() {
                    self.stop_session_internal();
                } else {
                    self.start_session_internal();
                }
            }
            RecorderCommand::AddBookmark { label } => {
                self.add_bookmark(label, MarkerSource::Hotkey);
            }
            RecorderCommand::SetGameMuted { muted } => {
                self.settings.audio.game_muted = muted;
                *self.settings_lock.write() = self.settings.clone();
                if let Some(a) = &self.audio {
                    a.set_game_muted(muted);
                }
            }
            RecorderCommand::ToggleGameMuted => {
                let muted = !self.settings.audio.game_muted;
                self.settings.audio.game_muted = muted;
                *self.settings_lock.write() = self.settings.clone();
                if let Some(a) = &self.audio {
                    a.set_game_muted(muted);
                }
            }
            RecorderCommand::SetMicMuted { muted } => {
                self.settings.audio.mic_muted = muted;
                *self.settings_lock.write() = self.settings.clone();
                if let Some(a) = &self.audio {
                    a.set_mic_muted(muted);
                }
            }
            RecorderCommand::ToggleMicMuted => {
                let muted = !self.settings.audio.mic_muted;
                self.settings.audio.mic_muted = muted;
                *self.settings_lock.write() = self.settings.clone();
                if let Some(a) = &self.audio {
                    a.set_mic_muted(muted);
                }
            }
            RecorderCommand::SetSecondaryOutputDevice { device_id } => {
                self.settings.audio.secondary_output_device = device_id.clone();
                *self.settings_lock.write() = self.settings.clone();
                if let Some(a) = &self.audio {
                    a.set_secondary_output_device(device_id);
                }
            }
            RecorderCommand::SetSecondaryMuted { muted } => {
                self.settings.audio.secondary_muted = muted;
                *self.settings_lock.write() = self.settings.clone();
                if let Some(a) = &self.audio {
                    a.set_secondary_muted(muted);
                }
            }
            RecorderCommand::SetAudioOutputMode(mode) => {
                self.settings.audio.audio_output_mode = mode.clone();
                *self.settings_lock.write() = self.settings.clone();
                if let Some(a) = &self.audio {
                    a.set_output_mode(mode);
                }
            }
            RecorderCommand::SetMicDevice(dev) => {
                self.settings.audio.mic_device = dev.clone();
                *self.settings_lock.write() = self.settings.clone();
                if let Some(a) = &self.audio {
                    a.set_mic_device(dev);
                }
            }
            RecorderCommand::SetAudioVolumes {
                game_volume,
                mic_volume,
                secondary_volume,
            } => {
                self.settings.audio.game_volume = game_volume.clamp(0.0, 2.0);
                self.settings.audio.mic_volume = mic_volume.clamp(0.0, 2.0);
                if let Some(sv) = secondary_volume {
                    self.settings.audio.secondary_volume = sv.clamp(0.0, 2.0);
                }
                *self.settings_lock.write() = self.settings.clone();
                if let Some(a) = &self.audio {
                    a.set_volumes(
                        self.settings.audio.game_volume,
                        self.settings.audio.mic_volume,
                        secondary_volume.map(|_| self.settings.audio.secondary_volume),
                    );
                }
            }
            RecorderCommand::SetMicNoiseSuppression {
                enabled,
                strength,
                noise_gate_db,
            } => {
                self.settings.audio.mic_noise_suppression = enabled;
                self.settings.audio.mic_noise_suppression_strength = strength.clamp(0.0, 1.0);
                self.settings.audio.mic_noise_gate_db = noise_gate_db.map(|db| db.clamp(-90.0, -10.0));
                *self.settings_lock.write() = self.settings.clone();
                if let Some(a) = &self.audio {
                    a.set_mic_noise_suppression(
                        enabled,
                        self.settings.audio.mic_noise_suppression_strength,
                        self.settings.audio.mic_noise_gate_db,
                    );
                }
            }
            RecorderCommand::ToggleMicNoiseSuppression => {
                let enabled = !self.settings.audio.mic_noise_suppression;
                self.settings.audio.mic_noise_suppression = enabled;
                *self.settings_lock.write() = self.settings.clone();
                if let Some(a) = &self.audio {
                    a.set_mic_noise_suppression(
                        enabled,
                        self.settings.audio.mic_noise_suppression_strength,
                        self.settings.audio.mic_noise_gate_db,
                    );
                }
            }
            RecorderCommand::SetMicActivationMode(mode) => {
                self.settings.audio.mic_activation_mode = mode.clone();
                self.settings.normalize();
                *self.settings_lock.write() = self.settings.clone();
                if let Some(a) = &self.audio {
                    a.set_mic_activation_mode(self.settings.audio.mic_activation_mode.clone());
                }
            }
            RecorderCommand::SetMicSoftLimiter { enabled } => {
                self.settings.audio.mic_soft_limiter = enabled;
                *self.settings_lock.write() = self.settings.clone();
                if let Some(a) = &self.audio {
                    a.set_mic_soft_limiter(enabled);
                }
            }
            RecorderCommand::SetPrivacyMode { enabled } => self.set_privacy_mode(enabled),
            RecorderCommand::TogglePrivacyMode => self.set_privacy_mode(!self.privacy_mode),
            RecorderCommand::PlayTestSound { kind } => {
                let mut test_cfg = self.settings.notifications.clone();
                test_cfg.sound_enabled = true;
                if let Some(player) = &self.sound_player {
                    player.play_event(kind, &test_cfg);
                }
            }
            RecorderCommand::RunStorageCleanup => self.enforce_storage_quota(),
            RecorderCommand::SetCaptureTarget(target) => self.switch_capture_target(target),
            RecorderCommand::UpdateSettings(boxed) => self.apply_settings(*boxed),
            RecorderCommand::Shutdown => return false,
        }
        self.publish_state_snapshot();
        true
    }

    fn publish_state_snapshot(&self) {
        let (fill_sec, buf_bytes, _, dropped_disk, _) = self.router.stats();
        let tray_state = if self.active_session.is_some() {
            TrayIndicatorState::Recording
        } else if !self.privacy_mode && self.video_capture.is_some() {
            TrayIndicatorState::Buffering
        } else {
            TrayIndicatorState::Idle
        };

        let active_session = self.active_session.as_ref().map(|s| {
            let mut info = s.info.clone();
            info.elapsed_sec =
                (Utc::now() - s.started_utc).num_milliseconds().max(0) as f64 / 1000.0;
            info
        });

        let snap = DaemonStateSnapshot {
            tray_state,
            capture_target: self.settings.capture.target.clone(),
            active_game: self.active_game.clone(),
            buffer_fill_sec: fill_sec,
            buffer_capacity_sec: self.settings.replay_buffer.duration_secs,
            buffer_bytes: buf_bytes,
            active_session,
            dropped_video_frames: dropped_disk,
            dropped_audio_frames: 0,
            game_muted: self.settings.audio.game_muted,
            mic_muted: self.settings.audio.mic_muted,
            secondary_muted: self.settings.audio.secondary_muted,
            mic_noise_suppression: self.settings.audio.mic_noise_suppression,
            privacy_mode: self.privacy_mode,
        };
        *self.state_lock.write() = snap.clone();
        let _ = self.evt_tx.try_send(RecorderEvent::StateChanged(snap));
    }

    fn shutdown(mut self) {
        if self.active_session.is_some() {
            self.stop_session_internal();
        }
        if let Some(cap) = self.video_capture.take() {
            cap.stop();
        }
        self.audio_bridge_stop.store(true, Ordering::SeqCst);
        if let Some(aud) = self.audio.take() {
            aud.stop();
        }
        if let Some(h) = self.audio_bridge_thread.take() {
            let _ = h.join();
        }
        if let Some(hk) = self.hotkeys.take() {
            hk.stop();
        }
        if let Some(sp) = self.sound_player.take() {
            sp.stop();
        }
        apply_temp_cache_policy(&self.settings.storage);
    }
}

#[allow(clippy::too_many_arguments)]
fn run_coordinator_loop(
    initial_settings: AppSettings,
    cmd_rx: Receiver<RecorderCommand>,
    evt_tx: Sender<RecorderEvent>,
    state_lock: Arc<RwLock<DaemonStateSnapshot>>,
    settings_lock: Arc<RwLock<AppSettings>>,
    stop_flag: Arc<AtomicBool>,
) {
    let clock = MonotonicClock::now();
    let max_ram = initial_settings
        .replay_buffer
        .max_ram_mb
        .saturating_mul(1024 * 1024);
    let router = match PacketRouter::new(initial_settings.replay_buffer.duration_secs, max_ram) {
        Ok(r) => r,
        Err(err) => {
            error!("PacketRouter initialization failed in daemon: {err}");
            return;
        }
    };

    let (pipe_tx, pipe_rx) = bounded::<PipelineMsg>(ENCODED_PACKET_CHANNEL_CAPACITY * 4);
    let (hotkey_tx, hotkey_rx) = bounded::<HotkeyAction>(32);

    let sound_player = SoundCuePlayer::start()
        .map_err(|e| warn!("SoundCuePlayer start failed: {e}"))
        .ok();
    let hotkeys = GlobalHotkeyService::start(initial_settings.hotkeys.clone(), hotkey_tx)
        .map_err(|e| warn!("GlobalHotkeyService start failed: {e}"))
        .ok();

    let buffering_default = !matches!(initial_settings.capture.target, CaptureTarget::AutoGame)
        || initial_settings.replay_buffer.enabled;

    let mut rt = DaemonRuntime {
        game_detector: GameDetector::new(initial_settings.capture.games.clone()),
        settings: initial_settings,
        clock,
        router,
        router_has_headers: false,
        current_video_epoch: 0,
        next_video_epoch_base: 0,
        min_accepted_video_epoch: 0,
        pending_video_restart: false,
        current_width: 1920,
        current_height: 1080,
        audio_mixed_present: false,
        audio_game_present: false,
        audio_mic_present: false,
        audio_sec_present: false,
        audio_app_slots: Vec::new(),
        buffering_requested: buffering_default,
        pending_session_start: false,
        privacy_mode: false,
        active_game: None,
        active_session: None,
        recent_replay_markers: VecDeque::new(),
        video_capture: None,
        audio: None,
        audio_bridge_stop: Arc::new(AtomicBool::new(false)),
        audio_bridge_thread: None,
        hotkeys,
        sound_player,
        pipe_tx,
        evt_tx,
        state_lock,
        settings_lock,
    };

    // Start audio pipeline right away if non-AutoGame or AutoGame already has active capture.
    rt.reconcile_capture_state();
    rt.publish_state_snapshot();

    let detector_tick = tick(Duration::from_millis(250));
    let mut last_cache_prune = Instant::now();

    while !stop_flag.load(Ordering::Relaxed) {
        // Priority-drain commands and hotkeys so user actions never wait behind media packets.
        while let Ok(cmd) = cmd_rx.try_recv() {
            if !rt.handle_command(cmd) {
                rt.shutdown();
                return;
            }
        }
        while let Ok(action) = hotkey_rx.try_recv() {
            let cmd = match action {
                HotkeyAction::SaveClip => RecorderCommand::SaveReplayClip,
                HotkeyAction::ToggleSession => RecorderCommand::ToggleSession,
                HotkeyAction::AddBookmark => RecorderCommand::AddBookmark { label: None },
                HotkeyAction::ToggleMicMute => RecorderCommand::ToggleMicMuted,
                HotkeyAction::TogglePrivacyMode => RecorderCommand::TogglePrivacyMode,
            };
            if !rt.handle_command(cmd) {
                rt.shutdown();
                return;
            }
        }

        select! {
            recv(cmd_rx) -> msg => {
                if let Ok(cmd) = msg {
                    if !rt.handle_command(cmd) {
                        rt.shutdown();
                        return;
                    }
                } else {
                    break;
                }
            }
            recv(hotkey_rx) -> msg => {
                if let Ok(action) = msg {
                    let cmd = match action {
                        HotkeyAction::SaveClip => RecorderCommand::SaveReplayClip,
                        HotkeyAction::ToggleSession => RecorderCommand::ToggleSession,
                        HotkeyAction::AddBookmark => RecorderCommand::AddBookmark { label: None },
                        HotkeyAction::ToggleMicMute => RecorderCommand::ToggleMicMuted,
                        HotkeyAction::TogglePrivacyMode => RecorderCommand::TogglePrivacyMode,
                    };
                    if !rt.handle_command(cmd) {
                        rt.shutdown();
                        return;
                    }
                }
            }
            recv(pipe_rx) -> msg => {
                if let Ok(pmsg) = msg {
                    process_pipeline_msg(&mut rt, pmsg);
                }
            }
            recv(detector_tick) -> _ => {
                match rt.game_detector.poll() {
                    GameDetectionTransition::Detected(game) => {
                        info!("Game detected: {} ({})", game.display_name, game.executable_name);
                        let pid = game.process_id;
                        rt.active_game = Some(game.clone());
                        if let Some(a) = &rt.audio {
                            a.set_target_process_id(Some(pid));
                        }
                        let _ = rt.evt_tx.try_send(RecorderEvent::GameDetected(game));
                        if matches!(rt.settings.capture.target, CaptureTarget::AutoGame)
                            && rt.settings.capture.games.auto_start
                        {
                            rt.reconcile_capture_state();
                        }
                    }
                    GameDetectionTransition::Lost(exe) => {
                        info!("Tracked game exited or unfocused beyond grace period: {}", exe);
                        rt.active_game = None;
                        if let Some(a) = &rt.audio {
                            a.set_target_process_id(None);
                        }
                        let _ = rt.evt_tx.try_send(RecorderEvent::GameLost { executable_name: exe });
                        if matches!(rt.settings.capture.target, CaptureTarget::AutoGame) {
                            rt.reconcile_capture_state();
                        }
                    }
                    GameDetectionTransition::NoChange => {}
                }
                if rt.settings.storage.editor_cache.mode == CacheCleanupMode::TimerAndLimit {
                    let interval = Duration::from_secs(
                        u64::from(rt.settings.storage.editor_cache.interval_minutes.max(5)) * 60,
                    );
                    if last_cache_prune.elapsed() >= interval {
                        last_cache_prune = Instant::now();
                        let temp_dir = rt.settings.storage.temp_dir.clone();
                        let max_bytes = rt
                            .settings
                            .storage
                            .editor_cache
                            .max_size_mb
                            .saturating_mul(1024 * 1024);
                        let _ = thread::Builder::new()
                            .name("ikoch-cache-prune".to_string())
                            .spawn(move || {
                                prune_temp_cache_to_limit(&temp_dir, max_bytes);
                            });
                    }
                }
                rt.publish_state_snapshot();
            }
        }
    }

    rt.shutdown();
}

fn process_pipeline_msg(rt: &mut DaemonRuntime, msg: PipelineMsg) {
    match msg {
        PipelineMsg::SourceReset {
            epoch,
            width,
            height,
        } => {
            if epoch < rt.min_accepted_video_epoch {
                return;
            }
            rt.current_video_epoch = epoch;
            rt.current_width = width;
            rt.current_height = height;
            rt.router.on_source_reset();
            rt.router_has_headers = false;
        }
        PipelineMsg::VideoHeader { epoch, header } => {
            if epoch >= rt.min_accepted_video_epoch && epoch == rt.current_video_epoch {
                rt.router.update_video_header(header);
                rt.router_has_headers = true;
                if rt.pending_session_start {
                    rt.pending_session_start = false;
                    rt.start_session_internal();
                }
            }
        }
        PipelineMsg::AudioHeader(hdr) => {
            match hdr.track {
                TrackKind::AudioMixed => rt.audio_mixed_present = true,
                TrackKind::AudioGame => rt.audio_game_present = true,
                TrackKind::AudioMic => rt.audio_mic_present = true,
                TrackKind::AudioSecondary => rt.audio_sec_present = true,
                TrackKind::AudioApp(slot_idx) => {
                    if !rt.audio_app_slots.contains(&slot_idx) {
                        rt.audio_app_slots.push(slot_idx);
                        rt.audio_app_slots.sort_unstable();
                    }
                }
                TrackKind::Video => {}
            }
            rt.router.update_audio_header(hdr);
        }
        PipelineMsg::VideoPacket { epoch, packet } => {
            if !rt.privacy_mode
                && epoch >= rt.min_accepted_video_epoch
                && epoch == rt.current_video_epoch
                && rt.router_has_headers
            {
                rt.router.push_packet(packet);
            }
        }
        PipelineMsg::AudioPacket(packet) => {
            if !rt.privacy_mode && rt.router_has_headers {
                rt.router.push_packet(packet);
            }
        }
        PipelineMsg::Warning(component, reason) => {
            let _ = rt
                .evt_tx
                .try_send(RecorderEvent::PipelineRecovering { component, reason });
        }
    }
}

fn strip_exe_suffix(name: &str) -> &str {
    name.strip_suffix(".exe")
        .or_else(|| name.strip_suffix(".EXE"))
        .unwrap_or(name)
}

/// Builds a concise, human-readable filename prefix (e.g. `Screen` or `Counter-Strike 2`).
#[must_use]
pub fn clean_recording_prefix(label: &str) -> String {
    let trimmed_raw = label.trim();
    let lower = trimmed_raw.to_ascii_lowercase();
    if trimmed_raw.is_empty()
        || lower == "screen"
        || lower == "desktop"
        || lower.starts_with("monitor")
        || lower.starts_with("display")
        || trimmed_raw.to_lowercase().starts_with("экран")
    {
        return "Screen".to_string();
    }

    // Remove parenthetical annotations like "(64-bit)" or "(Primary, 1920x1080)".
    let mut no_parens = String::with_capacity(trimmed_raw.len());
    let mut depth = 0usize;
    for ch in trimmed_raw.chars() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if depth == 0 => no_parens.push(ch),
            _ => {}
        }
    }

    let mut base = strip_exe_suffix(no_parens.trim()).to_string();
    for noise in ["-Win64-Shipping", "-win64-shipping", "-Win32-Shipping", "-win32-shipping"] {
        if let Some(stripped) = base.strip_suffix(noise) {
            base = stripped.to_string();
        }
    }

    let mut out = String::with_capacity(base.len());
    let mut prev_space = false;
    for ch in base.chars() {
        let safe = match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' | '_' => ' ',
            c if c.is_control() => ' ',
            c => c,
        };
        if safe == ' ' {
            if !prev_space && !out.is_empty() {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(safe);
            prev_space = false;
        }
    }

    let clean = out.trim_matches(|c| c == ' ' || c == '.' || c == '-' || c == '_');
    if clean.is_empty() {
        "Screen".to_string()
    } else {
        clean
            .chars()
            .take(36)
            .collect::<String>()
            .trim_matches(|c| c == ' ' || c == '.' || c == '-' || c == '_')
            .to_string()
    }
}

/// Ensures `output_dir` and its three organized media subfolders (`Clips`, `Edited`, `Sessions`) exist.
fn ensure_output_subfolders(output_dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(output_dir)?;
    for sub in ["Clips", "Edited", "Sessions"] {
        fs::create_dir_all(output_dir.join(sub))?;
    }
    Ok(())
}

/// Builds a concise folder name (`Screen` or `Counter-Strike 2`) safe for Windows paths.
#[must_use]
pub fn clean_game_folder_name(label: &str) -> String {
    clean_recording_prefix(label)
}

fn build_output_mp4_path(output_dir: &Path, label: &str, kind: &str) -> PathBuf {
    let prefix = clean_recording_prefix(label);
    let folder = clean_game_folder_name(label);
    let target_dir = match kind {
        "session" => output_dir.join("Sessions").join(&folder),
        "edited" => output_dir.join("Edited"),
        _ => output_dir.join("Clips").join(&folder),
    };
    let _ = fs::create_dir_all(&target_dir);

    let stamp_hm = Local::now().format("%Y-%m-%d_%H-%M").to_string();
    let primary = target_dir.join(format!("{prefix}_{stamp_hm}.mp4"));
    if !primary.exists() {
        return primary;
    }

    let stamp_hms = Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
    let secondary = target_dir.join(format!("{prefix}_{stamp_hms}.mp4"));
    if !secondary.exists() {
        return secondary;
    }

    for idx in 2..1000u32 {
        let candidate = target_dir.join(format!("{prefix}_{stamp_hms}_{idx}.mp4"));
        if !candidate.exists() {
            return candidate;
        }
    }
    target_dir.join(format!("{prefix}_{stamp_hms}_{}.mp4", Uuid::new_v4().simple()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_recording_prefix_and_path_match_expected_format() {
        assert_eq!(clean_recording_prefix("Monitor 1 (Primary, 1920x1080)"), "Screen");
        assert_eq!(clean_recording_prefix("Desktop"), "Screen");
        assert_eq!(clean_recording_prefix("Экран (Монитор 1)"), "Screen");
        assert_eq!(clean_recording_prefix("Counter-Strike 2"), "Counter-Strike 2");
        assert_eq!(clean_recording_prefix("cs2.exe"), "cs2");
        assert_eq!(clean_recording_prefix("Valorant (Live)"), "Valorant");

        let mon_name = "Monitor 1 (Primary, 1920x1080)";
        let mon_num: String = mon_name
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        assert_eq!(mon_num, "1");

        let base_tmp = std::env::temp_dir().join(format!("ikoch_path_test_{}", Uuid::new_v4()));
        let p = build_output_mp4_path(&base_tmp, "Экран", "clip");
        assert_eq!(
            p.parent(),
            Some(base_tmp.join("Clips").join("Screen").as_path())
        );
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        assert!(name.starts_with("Screen_"), "unexpected filename: {name}");
        assert!(name.ends_with(".mp4"), "unexpected filename: {name}");
        assert!(!name.contains("Primary"), "filename too verbose: {name}");
        assert!(!name.contains("_clip_"), "filename should be compact: {name}");

        let gp = build_output_mp4_path(&base_tmp, "Counter-Strike 2", "session");
        assert_eq!(
            gp.parent(),
            Some(base_tmp.join("Sessions").join("Counter-Strike 2").as_path())
        );
        let gname = gp.file_name().and_then(|n| n.to_str()).unwrap_or("");
        assert!(gname.starts_with("Counter-Strike 2_"), "unexpected filename: {gname}");
        assert!(gname.ends_with(".mp4"), "unexpected filename: {gname}");

        let ep = build_output_mp4_path(&base_tmp, "Counter-Strike 2", "edited");
        assert_eq!(ep.parent(), Some(base_tmp.join("Edited").as_path()));
        let _ = fs::remove_dir_all(&base_tmp);
    }

    #[test]
    fn video_pipeline_needs_restart_detects_encoder_changes() {
        let base = AppSettings::default();

        // UI-only changes must NOT restart capture/encoder.
        let mut ui_only = base.clone();
        ui_only.language = AppLanguage::En;
        ui_only.theme = core_types::AppTheme::OledMidnight;
        ui_only.notifications.overlay_toast_enabled = false;
        assert!(!video_pipeline_needs_restart(&base, &ui_only));

        // Changing bitrate must trigger instant encoder reinit.
        let mut changed_bitrate = base.clone();
        changed_bitrate.video.bitrate_kbps = 2_000;
        assert!(video_pipeline_needs_restart(&base, &changed_bitrate));

        // Changing rate control must trigger instant encoder reinit.
        let mut changed_rc = base.clone();
        changed_rc.video.rate_control = core_types::RateControlMode::Cbr;
        assert!(video_pipeline_needs_restart(&base, &changed_rc));

        // Changing codec, vendor, fps, resolution, gop_seconds, or hdr_mode must trigger reinit.
        let mut changed_codec = base.clone();
        changed_codec.video.codec = core_types::VideoCodec::Hevc;
        assert!(video_pipeline_needs_restart(&base, &changed_codec));

        let mut changed_fps = base.clone();
        changed_fps.video.fps = 120;
        assert!(video_pipeline_needs_restart(&base, &changed_fps));

        let mut changed_res = base.clone();
        changed_res.video.resolution = core_types::ResolutionPreset::P720;
        assert!(video_pipeline_needs_restart(&base, &changed_res));

        let mut changed_hdr = base.clone();
        changed_hdr.capture.hdr_mode = core_types::HdrMode::ForceSdrClamp;
        assert!(video_pipeline_needs_restart(&base, &changed_hdr));
    }

    #[test]
    fn hw_encode_2mbps_vbr_dynamic_motion_respects_bitrate_budget(
    ) -> Result<(), Box<dyn std::error::Error>> {
        use core_audio::{target_aac_bitrate_kbps, MfAacEncoder, AAC_FRAME_SAMPLES};
        use core_capture::{D3d11SharedContext, GpuVideoProcessor};
        use core_mux::write_faststart_mp4;
        use core_types::{
            GpuVendorPreference, RateControlMode, StreamHeaders, VideoCodec,
        };
        use std::process::Command;
        use windows::Win32::Graphics::Direct3D11::{
            ID3D11Texture2D, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE,
            D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
        };
        use windows::Win32::Graphics::Dxgi::Common::{
            DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
        };

        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_test_writer()
            .try_init();

        let width = 1920u32;
        let height = 1080u32;
        let fps = 60u32;
        let frame_count = 300usize; // 5.0 seconds @ 60 fps

        let shared = D3d11SharedContext::new()?;
        let mut processor = GpuVideoProcessor::new(&shared)?;
        let mut video_enc = MfHardwareEncoder::new(
            &shared.device,
            MfVideoEncoderConfig {
                codec: VideoCodec::H264,
                vendor: GpuVendorPreference::Auto,
                rate_control: RateControlMode::Vbr,
                width,
                height,
                fps,
                bitrate_kbps: 2_000,
                gop_seconds: 2,
            },
        )?;

        let tex_desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut tex_opt: Option<ID3D11Texture2D> = None;
        unsafe {
            shared
                .device
                .CreateTexture2D(&tex_desc, None, Some(&mut tex_opt))?;
        }
        let bgra_tex = tex_opt.ok_or("CreateTexture2D(BGRA) returned null")?;

        let mut pixel_words = vec![0u32; (width * height) as usize];
        let mut video_header: Option<VideoStreamHeader> = None;
        let mut packets: Vec<MediaPacket> = Vec::new();
        let mut video_bytes: u64 = 0;
        let mut video_keyframes: usize = 0;

        let mut rng_state: u32 = 0x1357_9BDF;
        for frame_idx in 0..frame_count {
            let i = frame_idx as u32;
            let shift_x = (i * 17) % width;
            let shift_y = (i * 9) % height;
            let scene_phase = (i / 60) % 5;

            // Four fast-moving high-contrast game-like sprites moving across the frame.
            let box1_x = (i * 17) % (width - 280);
            let box1_y = (i * 9) % (height - 180);
            let box2_x = width - 260 - ((i * 23) % (width - 260));
            let box2_y = (120 + i * 13) % (height - 200);
            let box3_x = (400 + i * 31) % (width - 140);
            let box3_y = height - 180 - ((i * 19) % (height - 180));
            let box4_x = ((i * 13) + 700) % (width - 200);
            let box4_y = ((i * 21) + 200) % (height - 160);
            let hud_bar_w = ((i * 29) % 520) + 120;

            // Realistic high-motion game scene: scrolling textured grid, sprites, HUD, light film grain.
            for y_block in (0..height).step_by(4) {
                rng_state ^= rng_state << 13;
                rng_state ^= rng_state >> 17;
                rng_state ^= rng_state << 5;
                let row_noise = rng_state;

                let in_hud = y_block >= height - 64 && y_block < height - 24;
                for x_block in (0..width).step_by(4) {
                    let in_box1 = x_block >= box1_x
                        && x_block < box1_x + 280
                        && y_block >= box1_y
                        && y_block < box1_y + 180;
                    let in_box2 = x_block >= box2_x
                        && x_block < box2_x + 240
                        && y_block >= box2_y
                        && y_block < box2_y + 160;
                    let in_box3 = x_block >= box3_x
                        && x_block < box3_x + 140
                        && y_block >= box3_y
                        && y_block < box3_y + 140;
                    let in_box4 = x_block >= box4_x
                        && x_block < box4_x + 180
                        && y_block >= box4_y
                        && y_block < box4_y + 120;

                    let grain =
                        row_noise.wrapping_mul(x_block.wrapping_add(1)).rotate_left(y_block & 15)
                            & 0x07;

                    let (r, g, b) = if in_hud && x_block >= 40 && x_block < 40 + hud_bar_w {
                        (30 + grain, 210 + grain, 140 + grain)
                    } else if in_hud && (40..680).contains(&x_block) {
                        (24, 28, 36)
                    } else if in_box1 {
                        let edge = x_block == box1_x || y_block == box1_y;
                        if edge {
                            (250, 250, 250)
                        } else {
                            (215 + grain, 75 + ((i * 5) & 0x3F), 50 + grain)
                        }
                    } else if in_box2 {
                        (55 + grain, 155 + grain, 235 + (grain / 2))
                    } else if in_box3 {
                        (235 + grain, 195 + grain, 40 + grain)
                    } else if in_box4 {
                        (180 + grain, 70 + grain, 220 + grain)
                    } else {
                        // Fast-scrolling diagonal textured landscape with sharp tile borders.
                        let sx = x_block.wrapping_add(shift_x);
                        let sy = y_block.wrapping_add(shift_y);
                        let tile = ((sx / 96) ^ (sy / 96)) & 1;
                        let border = (sx % 96) < 4 || (sy % 96) < 4;
                        let grad_x = (sx % 256) / 4;
                        let grad_y = (sy % 256) / 4;
                        let tint = scene_phase * 22;
                        if border {
                            (45 + tint, 55, 70)
                        } else if tile != 0 {
                            (35 + grad_x + tint + grain, 55 + grad_y + grain, 90 + (grad_x / 2))
                        } else {
                            (20 + (grad_y / 2) + tint + grain, 40 + grad_x + grain, 65 + grad_y)
                        }
                    };
                    let px = 0xFF00_0000 | ((r & 0xFF) << 16) | ((g & 0xFF) << 8) | (b & 0xFF);

                    for dy in 0..4u32 {
                        let row_off = ((y_block + dy) * width + x_block) as usize;
                        for dx in 0..4usize {
                            pixel_words[row_off + dx] = px;
                        }
                    }
                }
            }

            unsafe {
                shared.context.UpdateSubresource(
                    &bgra_tex,
                    0,
                    None,
                    pixel_words.as_ptr().cast(),
                    width * 4,
                    0,
                );
            }

            let pts_us = (frame_idx as i64) * 1_000_000 / i64::from(fps);
            let nv12 = processor.convert_to_nv12(
                &bgra_tex,
                width,
                height,
                fps,
                pts_us,
                (frame_idx + 1) as u64,
                1,
            )?;

            for out in video_enc.encode_gpu_frame(&nv12)? {
                match out {
                    EncoderOutput::StreamHeader(h) => video_header = Some(h),
                    EncoderOutput::Packet(p) => {
                        if p.is_keyframe {
                            video_keyframes += 1;
                        }
                        video_bytes += p.size_bytes() as u64;
                        packets.push(p);
                    }
                }
            }
        }

        for out in video_enc.flush()? {
            match out {
                EncoderOutput::StreamHeader(h) => video_header = Some(h),
                EncoderOutput::Packet(p) => {
                    if p.is_keyframe {
                        video_keyframes += 1;
                    }
                    video_bytes += p.size_bytes() as u64;
                    packets.push(p);
                }
            }
        }

        // Encode 5.0 seconds of synthetic stereo audio across Mixed (128k), Game (128k), Mic (96k).
        let mixed_kbps = target_aac_bitrate_kbps(TrackKind::AudioMixed, 128);
        let game_kbps = target_aac_bitrate_kbps(TrackKind::AudioGame, 128);
        let mic_kbps = target_aac_bitrate_kbps(TrackKind::AudioMic, 128);
        let mut aac_mixed = MfAacEncoder::new(TrackKind::AudioMixed, mixed_kbps)?;
        let mut aac_game = MfAacEncoder::new(TrackKind::AudioGame, game_kbps)?;
        let mut aac_mic = MfAacEncoder::new(TrackKind::AudioMic, mic_kbps)?;
        let hdr_mixed = aac_mixed.stream_header().clone();
        let hdr_game = aac_game.stream_header().clone();
        let hdr_mic = aac_mic.stream_header().clone();
        assert_eq!(hdr_mixed.bitrate_bps, 128_000);
        assert_eq!(hdr_game.bitrate_bps, 128_000);
        assert_eq!(hdr_mic.bitrate_bps, 96_000);

        let mut pcm_block = vec![0i16; AAC_FRAME_SAMPLES * 2];
        for blk_idx in 0..235i64 {
            let pts_us = (blk_idx * 1_024 * 1_000_000) / 48_000;
            for s in 0..AAC_FRAME_SAMPLES {
                let sample = (((blk_idx as usize * AAC_FRAME_SAMPLES + s) % 120) as i16) * 80;
                pcm_block[s * 2] = sample;
                pcm_block[s * 2 + 1] = -sample;
            }
            packets.extend(aac_mixed.encode_pcm_block(&pcm_block, pts_us)?);
            packets.extend(aac_game.encode_pcm_block(&pcm_block, pts_us)?);
            packets.extend(aac_mic.encode_pcm_block(&pcm_block, pts_us)?);
        }
        packets.extend(aac_mixed.flush(5_000_000)?);
        packets.extend(aac_game.flush(5_000_000)?);
        packets.extend(aac_mic.flush(5_000_000)?);

        let headers = StreamHeaders {
            video: video_header.ok_or("Encoder did not emit VideoStreamHeader")?,
            audio_mixed: Some(hdr_mixed),
            audio_game: Some(hdr_game),
            audio_mic: Some(hdr_mic),
            audio_secondary: None,
            app_audio: Vec::new(),
        };

        let tmp_mp4 = std::env::temp_dir().join(format!("ikoch_2mbps_test_{}.mp4", Uuid::new_v4()));
        let file_size_bytes = write_faststart_mp4(&tmp_mp4, &headers, &packets)?;

        let duration_sec = f64::from(frame_count as u32) / f64::from(fps);
        let video_bitrate_mbps = (video_bytes as f64 * 8.0) / duration_sec / 1_000_000.0;
        let total_bitrate_mbps = (file_size_bytes as f64 * 8.0) / duration_sec / 1_000_000.0;
        let projected_30s_mb =
            (file_size_bytes as f64 / duration_sec) * 30.0 / (1024.0 * 1024.0);

        println!(
            "=== 2 Mbps VBR Dynamic Motion Verification ===\n\
             Encoder: {}\n\
             Duration: {:.2}s ({} frames @ {} fps, GOP=2s, keyframes={})\n\
             Video Elementary Bytes: {} B -> {:.3} Mbps\n\
             Muxed MP4 (Video + 3x AAC) Size: {} B -> Total {:.3} Mbps\n\
             Projected 30s Clip Size: {:.2} MiB",
            video_enc.friendly_name(),
            duration_sec,
            frame_count,
            fps,
            video_keyframes,
            video_bytes,
            video_bitrate_mbps,
            file_size_bytes,
            total_bitrate_mbps,
            projected_30s_mb,
        );

        let ffprobe_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../src-tauri/binaries/ffprobe-x86_64-pc-windows-msvc.exe");
        if ffprobe_path.exists() {
            let probe_out = Command::new(&ffprobe_path)
                .args([
                    "-v",
                    "error",
                    "-show_entries",
                    "format=duration,size,bit_rate",
                    "-show_entries",
                    "stream=index,codec_type,codec_name,profile,bit_rate",
                    "-of",
                    "default=noprint_wrappers=1",
                ])
                .arg(&tmp_mp4)
                .output()?;
            println!(
                "--- ffprobe output ---\n{}",
                String::from_utf8_lossy(&probe_out.stdout)
            );
            assert!(
                probe_out.status.success(),
                "ffprobe failed: {}",
                String::from_utf8_lossy(&probe_out.stderr)
            );
        }

        let _ = fs::remove_file(&tmp_mp4);

        assert!(
            video_keyframes <= 5,
            "expected GOP=2s (~3 keyframes in 5s), got {video_keyframes}"
        );
        assert!(
            (1.0..=2.8).contains(&video_bitrate_mbps),
            "video bitrate {video_bitrate_mbps:.3} Mbps outside expected [1.0, 2.8] Mbps range"
        );
        assert!(
            projected_30s_mb <= 11.5,
            "projected 30s size {projected_30s_mb:.2} MiB exceeds 11.5 MiB budget"
        );

        Ok(())
    }
}
