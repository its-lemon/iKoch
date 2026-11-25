//! Multi-stream WASAPI audio capture threads (`Game Loopback` + `Microphone` + optional
//! `Secondary Loopback`) running at MMCSS `"Pro Audio"` priority + `timeBeginPeriod(1)`,
//! synchronized via a startup barrier (`t0`), feeding a lock-free multi-track mixer (`AudioMixed`,
//! `tkhd flags=0x03`) alongside clean stem tracks (`AudioGame`, `AudioMic`, `AudioSecondary`,
//! `tkhd flags=0x02`), pure-Rust RNNoise mic denoising in the mic thread, and Media Foundation AAC MFT.

use crate::aac_encoder::{elapsed_48k_frames, sample_index_to_pts_us, MfAacEncoder};
use crate::denoise::{mix_stereo_blocks_soft_clip, MicNoiseProcessor};
use crate::process_loopback::{
    find_best_pid_for_processes, is_pid_running, query_process_exe_name, ProcessLoopbackStream,
};
use crate::resampler::{
    float_frame_to_i16, linear_to_dbfs, Stereo48kResampler, AAC_FRAME_SAMPLES, TARGET_CHANNELS,
    TARGET_SAMPLE_RATE_HZ,
};
use core_types::{
    AppAudioSlotConfig, AudioError, AudioOutputMode, AudioSettings, AudioStreamHeader, MediaPacket,
    MicActivationMode, MicDeviceConfig, MonotonicClock, TrackKind, AUDIO_CHUNK_CHANNEL_CAPACITY,
};
use crossbeam_channel::{bounded, Receiver, Sender, TryRecvError};
use parking_lot::{Condvar, Mutex};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};
use wasapi::{AudioCaptureClient, AudioClient, DeviceEnumerator, Direction, Handle, Role, StreamMode, WaveFormat};
use windows::core::w;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CAPITAL, VK_CONTROL, VK_F1, VK_F10, VK_F11, VK_F12, VK_F2, VK_F3, VK_F4,
    VK_F5, VK_F6, VK_F7, VK_F8, VK_F9, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_MBUTTON, VK_MENU,
    VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_SHIFT, VK_SPACE, VK_TAB, VK_XBUTTON1, VK_XBUTTON2,
};

const STEREO_BLOCK_SAMPLES: usize = AAC_FRAME_SAMPLES * TARGET_CHANNELS;
const MAX_STAGED_BLOCKS_PER_SOURCE: usize = 32;
/// Requested shared-mode WASAPI loopback/capture ring buffer duration (80 ms in 100-ns HNS units).
const WASAPI_BUFFER_DURATION_HNS: i64 = 800_000;
/// Shared deadline before all source threads + mixer start counting blocks (`t0`).
const STARTUP_BARRIER_TIMEOUT: Duration = Duration::from_millis(300);
/// Idle loopback silence injection threshold (avoids false silence on normal 10-20 ms WASAPI jitter).
const IDLE_SILENCE_THRESHOLD: Duration = Duration::from_millis(110);
/// Duration of one 1024-sample block at 48 kHz in microseconds (`1024 * 1_000_000 / 48_000`).
const BLOCK_DURATION_US: u128 = 21_333;
/// Mixer deadline beyond slot due time before emitting an incomplete mix slot (`120 ms`).
const MIXER_SLOT_LATE_DEADLINE_US: i64 = 120_000;
/// Backoff between WASAPI reopen attempts on disconnect / unavailable device.
const WASAPI_RECONNECT_BACKOFF: Duration = Duration::from_millis(750);
/// Jitter buffer depth (blocks of 21.33 ms): ~85 ms tolerance so late `Game` blocks align cleanly.
const MIXER_JITTER_BUFFER_BLOCKS: usize = 4;

/// RAII guard configuring MTA COM, 1 ms OS timer resolution, and Windows MMCSS `"Pro Audio"` priority.
struct AudioThreadGuard {
    com_initialized: bool,
    timer_active: bool,
    avrt_handle: Option<HANDLE>,
}

impl AudioThreadGuard {
    fn enter_pro_audio(enable_high_res_timer: bool) -> Self {
        let com_hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let com_initialized = com_hr.is_ok();

        let timer_active = if enable_high_res_timer {
            unsafe { timeBeginPeriod(1) == 0 }
        } else {
            false
        };

        let mut task_index = 0u32;
        let avrt_handle =
            unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut task_index) }.ok();

        Self {
            com_initialized,
            timer_active,
            avrt_handle,
        }
    }
}

impl Drop for AudioThreadGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.avrt_handle.take() {
            let _ = unsafe { AvRevertMmThreadCharacteristics(handle) };
        }
        if self.timer_active {
            let _ = unsafe { timeEndPeriod(1) };
        }
        if self.com_initialized {
            unsafe { CoUninitialize() };
        }
    }
}

/// Lock-free startup barrier aligning `Game`, `Mic`, optional `Secondary`, and `Mixer` at shared `t0`.
struct StartupBarrierState {
    expected: usize,
    ready: usize,
    released: bool,
    base_pts_us: i64,
}

struct StartupBarrier {
    mutex: Mutex<StartupBarrierState>,
    cv: Condvar,
}

impl StartupBarrier {
    fn new(expected: usize, fallback_pts_us: i64) -> Self {
        Self {
            mutex: Mutex::new(StartupBarrierState {
                expected,
                ready: 0,
                released: expected == 0,
                base_pts_us: fallback_pts_us,
            }),
            cv: Condvar::new(),
        }
    }

    /// Marks an initial active capture stream as started and waits up to `timeout` for peers.
    fn mark_ready_and_wait(&self, clock: MonotonicClock, timeout: Duration) -> i64 {
        let deadline = Instant::now() + timeout;
        let mut guard = self.mutex.lock();
        guard.ready = guard.ready.saturating_add(1);
        if guard.ready >= guard.expected && !guard.released {
            guard.released = true;
            guard.base_pts_us = clock.now_us();
            self.cv.notify_all();
            return guard.base_pts_us;
        }
        while !guard.released {
            let wait_res = self.cv.wait_until(&mut guard, deadline);
            if wait_res.timed_out() && !guard.released {
                guard.released = true;
                guard.base_pts_us = clock.now_us();
                self.cv.notify_all();
                break;
            }
        }
        guard.base_pts_us
    }

    /// Waits without incrementing `ready` (used by mixer and intentionally-idle optional slots).
    fn wait_for_base_pts(&self, clock: MonotonicClock, timeout: Duration) -> i64 {
        let deadline = Instant::now() + timeout;
        let mut guard = self.mutex.lock();
        while !guard.released {
            let wait_res = self.cv.wait_until(&mut guard, deadline);
            if wait_res.timed_out() && !guard.released {
                guard.released = true;
                guard.base_pts_us = clock.now_us();
                self.cv.notify_all();
                break;
            }
        }
        guard.base_pts_us
    }
}

/// Live diagnostic telemetry per audio track (`Mixed`, `Game`, `Mic`, optional `Secondary`).
#[derive(Debug, Clone)]
pub struct AudioTrackTelemetry {
    pub active_device: String,
    pub real_frames_read: u64,
    pub silent_frames_injected: u64,
    pub silent_injected_blocks: u64,
    pub discontinuities: u64,
    pub mixer_underruns: u64,
    pub encoded_packets: u64,
    pub peak_linear: f32,
    pub peak_dbfs: f32,
}

impl Default for AudioTrackTelemetry {
    fn default() -> Self {
        Self {
            active_device: "Uninitialized".to_string(),
            real_frames_read: 0,
            silent_frames_injected: 0,
            silent_injected_blocks: 0,
            discontinuities: 0,
            mixer_underruns: 0,
            encoded_packets: 0,
            peak_linear: 0.0,
            peak_dbfs: -100.0,
        }
    }
}

/// Lock-free atomic backing store so hot audio paths never wait on a mutex.
struct TrackTelemetryShared {
    active_device: Mutex<String>,
    real_frames_read: AtomicU64,
    silent_frames_injected: AtomicU64,
    silent_injected_blocks: AtomicU64,
    discontinuities: AtomicU64,
    mixer_underruns: AtomicU64,
    encoded_packets: AtomicU64,
    peak_linear_bits: AtomicU32,
}

impl TrackTelemetryShared {
    fn new(initial_device: &str) -> Self {
        Self {
            active_device: Mutex::new(initial_device.to_string()),
            real_frames_read: AtomicU64::new(0),
            silent_frames_injected: AtomicU64::new(0),
            silent_injected_blocks: AtomicU64::new(0),
            discontinuities: AtomicU64::new(0),
            mixer_underruns: AtomicU64::new(0),
            encoded_packets: AtomicU64::new(0),
            peak_linear_bits: AtomicU32::new(0.0f32.to_bits()),
        }
    }

    fn set_active_device(&self, name: impl Into<String>) {
        *self.active_device.lock() = name.into();
    }

    #[inline]
    fn add_real_frames(&self, frames: u64) {
        self.real_frames_read.fetch_add(frames, Ordering::Relaxed);
    }

    #[inline]
    fn add_silent_block(&self, frames: u64) {
        self.silent_injected_blocks.fetch_add(1, Ordering::Relaxed);
        self.silent_frames_injected
            .fetch_add(frames, Ordering::Relaxed);
    }

    #[inline]
    fn inc_discontinuities(&self) {
        self.discontinuities.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    fn inc_mixer_underruns(&self) {
        self.mixer_underruns.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    fn inc_encoded_packets(&self) {
        self.encoded_packets.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    fn observe_peak_f32(&self, samples: &[f32]) {
        let mut max_abs = 0.0f32;
        for &s in samples {
            let abs = if s.is_finite() { s.abs() } else { 0.0 };
            if abs > max_abs {
                max_abs = abs;
            }
        }
        let linear = max_abs.min(1.0);
        let mut cur_bits = self.peak_linear_bits.load(Ordering::Relaxed);
        while linear > f32::from_bits(cur_bits) {
            match self.peak_linear_bits.compare_exchange_weak(
                cur_bits,
                linear.to_bits(),
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => cur_bits = actual,
            }
        }
    }

    fn snapshot(&self) -> AudioTrackTelemetry {
        let active_device = self.active_device.lock().clone();
        let peak_linear = f32::from_bits(self.peak_linear_bits.load(Ordering::Relaxed));
        AudioTrackTelemetry {
            active_device,
            real_frames_read: self.real_frames_read.load(Ordering::Relaxed),
            silent_frames_injected: self.silent_frames_injected.load(Ordering::Relaxed),
            silent_injected_blocks: self.silent_injected_blocks.load(Ordering::Relaxed),
            discontinuities: self.discontinuities.load(Ordering::Relaxed),
            mixer_underruns: self.mixer_underruns.load(Ordering::Relaxed),
            encoded_packets: self.encoded_packets.load(Ordering::Relaxed),
            peak_linear,
            peak_dbfs: linear_to_dbfs(peak_linear),
        }
    }
}

/// Combined telemetry snapshot across all active audio tracks.
#[derive(Debug, Clone, Default)]
pub struct AudioPipelineTelemetry {
    pub mixed: AudioTrackTelemetry,
    pub game: AudioTrackTelemetry,
    pub mic: AudioTrackTelemetry,
    pub secondary: Option<AudioTrackTelemetry>,
    pub apps: Vec<(u8, String, AudioTrackTelemetry)>,
}

/// Hot-reconfiguration commands for the multi-track audio pipeline.
#[derive(Debug, Clone)]
pub enum AudioControlCommand {
    SetOutputMode(AudioOutputMode),
    SetMicDevice(MicDeviceConfig),
    SetSecondaryOutputDevice(Option<String>),
    SetTargetProcessId(Option<u32>),
    SetVolumes {
        game_volume: f32,
        mic_volume: f32,
        secondary_volume: Option<f32>,
    },
    SetGameMuted(bool),
    SetMicMuted(bool),
    SetSecondaryMuted(bool),
    SetMicNoiseSuppression {
        enabled: bool,
        strength: f32,
        noise_gate_db: Option<f32>,
    },
    SetMicActivationMode(MicActivationMode),
    SetMicSoftLimiter(bool),
    Stop,
}

/// Output stream headers and AAC packets produced by `core-audio`.
#[derive(Debug, Clone)]
pub enum AudioOutputEvent {
    StreamHeader(AudioStreamHeader),
    Packet(MediaPacket),
    RecoverableWarning(String),
}

struct AppSlotRuntime {
    slot_idx: u8,
    config: AppAudioSlotConfig,
    telemetry: Arc<TrackTelemetryShared>,
    running: Arc<AtomicBool>,
    cmd_tx: Option<Sender<SourceControl>>,
}

/// Handle controlling WASAPI source threads and the central synchronized mixer/AAC encoder thread.
pub struct AudioPipelineHandle {
    game_cmd_tx: Sender<SourceControl>,
    mic_cmd_tx: Sender<SourceControl>,
    sec_cmd_tx: Sender<SourceControl>,
    mixer_cmd_tx: Sender<MixerControl>,
    pcm_tx: Sender<CapturedBlock>,
    out_tx: Sender<AudioOutputEvent>,
    clock: MonotonicClock,
    base_pts_us: Arc<AtomicI64>,
    stop_flag: Arc<AtomicBool>,
    mixed_telemetry: Arc<TrackTelemetryShared>,
    game_telemetry: Arc<TrackTelemetryShared>,
    mic_telemetry: Arc<TrackTelemetryShared>,
    sec_telemetry: Arc<TrackTelemetryShared>,
    has_secondary_track: Arc<AtomicBool>,
    app_slots: Mutex<Vec<AppSlotRuntime>>,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl AudioPipelineHandle {
    /// Applies a new game/system loopback output mode on the fly.
    pub fn set_output_mode(&self, mode: AudioOutputMode) {
        let _ = self.game_cmd_tx.send(SourceControl::SetOutputMode(mode));
    }

    /// Applies a new microphone device selection on the fly.
    pub fn set_mic_device(&self, cfg: MicDeviceConfig) {
        let _ = self.mic_cmd_tx.send(SourceControl::SetMicDevice(cfg));
    }

    /// Updates the optional secondary loopback device ID on the fly.
    pub fn set_secondary_output_device(&self, device_id: Option<String>) {
        let _ = self
            .sec_cmd_tx
            .send(SourceControl::SetSecondaryDevice(device_id));
    }

    /// Updates the target game PID used by [`AudioOutputMode::GameProcessOnly`].
    pub fn set_target_process_id(&self, pid: Option<u32>) {
        let _ = self
            .game_cmd_tx
            .send(SourceControl::SetTargetProcessId(pid));
    }

    /// Updates live mix volumes applied when summing `TrackKind::AudioMixed`.
    pub fn set_volumes(&self, game_volume: f32, mic_volume: f32, secondary_volume: Option<f32>) {
        let _ = self.mixer_cmd_tx.send(MixerControl::SetVolumes {
            game_volume,
            mic_volume,
            secondary_volume,
        });
    }

    /// Updates game audio mute state in the `AudioMixed` bus on the fly.
    pub fn set_game_muted(&self, muted: bool) {
        let _ = self.mixer_cmd_tx.send(MixerControl::SetGameMuted(muted));
    }

    /// Updates microphone mute state in the `AudioMixed` bus on the fly.
    pub fn set_mic_muted(&self, muted: bool) {
        let _ = self.mixer_cmd_tx.send(MixerControl::SetMicMuted(muted));
    }

    /// Updates secondary output mute state in the `AudioMixed` bus on the fly.
    pub fn set_secondary_muted(&self, muted: bool) {
        let _ = self
            .mixer_cmd_tx
            .send(MixerControl::SetSecondaryMuted(muted));
    }

    /// Updates pure-Rust RNNoise microphone suppression and noise gate directly on the Mic thread.
    pub fn set_mic_noise_suppression(
        &self,
        enabled: bool,
        strength: f32,
        noise_gate_db: Option<f32>,
    ) {
        let _ = self
            .mic_cmd_tx
            .send(SourceControl::SetMicNoiseSuppression {
                enabled,
                strength,
                noise_gate_db,
            });
    }

    /// Updates Push-to-Talk / Push-to-Mute / AlwaysOn activation mode on the mic capture thread.
    pub fn set_mic_activation_mode(&self, mode: MicActivationMode) {
        let _ = self
            .mic_cmd_tx
            .send(SourceControl::SetMicActivationMode(mode));
    }

    /// Enables or disables the `-0.5 dBFS` soft peak limiter directly on the Mic thread.
    pub fn set_mic_soft_limiter(&self, enabled: bool) {
        let _ = self
            .mic_cmd_tx
            .send(SourceControl::SetMicSoftLimiter(enabled));
    }

    /// Applies an [`AudioControlCommand`] uniformly.
    pub fn send_command(&self, cmd: AudioControlCommand) {
        match cmd {
            AudioControlCommand::SetOutputMode(mode) => self.set_output_mode(mode),
            AudioControlCommand::SetMicDevice(cfg) => self.set_mic_device(cfg),
            AudioControlCommand::SetSecondaryOutputDevice(device_id) => {
                self.set_secondary_output_device(device_id)
            }
            AudioControlCommand::SetTargetProcessId(pid) => self.set_target_process_id(pid),
            AudioControlCommand::SetVolumes {
                game_volume,
                mic_volume,
                secondary_volume,
            } => self.set_volumes(game_volume, mic_volume, secondary_volume),
            AudioControlCommand::SetGameMuted(muted) => self.set_game_muted(muted),
            AudioControlCommand::SetMicMuted(muted) => self.set_mic_muted(muted),
            AudioControlCommand::SetSecondaryMuted(muted) => self.set_secondary_muted(muted),
            AudioControlCommand::SetMicNoiseSuppression {
                enabled,
                strength,
                noise_gate_db,
            } => self.set_mic_noise_suppression(enabled, strength, noise_gate_db),
            AudioControlCommand::SetMicActivationMode(mode) => self.set_mic_activation_mode(mode),
            AudioControlCommand::SetMicSoftLimiter(enabled) => self.set_mic_soft_limiter(enabled),
            AudioControlCommand::Stop => {
                self.stop_flag.store(true, Ordering::SeqCst);
            }
        }
    }

    /// Polls idle enabled app slots and spawns capture threads only when a matching process is running.
    pub fn poll_idle_app_slots(&self) {
        if self.stop_flag.load(Ordering::Relaxed) {
            return;
        }
        let mut slots = self.app_slots.lock();
        let mut threads = self.threads.lock();
        for slot in slots.iter_mut() {
            if !slot.config.enabled
                || slot.config.process_names.is_empty()
                || slot.running.load(Ordering::Relaxed)
            {
                continue;
            }
            if let Ok(Some(session)) = find_best_pid_for_processes(&slot.config.process_names) {
                let (cmd_tx, cmd_rx) = bounded(8);
                slot.running.store(true, Ordering::Relaxed);
                slot.cmd_tx = Some(cmd_tx);
                let slot_idx = slot.slot_idx;
                let process_names = slot.config.process_names.clone();
                let label = slot.config.label.clone();
                let tel = Arc::clone(&slot.telemetry);
                let running = Arc::clone(&slot.running);
                let pcm_tx = self.pcm_tx.clone();
                let warn_tx = self.out_tx.clone();
                let clock = self.clock;
                let base_pts = Arc::clone(&self.base_pts_us);
                let stop = Arc::clone(&self.stop_flag);
                let initial_pid = session.pid;
                if let Ok(handle) = thread::Builder::new()
                    .name(format!("rustclip-audio-app-{slot_idx}"))
                    .spawn(move || {
                        run_app_slot_capture_loop(
                            slot_idx,
                            label,
                            process_names,
                            initial_pid,
                            base_pts,
                            clock,
                            cmd_rx,
                            pcm_tx,
                            warn_tx,
                            tel,
                            running,
                            stop,
                        );
                    })
                {
                    threads.push(handle);
                } else {
                    slot.running.store(false, Ordering::Relaxed);
                }
            }
        }
    }

    /// Returns a live lock-free telemetry snapshot across `Mixed`, `Game`, `Mic`, `Secondary`, and `App` tracks.
    #[must_use]
    pub fn telemetry(&self) -> AudioPipelineTelemetry {
        let secondary = if self.has_secondary_track.load(Ordering::Relaxed) {
            Some(self.sec_telemetry.snapshot())
        } else {
            None
        };
        let apps = self
            .app_slots
            .lock()
            .iter()
            .filter(|s| s.config.enabled)
            .map(|s| (s.slot_idx, s.config.label.clone(), s.telemetry.snapshot()))
            .collect();
        AudioPipelineTelemetry {
            mixed: self.mixed_telemetry.snapshot(),
            game: self.game_telemetry.snapshot(),
            mic: self.mic_telemetry.snapshot(),
            secondary,
            apps,
        }
    }

    /// Stops all audio capture/mixer threads cleanly and waits for them to join.
    pub fn stop(mut self) {
        self.shutdown_internal();
    }

    fn shutdown_internal(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        let _ = self.game_cmd_tx.send(SourceControl::Stop);
        let _ = self.mic_cmd_tx.send(SourceControl::Stop);
        let _ = self.sec_cmd_tx.send(SourceControl::Stop);
        for slot in self.app_slots.lock().iter() {
            if let Some(tx) = slot.cmd_tx.as_ref() {
                let _ = tx.send(SourceControl::Stop);
            }
        }
        let _ = self.mixer_cmd_tx.send(MixerControl::Stop);
        let drained: Vec<JoinHandle<()>> = self.threads.lock().drain(..).collect();
        for h in drained {
            let _ = h.join();
        }
    }
}

impl Drop for AudioPipelineHandle {
    fn drop(&mut self) {
        self.shutdown_internal();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceSlot {
    Game,
    Mic,
    Secondary,
    App(u8),
}

#[derive(Debug, Clone)]
enum SourceControl {
    SetOutputMode(AudioOutputMode),
    SetMicDevice(MicDeviceConfig),
    SetMicActivationMode(MicActivationMode),
    SetMicNoiseSuppression {
        enabled: bool,
        strength: f32,
        noise_gate_db: Option<f32>,
    },
    SetMicSoftLimiter(bool),
    SetSecondaryDevice(Option<String>),
    SetTargetProcessId(Option<u32>),
    Stop,
}

#[derive(Debug, Clone)]
enum MixerControl {
    SetVolumes {
        game_volume: f32,
        mic_volume: f32,
        secondary_volume: Option<f32>,
    },
    SetGameMuted(bool),
    SetMicMuted(bool),
    SetSecondaryMuted(bool),
    Stop,
}

#[derive(Debug, Clone)]
struct MicDspConfig {
    activation_mode: MicActivationMode,
    noise_suppression: bool,
    noise_suppression_strength: f32,
    noise_gate_db: Option<f32>,
    soft_limiter: bool,
}

struct CapturedBlock {
    slot: SourceSlot,
    block_idx: u64,
    samples: [f32; STEREO_BLOCK_SAMPLES],
}

/// Spawns WASAPI source capture threads (`Game`, `Mic`, optional `Secondary`) and the
/// central synchronized mixer + AAC encoder thread aligned at a shared startup barrier (`t0`).
pub fn start_audio_pipeline(
    settings: AudioSettings,
    target_pid: Option<u32>,
    clock: MonotonicClock,
) -> Result<(AudioPipelineHandle, Receiver<AudioOutputEvent>), AudioError> {
    let fallback_pts_us = clock.now_us();

    let (out_tx, out_rx) = bounded(AUDIO_CHUNK_CHANNEL_CAPACITY * 6);
    let (pcm_tx, pcm_rx) = bounded::<CapturedBlock>(AUDIO_CHUNK_CHANNEL_CAPACITY * 4);

    let (game_cmd_tx, game_cmd_rx) = bounded(16);
    let (mic_cmd_tx, mic_cmd_rx) = bounded(16);
    let (sec_cmd_tx, sec_cmd_rx) = bounded(16);
    let (mixer_cmd_tx, mixer_cmd_rx) = bounded(32);

    let stop_flag = Arc::new(AtomicBool::new(false));
    let has_secondary_track =
        Arc::new(AtomicBool::new(settings.secondary_output_device.is_some()));

    let game_active = settings.game_enabled;
    let mic_active = settings.mic_enabled && settings.mic_device != MicDeviceConfig::None;
    let sec_active = settings.secondary_output_device.is_some();

    let expected_initial_sources =
        usize::from(game_active) + usize::from(mic_active) + usize::from(sec_active);
    let startup_barrier = Arc::new(StartupBarrier::new(
        expected_initial_sources,
        fallback_pts_us,
    ));

    let mixed_telemetry = Arc::new(TrackTelemetryShared::new("Mixdown (48 kHz Stereo AAC)"));
    let game_telemetry = Arc::new(TrackTelemetryShared::new("Uninitialized"));
    let mic_telemetry = Arc::new(TrackTelemetryShared::new("Uninitialized"));
    let sec_telemetry = Arc::new(TrackTelemetryShared::new("Uninitialized"));

    let mic_dsp_cfg = MicDspConfig {
        activation_mode: settings.mic_activation_mode.clone(),
        noise_suppression: settings.mic_noise_suppression,
        noise_suppression_strength: settings.mic_noise_suppression_strength,
        noise_gate_db: settings.mic_noise_gate_db,
        soft_limiter: settings.mic_soft_limiter,
    };

    let mut threads = Vec::with_capacity(4);

    if settings.game_enabled {
        let tx = pcm_tx.clone();
        let warn_tx = out_tx.clone();
        let stop = Arc::clone(&stop_flag);
        let tel = Arc::clone(&game_telemetry);
        let barrier = Arc::clone(&startup_barrier);
        let mode = settings.audio_output_mode.clone();
        let mic_dsp = mic_dsp_cfg.clone();
        let handle = thread::Builder::new()
            .name("rustclip-audio-game".to_string())
            .spawn(move || {
                run_source_capture_loop(
                    SourceRole::GameLoopback { mode, target_pid },
                    mic_dsp,
                    barrier,
                    clock,
                    game_cmd_rx,
                    tx,
                    warn_tx,
                    tel,
                    stop,
                );
            })
            .map_err(|e| AudioError::WasapiInitFailed(format!("Spawn game audio failed: {e}")))?;
        threads.push(handle);
    }

    if settings.mic_enabled {
        let tx = pcm_tx.clone();
        let warn_tx = out_tx.clone();
        let stop = Arc::clone(&stop_flag);
        let tel = Arc::clone(&mic_telemetry);
        let barrier = Arc::clone(&startup_barrier);
        let device = settings.mic_device.clone();
        let mic_dsp = mic_dsp_cfg.clone();
        let handle = thread::Builder::new()
            .name("rustclip-audio-mic".to_string())
            .spawn(move || {
                run_source_capture_loop(
                    SourceRole::Microphone { device },
                    mic_dsp,
                    barrier,
                    clock,
                    mic_cmd_rx,
                    tx,
                    warn_tx,
                    tel,
                    stop,
                );
            })
            .map_err(|e| AudioError::WasapiInitFailed(format!("Spawn mic audio failed: {e}")))?;
        threads.push(handle);
    }

    {
        let tx = pcm_tx.clone();
        let warn_tx = out_tx.clone();
        let stop = Arc::clone(&stop_flag);
        let tel = Arc::clone(&sec_telemetry);
        let barrier = Arc::clone(&startup_barrier);
        let device_id = settings.secondary_output_device.clone();
        let mic_dsp = mic_dsp_cfg.clone();
        let handle = thread::Builder::new()
            .name("rustclip-audio-secondary".to_string())
            .spawn(move || {
                run_source_capture_loop(
                    SourceRole::SecondaryLoopback { device_id },
                    mic_dsp,
                    barrier,
                    clock,
                    sec_cmd_rx,
                    tx,
                    warn_tx,
                    tel,
                    stop,
                );
            })
            .map_err(|e| {
                AudioError::WasapiInitFailed(format!("Spawn secondary audio failed: {e}"))
            })?;
        threads.push(handle);
    }

    let base_pts_atomic = Arc::new(AtomicI64::new(fallback_pts_us));

    let mut app_slots_runtime = Vec::new();
    let mut app_slots_mixer = Vec::new();
    for (idx, slot_cfg) in settings.app_audio_slots.iter().take(16).enumerate() {
        let slot_idx = idx as u8;
        let tel = Arc::new(TrackTelemetryShared::new(&format!(
            "App: {}",
            slot_cfg.label
        )));
        let running = Arc::new(AtomicBool::new(false));
        let mut cmd_tx_opt = None;

        if slot_cfg.enabled && !slot_cfg.process_names.is_empty() {
            if let Ok(Some(session)) = find_best_pid_for_processes(&slot_cfg.process_names) {
                let (cmd_tx, cmd_rx) = bounded(8);
                running.store(true, Ordering::Relaxed);
                cmd_tx_opt = Some(cmd_tx);
                let process_names = slot_cfg.process_names.clone();
                let label = slot_cfg.label.clone();
                let tel_c = Arc::clone(&tel);
                let run_c = Arc::clone(&running);
                let tx_c = pcm_tx.clone();
                let warn_c = out_tx.clone();
                let base_c = Arc::clone(&base_pts_atomic);
                let stop_c = Arc::clone(&stop_flag);
                let initial_pid = session.pid;
                if let Ok(handle) = thread::Builder::new()
                    .name(format!("rustclip-audio-app-{slot_idx}"))
                    .spawn(move || {
                        run_app_slot_capture_loop(
                            slot_idx,
                            label,
                            process_names,
                            initial_pid,
                            base_c,
                            clock,
                            cmd_rx,
                            tx_c,
                            warn_c,
                            tel_c,
                            run_c,
                            stop_c,
                        );
                    })
                {
                    threads.push(handle);
                } else {
                    running.store(false, Ordering::Relaxed);
                }
            }
        }

        app_slots_runtime.push(AppSlotRuntime {
            slot_idx,
            config: slot_cfg.clone(),
            telemetry: Arc::clone(&tel),
            running,
            cmd_tx: cmd_tx_opt,
        });
        app_slots_mixer.push((slot_idx, slot_cfg.clone(), tel));
    }

    {
        let stop = Arc::clone(&stop_flag);
        let mixed_tel = Arc::clone(&mixed_telemetry);
        let game_tel = Arc::clone(&game_telemetry);
        let mic_tel = Arc::clone(&mic_telemetry);
        let sec_tel = Arc::clone(&sec_telemetry);
        let barrier = Arc::clone(&startup_barrier);
        let sec_flag = Arc::clone(&has_secondary_track);
        let cfg = settings.clone();
        let out_tx_m = out_tx.clone();
        let base_pts_m = Arc::clone(&base_pts_atomic);
        let handle = thread::Builder::new()
            .name("rustclip-audio-mixer".to_string())
            .spawn(move || {
                run_mixer_loop(
                    cfg,
                    barrier,
                    base_pts_m,
                    clock,
                    pcm_rx,
                    mixer_cmd_rx,
                    out_tx_m,
                    mixed_tel,
                    game_tel,
                    mic_tel,
                    sec_tel,
                    sec_flag,
                    app_slots_mixer,
                    stop,
                );
            })
            .map_err(|e| AudioError::WasapiInitFailed(format!("Spawn audio mixer failed: {e}")))?;
        threads.push(handle);
    }

    Ok((
        AudioPipelineHandle {
            game_cmd_tx,
            mic_cmd_tx,
            sec_cmd_tx,
            mixer_cmd_tx,
            pcm_tx,
            out_tx,
            clock,
            base_pts_us: base_pts_atomic,
            stop_flag,
            mixed_telemetry,
            game_telemetry,
            mic_telemetry,
            sec_telemetry,
            has_secondary_track,
            app_slots: Mutex::new(app_slots_runtime),
            threads: Mutex::new(threads),
        },
        out_rx,
    ))
}

#[derive(Debug, Clone)]
enum SourceRole {
    GameLoopback {
        mode: AudioOutputMode,
        target_pid: Option<u32>,
    },
    Microphone {
        device: MicDeviceConfig,
    },
    SecondaryLoopback {
        device_id: Option<String>,
    },
}

impl SourceRole {
    const fn slot(&self) -> SourceSlot {
        match self {
            Self::GameLoopback { .. } => SourceSlot::Game,
            Self::Microphone { .. } => SourceSlot::Mic,
            Self::SecondaryLoopback { .. } => SourceSlot::Secondary,
        }
    }

    fn is_intentionally_idle(&self) -> bool {
        match self {
            Self::Microphone { device } => *device == MicDeviceConfig::None,
            Self::SecondaryLoopback { device_id } => device_id.is_none(),
            Self::GameLoopback { .. } => false,
        }
    }
}

struct OpenedWasapiStream {
    audio_client: AudioClient,
    capture_client: AudioCaptureClient,
    event_handle: Option<Handle>,
    resampler: Stereo48kResampler,
    bytes_per_frame: usize,
    description: String,
    scratch: Vec<u8>,
    first_live_packet_seen: bool,
}

impl OpenedWasapiStream {
    fn drain_stale_and_reset_resampler(&mut self) {
        let _ = drain_wasapi_packets(
            &self.capture_client,
            &mut self.scratch,
            self.bytes_per_frame,
        );
        self.resampler.clear();
        self.first_live_packet_seen = false;
    }
}

fn drain_wasapi_packets(
    capture_client: &AudioCaptureClient,
    scratch: &mut Vec<u8>,
    bytes_per_frame: usize,
) -> Result<(), AudioError> {
    while let Some(avail_frames) = capture_client
        .get_next_packet_size()
        .map_err(|e| AudioError::WasapiInitFailed(format!("get_next_packet_size failed: {e}")))?
    {
        if avail_frames == 0 {
            break;
        }
        let needed_bytes = avail_frames as usize * bytes_per_frame.max(1);
        if scratch.len() < needed_bytes {
            scratch.resize(needed_bytes, 0);
        }
        capture_client
            .read_from_device(&mut scratch[..needed_bytes])
            .map_err(|e| AudioError::WasapiInitFailed(format!("read_from_device drain failed: {e}")))?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_source_capture_loop(
    mut role: SourceRole,
    mic_dsp_cfg: MicDspConfig,
    startup: Arc<StartupBarrier>,
    clock: MonotonicClock,
    cmd_rx: Receiver<SourceControl>,
    pcm_tx: Sender<CapturedBlock>,
    warn_tx: Sender<AudioOutputEvent>,
    telemetry: Arc<TrackTelemetryShared>,
    stop_flag: Arc<AtomicBool>,
) {
    let _thread_guard = AudioThreadGuard::enter_pro_audio(true);
    let slot = role.slot();

    let mut ptt_gate = PttGateState::new(mic_dsp_cfg.activation_mode);
    let mut mic_denoiser = (slot == SourceSlot::Mic).then(|| {
        let mut d = MicNoiseProcessor::new(
            mic_dsp_cfg.noise_suppression,
            mic_dsp_cfg.noise_suppression_strength,
            mic_dsp_cfg.noise_gate_db,
        );
        d.set_soft_limiter(mic_dsp_cfg.soft_limiter);
        d
    });

    let mut stream: Option<OpenedWasapiStream> = None;
    let mut proc_stream: Option<ProcessLoopbackStream> = None;
    let mut proc_pcm_buf: Vec<f32> = Vec::with_capacity(STEREO_BLOCK_SAMPLES * 4);
    let mut proc_exe_name: Option<String> = None;
    let mut last_pid_recheck = Instant::now();
    let mut last_open_attempt = Instant::now() - Duration::from_secs(10);
    let mut force_reopen = false;

    if !role.is_intentionally_idle() {
        last_open_attempt = Instant::now();
        if let SourceRole::GameLoopback {
            mode: AudioOutputMode::GameProcessOnly,
            target_pid,
        } = &role
        {
            if let Some(pid) = *target_pid {
                match ProcessLoopbackStream::open(pid) {
                    Ok(ps) => {
                        let exe = query_process_exe_name(pid);
                        info!("Opened GameProcessOnly loopback: {exe} (PID={pid})");
                        telemetry.set_active_device(format!("Process Loopback: {exe} (PID {pid})"));
                        proc_exe_name = (!exe.starts_with('<')).then_some(exe);
                        proc_stream = Some(ps);
                    }
                    Err(err) => {
                        debug!("Process loopback initial open deferred (PID={pid}): {err}");
                        telemetry.set_active_device(format!("Process Loopback deferred ({err})"));
                    }
                }
            } else {
                telemetry.set_active_device("Process Loopback (Waiting for Game PID)");
            }
        } else {
            match open_wasapi_stream(&role) {
                Ok(opened) => {
                    info!("Opened audio capture ({:?}): {}", slot, opened.description);
                    telemetry.set_active_device(opened.description.clone());
                    stream = Some(opened);
                }
                Err(err) => {
                    debug!("WASAPI initial open deferred for {:?}: {}", slot, err);
                    telemetry.set_active_device(format!("Unavailable ({err})"));
                }
            }
        }
    } else {
        telemetry.set_active_device(match &role {
            SourceRole::Microphone { .. } => "Microphone Disabled",
            SourceRole::SecondaryLoopback { .. } => "Secondary Disabled",
            SourceRole::GameLoopback { .. } => "Game Disabled",
        });
    }

    // Synchronize startup across active source threads before resetting queues and latching t0.
    let base_pts_us = if !role.is_intentionally_idle() {
        startup.mark_ready_and_wait(clock, STARTUP_BARRIER_TIMEOUT)
    } else {
        startup.wait_for_base_pts(clock, STARTUP_BARRIER_TIMEOUT)
    };

    if let Some(s) = stream.as_mut() {
        s.drain_stale_and_reset_resampler();
    }

    let mut block_idx: u64 = 0;
    let mut last_real_packet_at = Instant::now();
    let mut silent_since: Option<Instant> = None;
    let mut silent_blocks_emitted: u64 = 0;

    while !stop_flag.load(Ordering::Relaxed) {
        // 1. Dynamic device, PTT, and Mic RNNoise control updates.
        loop {
            match cmd_rx.try_recv() {
                Ok(SourceControl::Stop) => {
                    if let Some(s) = stream.take() {
                        let _ = s.audio_client.stop_stream();
                    }
                    return;
                }
                Ok(SourceControl::SetOutputMode(new_mode)) => {
                    if let SourceRole::GameLoopback { mode, .. } = &mut role {
                        if *mode != new_mode {
                            info!("Switching AudioOutputMode to {:?}", new_mode);
                            *mode = new_mode;
                            force_reopen = true;
                        }
                    }
                }
                Ok(SourceControl::SetTargetProcessId(new_pid)) => {
                    if let SourceRole::GameLoopback {
                        mode,
                        target_pid,
                    } = &mut role
                    {
                        if *target_pid != new_pid {
                            *target_pid = new_pid;
                            if *mode == AudioOutputMode::GameProcessOnly {
                                force_reopen = true;
                            }
                        }
                    }
                }
                Ok(SourceControl::SetMicDevice(new_dev)) => {
                    if let SourceRole::Microphone { device } = &mut role {
                        if *device != new_dev {
                            info!("Switching MicDeviceConfig to {:?}", new_dev);
                            *device = new_dev;
                            force_reopen = true;
                        }
                    }
                }
                Ok(SourceControl::SetMicActivationMode(new_mode)) => {
                    if slot == SourceSlot::Mic {
                        info!("Updating mic activation mode to {:?}", new_mode);
                        ptt_gate.set_mode(new_mode);
                    }
                }
                Ok(SourceControl::SetMicNoiseSuppression {
                    enabled,
                    strength,
                    noise_gate_db,
                }) => {
                    if let Some(d) = mic_denoiser.as_mut() {
                        d.update_config(enabled, strength, noise_gate_db);
                    }
                }
                Ok(SourceControl::SetMicSoftLimiter(enabled)) => {
                    if let Some(d) = mic_denoiser.as_mut() {
                        d.set_soft_limiter(enabled);
                    }
                }
                Ok(SourceControl::SetSecondaryDevice(new_dev)) => {
                    if let SourceRole::SecondaryLoopback { device_id } = &mut role {
                        if *device_id != new_dev {
                            info!("Switching secondary audio device to {:?}", new_dev);
                            *device_id = new_dev;
                            force_reopen = true;
                        }
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }

        // 2. Open / gracefully recover WASAPI stream when active.
        if role.is_intentionally_idle() {
            if let Some(old) = stream.take() {
                let _ = old.audio_client.stop_stream();
            }
            telemetry.set_active_device(match &role {
                SourceRole::Microphone { .. } => "Microphone Disabled",
                SourceRole::SecondaryLoopback { .. } => "Secondary Disabled",
                SourceRole::GameLoopback { .. } => "Game Disabled",
            });
            force_reopen = false;
            silent_since = None;
            silent_blocks_emitted = 0;
            thread::sleep(Duration::from_millis(20));
            let target_blocks =
                elapsed_48k_frames(base_pts_us, clock.now_us()) / (AAC_FRAME_SAMPLES as u64);
            if target_blocks > block_idx {
                block_idx = target_blocks;
            }
            continue;
        }

        let is_proc_mode = matches!(
            &role,
            SourceRole::GameLoopback {
                mode: AudioOutputMode::GameProcessOnly,
                ..
            }
        );

        if force_reopen
            || ((!is_proc_mode && stream.is_none() || is_proc_mode && proc_stream.is_none())
                && last_open_attempt.elapsed() >= WASAPI_RECONNECT_BACKOFF)
        {
            if let Some(old) = stream.take() {
                let _ = old.audio_client.stop_stream();
            }
            proc_stream = None;
            proc_pcm_buf.clear();
            proc_exe_name = None;
            force_reopen = false;
            last_open_attempt = Instant::now();

            if let SourceRole::GameLoopback {
                mode: AudioOutputMode::GameProcessOnly,
                target_pid,
            } = &role
            {
                if let Some(pid) = *target_pid {
                    match ProcessLoopbackStream::open(pid) {
                        Ok(ps) => {
                            let exe = query_process_exe_name(pid);
                            info!("Opened GameProcessOnly loopback: {exe} (PID={pid})");
                            telemetry
                                .set_active_device(format!("Process Loopback: {exe} (PID {pid})"));
                            proc_exe_name = (!exe.starts_with('<')).then_some(exe);
                            last_real_packet_at = Instant::now();
                            silent_since = None;
                            silent_blocks_emitted = 0;
                            let target_blocks = elapsed_48k_frames(base_pts_us, clock.now_us())
                                / (AAC_FRAME_SAMPLES as u64);
                            if target_blocks > block_idx {
                                block_idx = target_blocks;
                            }
                            proc_stream = Some(ps);
                        }
                        Err(err) => {
                            debug!("Process loopback open deferred (PID={pid}): {err}");
                            telemetry.set_active_device(format!("Process Loopback deferred ({err})"));
                        }
                    }
                } else {
                    telemetry.set_active_device("Process Loopback (Waiting for Game PID)");
                }
            } else {
                match open_wasapi_stream(&role) {
                    Ok(mut opened) => {
                        opened.drain_stale_and_reset_resampler();
                        info!("Opened audio capture ({:?}): {}", slot, opened.description);
                        telemetry.set_active_device(opened.description.clone());
                        last_real_packet_at = Instant::now();
                        silent_since = None;
                        silent_blocks_emitted = 0;
                        let target_blocks = elapsed_48k_frames(base_pts_us, clock.now_us())
                            / (AAC_FRAME_SAMPLES as u64);
                        if target_blocks > block_idx {
                            block_idx = target_blocks;
                        }
                        stream = Some(opened);
                    }
                    Err(err) => {
                        debug!("WASAPI open deferred for {:?}: {}", slot, err);
                        telemetry.set_active_device(format!("Unavailable ({err})"));
                    }
                }
            }
        }

        // 3. Drain all buffered WASAPI capture packets (single get_next_packet_size per packet).
        let mut device_disconnected = false;
        if let Some(ps) = proc_stream.as_mut() {
            match ps.wait_and_read(20, &mut proc_pcm_buf) {
                Ok((total_frames, real_frames)) => {
                    if total_frames > 0 {
                        let was_in_silence = silent_since.is_some();
                        last_real_packet_at = Instant::now();
                        silent_since = None;
                        silent_blocks_emitted = 0;
                        if was_in_silence && proc_pcm_buf.len() < STEREO_BLOCK_SAMPLES {
                            let target_wall_blocks =
                                elapsed_48k_frames(base_pts_us, clock.now_us())
                                    / (AAC_FRAME_SAMPLES as u64);
                            if target_wall_blocks > block_idx.saturating_add(2) {
                                block_idx = target_wall_blocks.saturating_sub(1);
                            }
                        }
                        telemetry.add_real_frames(total_frames);
                    }
                    while proc_pcm_buf.len() >= STEREO_BLOCK_SAMPLES {
                        let mut f32_block = [0.0f32; STEREO_BLOCK_SAMPLES];
                        f32_block.copy_from_slice(&proc_pcm_buf[..STEREO_BLOCK_SAMPLES]);
                        proc_pcm_buf.drain(..STEREO_BLOCK_SAMPLES);
                        telemetry.observe_peak_f32(&f32_block);
                        let this_idx = block_idx;
                        block_idx = block_idx.saturating_add(1);
                        let _ = pcm_tx.try_send(CapturedBlock {
                            slot,
                            block_idx: this_idx,
                            samples: f32_block,
                        });
                    }
                    if last_pid_recheck.elapsed() >= Duration::from_millis(400) {
                        last_pid_recheck = Instant::now();
                        if let Some(exe) = proc_exe_name.as_ref() {
                            if let Ok(Some(best)) =
                                find_best_pid_for_processes(std::slice::from_ref(exe))
                            {
                                if best.pid != ps.pid()
                                    && (best.peak > 0.0001
                                        && real_frames == 0
                                        || !is_pid_running(ps.pid()))
                                {
                                    info!(
                                        "GameProcessOnly re-attaching {} PID {} -> {}",
                                        exe,
                                        ps.pid(),
                                        best.pid
                                    );
                                    if let Ok(new_ps) = ProcessLoopbackStream::open(best.pid) {
                                        proc_stream = Some(new_ps);
                                        telemetry.set_active_device(format!(
                                            "Process Loopback: {} (PID {})",
                                            exe, best.pid
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
                Err(err) => {
                    warn!("GameProcessOnly loopback error: {err}");
                    proc_stream = None;
                    device_disconnected = true;
                }
            }
        } else if let Some(s) = stream.as_mut() {
            if let Some(ev) = s.event_handle.as_ref() {
                let _ = ev.wait_for_event(25);
            } else {
                thread::sleep(Duration::from_millis(2));
            }

            loop {
                match s.capture_client.get_next_packet_size() {
                    Ok(Some(0)) | Ok(None) => break,
                    Ok(Some(avail_frames)) => {
                        let needed_bytes = avail_frames as usize * s.bytes_per_frame;
                        if s.scratch.len() < needed_bytes {
                            s.scratch.resize(needed_bytes, 0);
                        }

                        match s
                            .capture_client
                            .read_from_device(&mut s.scratch[..needed_bytes])
                        {
                            Ok((frames_read, buf_info)) => {
                                let is_first_packet = !s.first_live_packet_seen;
                                if frames_read > 0 || buf_info.flags.data_discontinuity {
                                    s.first_live_packet_seen = true;
                                }
                                if buf_info.flags.data_discontinuity {
                                    if is_first_packet {
                                        debug!(
                                            "Ignoring initial WASAPI startup data_discontinuity on {:?} (frames_read={})",
                                            slot, frames_read
                                        );
                                    } else {
                                        warn!(
                                            "WASAPI mid-stream data_discontinuity on {:?} at block_idx={} (frames_read={})",
                                            slot, block_idx, frames_read
                                        );
                                        telemetry.inc_discontinuities();
                                    }
                                }
                                if frames_read > 0 {
                                    let was_in_silence = silent_since.is_some();
                                    last_real_packet_at = Instant::now();
                                    silent_since = None;
                                    silent_blocks_emitted = 0;
                                    if was_in_silence && s.resampler.queued_input_frames() < AAC_FRAME_SAMPLES {
                                        let target_wall_blocks =
                                            elapsed_48k_frames(base_pts_us, clock.now_us())
                                                / (AAC_FRAME_SAMPLES as u64);
                                        if target_wall_blocks > block_idx.saturating_add(2) {
                                            block_idx = target_wall_blocks.saturating_sub(1);
                                        }
                                    }
                                    let read_bytes = frames_read as usize * s.bytes_per_frame;
                                    telemetry.add_real_frames(u64::from(frames_read));
                                    if buf_info.flags.silent {
                                        s.resampler.push_silence_input_frames(frames_read as usize);
                                    } else {
                                        s.resampler.push_raw_bytes(&s.scratch[..read_bytes]);
                                    }
                                }
                            }
                            Err(err) => {
                                warn!("WASAPI read error ({:?}): {err}", slot);
                                device_disconnected = true;
                                break;
                            }
                        }
                    }
                    Err(err) => {
                        warn!("WASAPI packet query error ({:?}): {err}", slot);
                        device_disconnected = true;
                        break;
                    }
                }
            }

            while let Ok(Some(mut f32_block)) = s.resampler.pop_48k_float_frame() {
                if slot == SourceSlot::Mic {
                    ptt_gate.apply_to_block(&mut f32_block);
                    if let Some(denoiser) = mic_denoiser.as_mut() {
                        denoiser.process_stereo_1024(&mut f32_block);
                    }
                }
                telemetry.observe_peak_f32(&f32_block);
                let this_idx = block_idx;
                block_idx = block_idx.saturating_add(1);
                let _ = pcm_tx.try_send(CapturedBlock {
                    slot,
                    block_idx: this_idx,
                    samples: f32_block,
                });
            }
        } else {
            thread::sleep(Duration::from_millis(10));
        }

        if device_disconnected {
            let desc = stream
                .take()
                .map(|s| s.description)
                .unwrap_or_else(|| format!("{slot:?}"));
            warn!(
                "WASAPI stream invalidated/disconnected ({slot:?}, last={desc}); scheduling reopen with fresh format"
            );
            let _ = warn_tx.try_send(AudioOutputEvent::RecoverableWarning(format!(
                "Audio device disconnected ({desc}), reconnecting"
            )));
            last_open_attempt = Instant::now() - WASAPI_RECONNECT_BACKOFF + Duration::from_millis(150);
        }

        // 4. Real-Instant silence injection only after >= 110 ms of genuine WASAPI inactivity.
        if (stream.is_none() && proc_stream.is_none())
            || last_real_packet_at.elapsed() >= IDLE_SILENCE_THRESHOLD
        {
            let pending_resampler_blocks = stream.as_ref().map_or(0, |s| {
                let q = s.resampler.queued_input_frames() as u64;
                let in_rate = u64::from(s.resampler.input_sample_rate().max(1));
                let q_48k = (q * u64::from(TARGET_SAMPLE_RATE_HZ)) / in_rate;
                q_48k / (AAC_FRAME_SAMPLES as u64)
            });

            let target_wall_blocks =
                elapsed_48k_frames(base_pts_us, clock.now_us()) / (AAC_FRAME_SAMPLES as u64);
            let idle_origin =
                *silent_since.get_or_insert_with(|| Instant::now() - Duration::from_millis(22));
            let target_silent_blocks = (idle_origin.elapsed().as_micros() / BLOCK_DURATION_US) as u64;

            if let Some(s) = stream.as_mut() {
                let mut steps = 0u32;
                while silent_blocks_emitted < target_silent_blocks
                    && block_idx.saturating_add(pending_resampler_blocks) < target_wall_blocks
                    && steps < 8
                {
                    steps += 1;
                    silent_blocks_emitted = silent_blocks_emitted.saturating_add(1);
                    let in_rate = u64::from(s.resampler.input_sample_rate().max(1));
                    let in_frames = ((u64::from(AAC_FRAME_SAMPLES as u32) * in_rate)
                        / u64::from(TARGET_SAMPLE_RATE_HZ))
                    .max(1) as usize;
                    s.resampler.push_silence_input_frames(in_frames);
                    telemetry.add_silent_block(u64::from(AAC_FRAME_SAMPLES as u32));

                    while let Ok(Some(mut f32_block)) = s.resampler.pop_48k_float_frame() {
                        if slot == SourceSlot::Mic {
                            ptt_gate.apply_to_block(&mut f32_block);
                            if let Some(denoiser) = mic_denoiser.as_mut() {
                                denoiser.process_stereo_1024(&mut f32_block);
                            }
                        }
                        let this_idx = block_idx;
                        block_idx = block_idx.saturating_add(1);
                        let _ = pcm_tx.try_send(CapturedBlock {
                            slot,
                            block_idx: this_idx,
                            samples: f32_block,
                        });
                    }
                }
            } else {
                let zero_block = [0.0f32; STEREO_BLOCK_SAMPLES];
                let mut steps = 0u32;
                while silent_blocks_emitted < target_silent_blocks
                    && block_idx < target_wall_blocks
                    && steps < 8
                {
                    steps += 1;
                    silent_blocks_emitted = silent_blocks_emitted.saturating_add(1);
                    let this_idx = block_idx;
                    block_idx = block_idx.saturating_add(1);
                    telemetry.add_silent_block(AAC_FRAME_SAMPLES as u64);
                    let _ = pcm_tx.try_send(CapturedBlock {
                        slot,
                        block_idx: this_idx,
                        samples: zero_block,
                    });
                }
            }
        }
    }
}

struct MixerConfigState {
    game_enabled: bool,
    mic_enabled: bool,
    secondary_enabled: bool,
    separate_tracks: bool,
    game_volume: f32,
    game_muted: bool,
    mic_volume: f32,
    mic_muted: bool,
    secondary_volume: f32,
    secondary_muted: bool,
}

struct AppSlotMixerState {
    slot_idx: u8,
    volume: f32,
    muted: bool,
    enabled: bool,
    queue: BTreeMap<u64, [f32; STEREO_BLOCK_SAMPLES]>,
    encoder: Option<MfAacEncoder>,
    telemetry: Arc<TrackTelemetryShared>,
    last_block_at: Option<Instant>,
}

#[allow(clippy::too_many_arguments)]
fn run_app_slot_capture_loop(
    slot_idx: u8,
    label: String,
    process_names: Vec<String>,
    initial_pid: u32,
    base_pts_atomic: Arc<AtomicI64>,
    clock: MonotonicClock,
    cmd_rx: Receiver<SourceControl>,
    pcm_tx: Sender<CapturedBlock>,
    warn_tx: Sender<AudioOutputEvent>,
    telemetry: Arc<TrackTelemetryShared>,
    running: Arc<AtomicBool>,
    stop_flag: Arc<AtomicBool>,
) {
    struct RunningGuard(Arc<AtomicBool>);
    impl Drop for RunningGuard {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Relaxed);
        }
    }
    let _running_guard = RunningGuard(running);
    let _thread_guard = AudioThreadGuard::enter_pro_audio(true);

    let mut proc_stream = match ProcessLoopbackStream::open(initial_pid) {
        Ok(ps) => {
            let exe = query_process_exe_name(initial_pid);
            info!("App slot '{label}' attached to {exe} (PID={initial_pid})");
            telemetry.set_active_device(format!("{label}: {exe} (PID {initial_pid})"));
            Some(ps)
        }
        Err(err) => {
            let _ = warn_tx.try_send(AudioOutputEvent::RecoverableWarning(format!(
                "App slot '{label}' initial open failed (PID={initial_pid}): {err}"
            )));
            None
        }
    };

    let mut pcm_buf: Vec<f32> = Vec::with_capacity(STEREO_BLOCK_SAMPLES * 4);
    let base_pts_us = base_pts_atomic.load(Ordering::Relaxed);
    let mut block_idx =
        elapsed_48k_frames(base_pts_us, clock.now_us()) / (AAC_FRAME_SAMPLES as u64);
    let mut last_recheck = Instant::now();
    let mut last_nonzero = Instant::now();
    let mut last_open_attempt = Instant::now();

    while !stop_flag.load(Ordering::Relaxed) {
        match cmd_rx.try_recv() {
            Ok(SourceControl::Stop) | Err(TryRecvError::Disconnected) => return,
            Ok(_) | Err(TryRecvError::Empty) => {}
        }

        if let Some(ps) = proc_stream.as_mut() {
            match ps.wait_and_read(20, &mut pcm_buf) {
                Ok((total_frames, real_frames)) => {
                    if total_frames > 0 {
                        telemetry.add_real_frames(total_frames);
                    }
                    if real_frames > 0 {
                        last_nonzero = Instant::now();
                    }
                    let target_wall = elapsed_48k_frames(
                        base_pts_atomic.load(Ordering::Relaxed),
                        clock.now_us(),
                    ) / (AAC_FRAME_SAMPLES as u64);
                    if target_wall > block_idx.saturating_add(4) {
                        block_idx = target_wall.saturating_sub(1);
                    }
                    while pcm_buf.len() >= STEREO_BLOCK_SAMPLES {
                        let mut f32_block = [0.0f32; STEREO_BLOCK_SAMPLES];
                        f32_block.copy_from_slice(&pcm_buf[..STEREO_BLOCK_SAMPLES]);
                        pcm_buf.drain(..STEREO_BLOCK_SAMPLES);
                        telemetry.observe_peak_f32(&f32_block);
                        let this_idx = block_idx;
                        block_idx = block_idx.saturating_add(1);
                        let _ = pcm_tx.try_send(CapturedBlock {
                            slot: SourceSlot::App(slot_idx),
                            block_idx: this_idx,
                            samples: f32_block,
                        });
                    }
                }
                Err(err) => {
                    warn!("App slot '{label}' loopback error: {err}");
                    proc_stream = None;
                    last_open_attempt = Instant::now() - Duration::from_millis(500);
                }
            }
        } else {
            thread::sleep(Duration::from_millis(20));
        }

        if last_recheck.elapsed() >= Duration::from_millis(400) {
            last_recheck = Instant::now();
            match find_best_pid_for_processes(&process_names) {
                Ok(Some(best)) => {
                    let current_pid = proc_stream.as_ref().map(ProcessLoopbackStream::pid);
                    let current_idle = last_nonzero.elapsed() >= Duration::from_millis(400);
                    let need_switch = match current_pid {
                        None => last_open_attempt.elapsed() >= Duration::from_millis(500),
                        Some(pid) => {
                            best.pid != pid
                                && (best.peak > 0.0001 && current_idle || !is_pid_running(pid))
                        }
                    };
                    if need_switch {
                        last_open_attempt = Instant::now();
                        if let Ok(new_ps) = ProcessLoopbackStream::open(best.pid) {
                            let exe = query_process_exe_name(best.pid);
                            info!("App slot '{label}' attached to {exe} (PID={})", best.pid);
                            telemetry.set_active_device(format!("{label}: {exe} (PID {})", best.pid));
                            pcm_buf.clear();
                            proc_stream = Some(new_ps);
                        }
                    }
                }
                Ok(None) => {
                    let still_alive = proc_stream
                        .as_ref()
                        .is_some_and(|ps| is_pid_running(ps.pid()));
                    if !still_alive {
                        info!("App slot '{label}' process exited; stopping capture thread (idle 0%)");
                        telemetry.set_active_device(format!("{label} (Idle)"));
                        return;
                    }
                }
                Err(_) => {}
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_mixer_loop(
    settings: AudioSettings,
    startup: Arc<StartupBarrier>,
    base_pts_atomic: Arc<AtomicI64>,
    clock: MonotonicClock,
    pcm_rx: Receiver<CapturedBlock>,
    cmd_rx: Receiver<MixerControl>,
    out_tx: Sender<AudioOutputEvent>,
    mixed_telemetry: Arc<TrackTelemetryShared>,
    game_telemetry: Arc<TrackTelemetryShared>,
    mic_telemetry: Arc<TrackTelemetryShared>,
    sec_telemetry: Arc<TrackTelemetryShared>,
    has_secondary_track: Arc<AtomicBool>,
    app_slots_init: Vec<(u8, AppAudioSlotConfig, Arc<TrackTelemetryShared>)>,
    stop_flag: Arc<AtomicBool>,
) {
    let _thread_guard = AudioThreadGuard::enter_pro_audio(true);

    let mut cfg = MixerConfigState {
        game_enabled: settings.game_enabled,
        mic_enabled: settings.mic_enabled && settings.mic_device != MicDeviceConfig::None,
        secondary_enabled: settings.secondary_output_device.is_some(),
        separate_tracks: settings.separate_tracks,
        game_volume: settings.game_volume,
        game_muted: settings.game_muted,
        mic_volume: settings.mic_volume,
        mic_muted: settings.mic_muted,
        secondary_volume: settings.secondary_volume,
        secondary_muted: settings.secondary_muted,
    };

    let mixed_kbps = target_aac_bitrate_kbps(TrackKind::AudioMixed, settings.aac_bitrate_kbps);
    let game_kbps = target_aac_bitrate_kbps(TrackKind::AudioGame, settings.aac_bitrate_kbps);
    let mic_kbps = target_aac_bitrate_kbps(TrackKind::AudioMic, settings.aac_bitrate_kbps);
    let sec_kbps = target_aac_bitrate_kbps(TrackKind::AudioSecondary, settings.aac_bitrate_kbps);
    info!(
        "Starting AAC encoders (configured={} kbps -> Mixed={} kbps, Game={} kbps, Mic={} kbps, Secondary={} kbps)",
        settings.aac_bitrate_kbps, mixed_kbps, game_kbps, mic_kbps, sec_kbps
    );

    let mut enc_mixed = match MfAacEncoder::new(TrackKind::AudioMixed, mixed_kbps) {
        Ok(enc) => enc,
        Err(err) => {
            let _ = out_tx.try_send(AudioOutputEvent::RecoverableWarning(format!(
                "AAC encoder init failed for AudioMixed: {err}"
            )));
            return;
        }
    };
    let _ = out_tx.send(AudioOutputEvent::StreamHeader(
        enc_mixed.stream_header().clone(),
    ));

    let mut enc_game = if cfg.separate_tracks && cfg.game_enabled {
        match MfAacEncoder::new(TrackKind::AudioGame, game_kbps) {
            Ok(enc) => {
                let _ = out_tx.send(AudioOutputEvent::StreamHeader(enc.stream_header().clone()));
                Some(enc)
            }
            Err(err) => {
                let _ = out_tx.try_send(AudioOutputEvent::RecoverableWarning(format!(
                    "AAC encoder init failed for AudioGame: {err}"
                )));
                None
            }
        }
    } else {
        None
    };

    let mut enc_mic = if cfg.separate_tracks && cfg.mic_enabled {
        match MfAacEncoder::new(TrackKind::AudioMic, mic_kbps) {
            Ok(enc) => {
                let _ = out_tx.send(AudioOutputEvent::StreamHeader(enc.stream_header().clone()));
                Some(enc)
            }
            Err(err) => {
                let _ = out_tx.try_send(AudioOutputEvent::RecoverableWarning(format!(
                    "AAC encoder init failed for AudioMic: {err}"
                )));
                None
            }
        }
    } else {
        None
    };

    let mut enc_sec = if cfg.separate_tracks && cfg.secondary_enabled {
        match MfAacEncoder::new(TrackKind::AudioSecondary, sec_kbps) {
            Ok(enc) => {
                let _ = out_tx.send(AudioOutputEvent::StreamHeader(enc.stream_header().clone()));
                Some(enc)
            }
            Err(err) => {
                let _ = out_tx.try_send(AudioOutputEvent::RecoverableWarning(format!(
                    "AAC encoder init failed for AudioSecondary: {err}"
                )));
                None
            }
        }
    } else {
        None
    };

    let mut app_mix: Vec<AppSlotMixerState> = app_slots_init
        .into_iter()
        .map(|(slot_idx, slot_cfg, tel)| {
            let encoder = if cfg.separate_tracks && slot_cfg.enabled {
                let kbps =
                    target_aac_bitrate_kbps(TrackKind::AudioApp(slot_idx), settings.aac_bitrate_kbps);
                match MfAacEncoder::new(TrackKind::AudioApp(slot_idx), kbps) {
                    Ok(enc) => {
                        let _ = out_tx.send(AudioOutputEvent::StreamHeader(
                            enc.stream_header().clone(),
                        ));
                        Some(enc)
                    }
                    Err(err) => {
                        let _ = out_tx.try_send(AudioOutputEvent::RecoverableWarning(format!(
                            "AAC encoder init failed for AudioApp({slot_idx}): {err}"
                        )));
                        None
                    }
                }
            } else {
                None
            };
            AppSlotMixerState {
                slot_idx,
                volume: slot_cfg.volume.clamp(0.0, 5.0),
                muted: slot_cfg.muted,
                enabled: slot_cfg.enabled,
                queue: BTreeMap::new(),
                encoder,
                telemetry: tel,
                last_block_at: None,
            }
        })
        .collect();

    // Synchronize timeline epoch with all active WASAPI threads before draining block #0.
    let base_pts_us = startup.wait_for_base_pts(clock, STARTUP_BARRIER_TIMEOUT);
    base_pts_atomic.store(base_pts_us, Ordering::Relaxed);

    let mut game_queue: BTreeMap<u64, [f32; STEREO_BLOCK_SAMPLES]> = BTreeMap::new();
    let mut mic_queue: BTreeMap<u64, [f32; STEREO_BLOCK_SAMPLES]> = BTreeMap::new();
    let mut sec_queue: BTreeMap<u64, [f32; STEREO_BLOCK_SAMPLES]> = BTreeMap::new();
    let mut next_mix_idx: u64 = 0;

    let zero_block = [0.0f32; STEREO_BLOCK_SAMPLES];

    while !stop_flag.load(Ordering::Relaxed) {
        // 1. Handle live mixer control updates.
        loop {
            match cmd_rx.try_recv() {
                Ok(MixerControl::Stop) => {
                    flush_all_encoders(
                        &mut enc_mixed,
                        &mut enc_game,
                        &mut enc_mic,
                        &mut enc_sec,
                        &mut app_mix,
                        base_pts_us,
                        next_mix_idx,
                        &mixed_telemetry,
                        &game_telemetry,
                        &mic_telemetry,
                        &sec_telemetry,
                        &out_tx,
                    );
                    log_final_audio_diagnostics(&game_telemetry, &mic_telemetry, &sec_telemetry);
                    return;
                }
                Ok(MixerControl::SetVolumes {
                    game_volume,
                    mic_volume,
                    secondary_volume,
                }) => {
                    cfg.game_volume = game_volume.clamp(0.0, 5.0);
                    cfg.mic_volume = mic_volume.clamp(0.0, 5.0);
                    if let Some(sec_vol) = secondary_volume {
                        cfg.secondary_volume = sec_vol.clamp(0.0, 5.0);
                    }
                }
                Ok(MixerControl::SetGameMuted(muted)) => cfg.game_muted = muted,
                Ok(MixerControl::SetMicMuted(muted)) => cfg.mic_muted = muted,
                Ok(MixerControl::SetSecondaryMuted(muted)) => cfg.secondary_muted = muted,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    log_final_audio_diagnostics(&game_telemetry, &mic_telemetry, &sec_telemetry);
                    return;
                }
            }
        }

        // 2. Receive captured PCM blocks into per-source block maps.
        match pcm_rx.recv_timeout(Duration::from_millis(5)) {
            Ok(blk) => {
                insert_staged_block(
                    blk,
                    next_mix_idx,
                    &mut game_queue,
                    &mut mic_queue,
                    &mut sec_queue,
                    &mut app_mix,
                );
                while let Ok(more) = pcm_rx.try_recv() {
                    insert_staged_block(
                        more,
                        next_mix_idx,
                        &mut game_queue,
                        &mut mic_queue,
                        &mut sec_queue,
                        &mut app_mix,
                    );
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }

        // Dynamically promote secondary encoder header if secondary loopback starts arriving later.
        if !sec_queue.is_empty() && !cfg.secondary_enabled {
            cfg.secondary_enabled = true;
            has_secondary_track.store(true, Ordering::Relaxed);
            if cfg.separate_tracks && enc_sec.is_none() {
                if let Ok(enc) = MfAacEncoder::new(TrackKind::AudioSecondary, sec_kbps) {
                    let _ = out_tx.send(AudioOutputEvent::StreamHeader(enc.stream_header().clone()));
                    enc_sec = Some(enc);
                }
            }
        }

        // 3. Emit complete aligned slots, or emit past 120 ms deadline while recording underruns.
        loop {
            let slot_due_us = sample_index_to_pts_us(
                base_pts_us,
                (next_mix_idx + 1) * (AAC_FRAME_SAMPLES as u64),
            );
            let past_deadline =
                clock.now_us() >= slot_due_us.saturating_add(MIXER_SLOT_LATE_DEADLINE_US);

            let game_ready = !cfg.game_enabled || game_queue.contains_key(&next_mix_idx);
            let mic_ready = !cfg.mic_enabled || mic_queue.contains_key(&next_mix_idx);
            let sec_ready = !cfg.secondary_enabled || sec_queue.contains_key(&next_mix_idx);
            let apps_ready = app_mix.iter().all(|a| {
                let recently_active = a
                    .last_block_at
                    .is_some_and(|t| t.elapsed() < IDLE_SILENCE_THRESHOLD);
                !a.enabled || !recently_active || a.queue.contains_key(&next_mix_idx)
            });
            let all_ready = game_ready && mic_ready && sec_ready && apps_ready;

            // Hold up to `MIXER_SLOT_LATE_DEADLINE_US` (120 ms, ~5 blocks) / `MIXER_JITTER_BUFFER_BLOCKS`
            // so a slightly jittery Game stream (e.g. 8ch 7.1 wireless headset) aligns with Mic
            // instead of emitting premature silence; past deadline, substitute silence and advance.
            let any_queue_overflowing = game_queue.len() > MIXER_JITTER_BUFFER_BLOCKS * 2
                || mic_queue.len() > MIXER_JITTER_BUFFER_BLOCKS * 2
                || sec_queue.len() > MIXER_JITTER_BUFFER_BLOCKS * 2
                || app_mix
                    .iter()
                    .any(|a| a.queue.len() > MIXER_JITTER_BUFFER_BLOCKS * 2);
            if !all_ready && !past_deadline && !any_queue_overflowing {
                break;
            }

            if !all_ready {
                if !game_ready {
                    game_telemetry.inc_mixer_underruns();
                }
                if !mic_ready {
                    mic_telemetry.inc_mixer_underruns();
                }
                if !sec_ready {
                    sec_telemetry.inc_mixer_underruns();
                }
            }

            let pts_us =
                sample_index_to_pts_us(base_pts_us, next_mix_idx * (AAC_FRAME_SAMPLES as u64));

            let game_block = game_queue.remove(&next_mix_idx).unwrap_or(zero_block);
            let mic_block = mic_queue.remove(&next_mix_idx).unwrap_or(zero_block);
            let sec_block = sec_queue.remove(&next_mix_idx).unwrap_or(zero_block);

            // Encode clean stems (mic denoising already performed in the Mic thread).
            if let Some(ref mut enc) = enc_game {
                encode_and_emit(enc, &game_block, pts_us, &game_telemetry, &out_tx);
            }
            if let Some(ref mut enc) = enc_mic {
                encode_and_emit(enc, &mic_block, pts_us, &mic_telemetry, &out_tx);
            }
            if let Some(ref mut enc) = enc_sec {
                encode_and_emit(enc, &sec_block, pts_us, &sec_telemetry, &out_tx);
            }

            let mut app_blocks: Vec<([f32; STEREO_BLOCK_SAMPLES], f32, bool)> =
                Vec::with_capacity(app_mix.len());
            for app in &mut app_mix {
                let blk = app.queue.remove(&next_mix_idx).unwrap_or(zero_block);
                if let Some(ref mut enc) = app.encoder {
                    encode_and_emit(enc, &blk, pts_us, &app.telemetry, &out_tx);
                }
                if app.enabled {
                    app_blocks.push((blk, app.volume, app.muted));
                }
            }

            let mut mix_inputs: Vec<(&[f32; STEREO_BLOCK_SAMPLES], f32, bool)> =
                Vec::with_capacity(3 + app_blocks.len());
            if cfg.game_enabled {
                mix_inputs.push((&game_block, cfg.game_volume, cfg.game_muted));
            }
            if cfg.mic_enabled {
                mix_inputs.push((&mic_block, cfg.mic_volume, cfg.mic_muted));
            }
            if cfg.secondary_enabled {
                mix_inputs.push((&sec_block, cfg.secondary_volume, cfg.secondary_muted));
            }
            for (blk, vol, muted) in &app_blocks {
                mix_inputs.push((blk, *vol, *muted));
            }
            let mixed_block = mix_stereo_blocks_soft_clip(&mix_inputs);
            encode_and_emit(&mut enc_mixed, &mixed_block, pts_us, &mixed_telemetry, &out_tx);

            next_mix_idx = next_mix_idx.saturating_add(1);
        }
    }

    flush_all_encoders(
        &mut enc_mixed,
        &mut enc_game,
        &mut enc_mic,
        &mut enc_sec,
        &mut app_mix,
        base_pts_us,
        next_mix_idx,
        &mixed_telemetry,
        &game_telemetry,
        &mic_telemetry,
        &sec_telemetry,
        &out_tx,
    );
    log_final_audio_diagnostics(&game_telemetry, &mic_telemetry, &sec_telemetry);
}

#[allow(clippy::too_many_arguments)]
fn flush_all_encoders(
    enc_mixed: &mut MfAacEncoder,
    enc_game: &mut Option<MfAacEncoder>,
    enc_mic: &mut Option<MfAacEncoder>,
    enc_sec: &mut Option<MfAacEncoder>,
    app_mix: &mut [AppSlotMixerState],
    base_pts_us: i64,
    next_mix_idx: u64,
    mixed_tel: &Arc<TrackTelemetryShared>,
    game_tel: &Arc<TrackTelemetryShared>,
    mic_tel: &Arc<TrackTelemetryShared>,
    sec_tel: &Arc<TrackTelemetryShared>,
    out_tx: &Sender<AudioOutputEvent>,
) {
    flush_encoder(enc_mixed, base_pts_us, next_mix_idx, mixed_tel, out_tx);
    if let Some(ref mut enc) = enc_game {
        flush_encoder(enc, base_pts_us, next_mix_idx, game_tel, out_tx);
    }
    if let Some(ref mut enc) = enc_mic {
        flush_encoder(enc, base_pts_us, next_mix_idx, mic_tel, out_tx);
    }
    if let Some(ref mut enc) = enc_sec {
        flush_encoder(enc, base_pts_us, next_mix_idx, sec_tel, out_tx);
    }
    for app in app_mix {
        if let Some(ref mut enc) = app.encoder {
            flush_encoder(enc, base_pts_us, next_mix_idx, &app.telemetry, out_tx);
        }
    }
}

fn log_final_audio_diagnostics(
    game_tel: &Arc<TrackTelemetryShared>,
    mic_tel: &Arc<TrackTelemetryShared>,
    sec_tel: &Arc<TrackTelemetryShared>,
) {
    let g = game_tel.snapshot();
    let m = mic_tel.snapshot();
    let s = sec_tel.snapshot();
    info!(
        "Audio pipeline stats -> Game(discontinuities={}, silent_injected_blocks={}, mixer_underruns={}) | Mic(discontinuities={}, silent_injected_blocks={}, mixer_underruns={}) | Secondary(discontinuities={}, mixer_underruns={})",
        g.discontinuities,
        g.silent_injected_blocks,
        g.mixer_underruns,
        m.discontinuities,
        m.silent_injected_blocks,
        m.mixer_underruns,
        s.discontinuities,
        s.mixer_underruns,
    );
}

fn insert_staged_block(
    blk: CapturedBlock,
    min_idx: u64,
    game_queue: &mut BTreeMap<u64, [f32; STEREO_BLOCK_SAMPLES]>,
    mic_queue: &mut BTreeMap<u64, [f32; STEREO_BLOCK_SAMPLES]>,
    sec_queue: &mut BTreeMap<u64, [f32; STEREO_BLOCK_SAMPLES]>,
    app_mix: &mut [AppSlotMixerState],
) {
    let map = match blk.slot {
        SourceSlot::Game => game_queue,
        SourceSlot::Mic => mic_queue,
        SourceSlot::Secondary => sec_queue,
        SourceSlot::App(slot_idx) => {
            let Some(app) = app_mix.iter_mut().find(|a| a.slot_idx == slot_idx) else {
                return;
            };
            app.last_block_at = Some(Instant::now());
            &mut app.queue
        }
    };
    let target_idx = if blk.block_idx < min_idx {
        // Never discard slightly-late blocks (e.g. after 110ms silence pause or 10-40ms USB/7.1 jitter):
        // snap forward into the next unoccupied mixer slot >= `min_idx`.
        let Some(free_idx) = (min_idx..min_idx + (MAX_STAGED_BLOCKS_PER_SOURCE as u64))
            .find(|idx| !map.contains_key(idx))
        else {
            return;
        };
        free_idx
    } else {
        blk.block_idx
    };
    map.insert(target_idx, blk.samples);
    while map.len() > MAX_STAGED_BLOCKS_PER_SOURCE {
        let Some(&oldest_key) = map.keys().next() else {
            break;
        };
        map.remove(&oldest_key);
    }
}

fn encode_and_emit(
    encoder: &mut MfAacEncoder,
    samples_f32: &[f32; STEREO_BLOCK_SAMPLES],
    pts_us: i64,
    telemetry: &Arc<TrackTelemetryShared>,
    out_tx: &Sender<AudioOutputEvent>,
) {
    telemetry.observe_peak_f32(samples_f32);
    let pcm_i16 = float_frame_to_i16(samples_f32);
    if let Ok(pkts) = encoder.encode_pcm_block(&pcm_i16, pts_us) {
        for p in pkts {
            telemetry.inc_encoded_packets();
            let _ = out_tx.try_send(AudioOutputEvent::Packet(p));
        }
    }
}

fn flush_encoder(
    encoder: &mut MfAacEncoder,
    base_pts_us: i64,
    next_mix_idx: u64,
    telemetry: &Arc<TrackTelemetryShared>,
    out_tx: &Sender<AudioOutputEvent>,
) {
    let tail_pts = sample_index_to_pts_us(base_pts_us, next_mix_idx * (AAC_FRAME_SAMPLES as u64));
    if let Ok(pkts) = encoder.flush(tail_pts) {
        for p in pkts {
            telemetry.inc_encoded_packets();
            let _ = out_tx.try_send(AudioOutputEvent::Packet(p));
        }
    }
}

impl OpenedWasapiStream {
    fn open_with_client(
        audio_client: AudioClient,
        wavefmt: WaveFormat,
        use_events: bool,
        description: String,
    ) -> Result<Self, AudioError> {
        Self::open_with_client_ex(audio_client, wavefmt, use_events, true, description)
    }

    fn open_with_client_ex(
        mut audio_client: AudioClient,
        wavefmt: WaveFormat,
        use_events: bool,
        autoconvert: bool,
        description: String,
    ) -> Result<Self, AudioError> {
        let stream_mode = if use_events {
            StreamMode::EventsShared {
                autoconvert,
                buffer_duration_hns: WASAPI_BUFFER_DURATION_HNS,
            }
        } else {
            StreamMode::PollingShared {
                autoconvert,
                buffer_duration_hns: WASAPI_BUFFER_DURATION_HNS,
            }
        };

        audio_client
            .initialize_client(&wavefmt, Direction::Capture, stream_mode, None)
            .map_err(|e| AudioError::WasapiInitFailed(format!("initialize_client failed: {e}")))?;

        let event_handle = if use_events {
            Some(audio_client.set_get_eventhandle().map_err(|e| {
                AudioError::WasapiInitFailed(format!("set_get_eventhandle failed: {e}"))
            })?)
        } else {
            None
        };

        let capture_client = audio_client.get_audiocaptureclient().map_err(|e| {
            AudioError::WasapiInitFailed(format!("get_audiocaptureclient failed: {e}"))
        })?;
        let buffer_frames = audio_client.get_buffer_size().unwrap_or(4096) as usize;

        let in_rate = wavefmt.get_samplespersec();
        let in_channels = wavefmt.get_nchannels();
        let sample_type = wavefmt
            .get_subformat()
            .map_err(|e| AudioError::WasapiInitFailed(format!("Unsupported sample subformat: {e}")))?;
        let bits_per_sample = wavefmt.get_bitspersample();
        let valid_bits = wavefmt.get_validbitspersample();
        let block_align = wavefmt.get_blockalign() as usize;

        let resampler = Stereo48kResampler::new(
            in_rate,
            in_channels,
            sample_type,
            bits_per_sample,
            valid_bits,
            block_align,
            1.0,
            false,
        )?;

        audio_client
            .start_stream()
            .map_err(|e| AudioError::WasapiInitFailed(format!("start_stream failed: {e}")))?;

        let scratch_len = buffer_frames.max(4096) * block_align.max(4);
        let mut scratch = vec![0u8; scratch_len];

        // Drain stale initial WASAPI loopback/capture frames over 2 quick cycles right after start.
        for cycle in 0..2 {
            let _ = drain_wasapi_packets(&capture_client, &mut scratch, block_align);
            if cycle == 0 {
                thread::sleep(Duration::from_millis(12));
            }
        }

        Ok(Self {
            audio_client,
            capture_client,
            event_handle,
            resampler,
            bytes_per_frame: block_align,
            description,
            scratch,
            first_live_packet_seen: false,
        })
    }
}

fn open_wasapi_stream(role: &SourceRole) -> Result<OpenedWasapiStream, AudioError> {
    let enumerator = DeviceEnumerator::new().map_err(|e| {
        AudioError::WasapiInitFailed(format!("DeviceEnumerator::new failed: {e}"))
    })?;

    match role {
        SourceRole::GameLoopback { mode, .. } => {
            // Always bind strictly to the user-chosen endpoint or Windows default Render endpoint
            // (never hop unpredictably to virtual audio endpoints like Voicemod/Voicemeeter in silence).
            let device = match mode {
                AudioOutputMode::SpecificDevice(id) => enumerator.get_device(id).or_else(|_| {
                    enumerator.get_default_device_for_role(Direction::Render, Role::Multimedia)
                }),
                AudioOutputMode::DefaultDevice => enumerator
                    .get_default_device_for_role(Direction::Render, Role::Multimedia)
                    .or_else(|_| enumerator.get_default_device(Direction::Render)),
                AudioOutputMode::GameProcessOnly => {
                    return Err(AudioError::WasapiInitFailed(
                        "GameProcessOnly uses ProcessLoopbackStream".to_string(),
                    ));
                }
            }
            .map_err(|e| AudioError::DeviceDisconnected {
                device: format!("Render device unavailable: {e}"),
            })?;

            let friendly = device
                .get_friendlyname()
                .unwrap_or_else(|_| "Default Render Device".to_string());
            let client = device
                .get_iaudioclient()
                .map_err(|e| AudioError::WasapiInitFailed(format!("get_iaudioclient failed: {e}")))?;
            let mix_fmt = client
                .get_mixformat()
                .map_err(|e| AudioError::WasapiInitFailed(format!("get_mixformat failed: {e}")))?;

            let fmt_desc = format!(
                "Loopback: {} ({} Hz, {} ch, {} bit)",
                friendly,
                mix_fmt.get_samplespersec(),
                mix_fmt.get_nchannels(),
                mix_fmt.get_bitspersample()
            );

            OpenedWasapiStream::open_with_client(client, mix_fmt, false, fmt_desc)
        }
        SourceRole::Microphone { device: mic_cfg } => {
            let device = match mic_cfg {
                MicDeviceConfig::None => {
                    return Err(AudioError::DeviceDisconnected {
                        device: "Microphone disabled".to_string(),
                    })
                }
                MicDeviceConfig::SpecificDevice(id) => enumerator
                    .get_device(id)
                    .or_else(|_| enumerator.get_default_device(Direction::Capture)),
                MicDeviceConfig::Default => enumerator.get_default_device(Direction::Capture),
            }
            .map_err(|e| AudioError::DeviceDisconnected {
                device: format!("Microphone unavailable: {e}"),
            })?;

            let friendly = device
                .get_friendlyname()
                .unwrap_or_else(|_| "Default Microphone".to_string());
            let client = device
                .get_iaudioclient()
                .map_err(|e| AudioError::WasapiInitFailed(format!("get_iaudioclient failed: {e}")))?;
            let mix_fmt = client
                .get_mixformat()
                .map_err(|e| AudioError::WasapiInitFailed(format!("get_mixformat failed: {e}")))?;

            let fmt_desc = format!(
                "Mic: {} ({} Hz, {} ch, {} bit)",
                friendly,
                mix_fmt.get_samplespersec(),
                mix_fmt.get_nchannels(),
                mix_fmt.get_bitspersample()
            );

            OpenedWasapiStream::open_with_client(client, mix_fmt, false, fmt_desc)
        }
        SourceRole::SecondaryLoopback { device_id } => {
            let Some(id) = device_id else {
                return Err(AudioError::DeviceDisconnected {
                    device: "Secondary audio device not configured".to_string(),
                });
            };
            let device = enumerator.get_device(id).map_err(|e| AudioError::DeviceDisconnected {
                device: format!("Secondary audio device unavailable: {e}"),
            })?;
            let friendly = device
                .get_friendlyname()
                .unwrap_or_else(|_| "Secondary Output Device".to_string());
            let client = device
                .get_iaudioclient()
                .map_err(|e| AudioError::WasapiInitFailed(format!("get_iaudioclient failed: {e}")))?;
            let mix_fmt = client
                .get_mixformat()
                .map_err(|e| AudioError::WasapiInitFailed(format!("get_mixformat failed: {e}")))?;

            let fmt_desc = format!(
                "Secondary: {} ({} Hz, {} ch, {} bit)",
                friendly,
                mix_fmt.get_samplespersec(),
                mix_fmt.get_nchannels(),
                mix_fmt.get_bitspersample()
            );

            OpenedWasapiStream::open_with_client(client, mix_fmt, false, fmt_desc)
        }
    }
}

struct PttGateState {
    mode: MicActivationMode,
    last_key_down_at: Option<Instant>,
    current_gain: f32,
}

impl PttGateState {
    fn new(mode: MicActivationMode) -> Self {
        let initial_gain = match mode {
            MicActivationMode::PushToTalk { .. } => 0.0,
            MicActivationMode::AlwaysOn | MicActivationMode::PushToMute { .. } => 1.0,
        };
        Self {
            mode,
            last_key_down_at: None,
            current_gain: initial_gain,
        }
    }

    fn set_mode(&mut self, mode: MicActivationMode) {
        self.mode = mode;
        self.last_key_down_at = None;
    }

    fn is_open_now(&mut self) -> bool {
        match &self.mode {
            MicActivationMode::AlwaysOn => true,
            MicActivationMode::PushToTalk {
                key,
                release_delay_ms,
            } => {
                if is_ptt_key_down(key) {
                    self.last_key_down_at = Some(Instant::now());
                    true
                } else {
                    self.last_key_down_at.is_some_and(|t| {
                        t.elapsed() < Duration::from_millis(u64::from(*release_delay_ms))
                    })
                }
            }
            MicActivationMode::PushToMute {
                key,
                release_delay_ms,
            } => {
                if is_ptt_key_down(key) {
                    self.last_key_down_at = Some(Instant::now());
                    false
                } else {
                    !self.last_key_down_at.is_some_and(|t| {
                        t.elapsed() < Duration::from_millis(u64::from(*release_delay_ms))
                    })
                }
            }
        }
    }

    fn apply_to_block(&mut self, samples: &mut [f32; STEREO_BLOCK_SAMPLES]) {
        if matches!(self.mode, MicActivationMode::AlwaysOn) && (self.current_gain - 1.0).abs() < 1e-5
        {
            return;
        }
        let target_gain = if self.is_open_now() { 1.0 } else { 0.0 };
        if (self.current_gain - target_gain).abs() < 1e-5 {
            self.current_gain = target_gain;
            if target_gain == 0.0 {
                samples.fill(0.0);
            }
            return;
        }

        // Smooth ~5 ms linear ramp (240 frames at 48 kHz) avoids clicks when opening or releasing PTT.
        let step = 1.0 / 240.0;
        for frame in 0..AAC_FRAME_SAMPLES {
            if self.current_gain < target_gain {
                self.current_gain = (self.current_gain + step).min(target_gain);
            } else if self.current_gain > target_gain {
                self.current_gain = (self.current_gain - step).max(target_gain);
            }
            samples[frame * 2] *= self.current_gain;
            samples[frame * 2 + 1] *= self.current_gain;
        }
    }
}

/// Returns the optimal per-track AAC bitrate (`128 kbps` for Mixed/Game/Secondary, `96 kbps` for Mic).
#[must_use]
pub fn target_aac_bitrate_kbps(track: TrackKind, configured_kbps: u32) -> u32 {
    let clamped = configured_kbps.clamp(64, 320);
    match track {
        TrackKind::AudioMic => clamped.min(96),
        TrackKind::AudioMixed
        | TrackKind::AudioGame
        | TrackKind::AudioSecondary
        | TrackKind::AudioApp(_)
        | TrackKind::Video => clamped.min(128),
    }
}

/// Checks safely via Win32 `GetAsyncKeyState` whether a Push-to-Talk / Push-to-Mute key
/// descriptor (e.g. `"V"`, `"Mouse5"`, `"CapsLock"`, `"Ctrl+V"`) is currently held down.
#[must_use]
pub fn is_ptt_key_down(key_descriptor: &str) -> bool {
    let trimmed = key_descriptor.trim();
    if trimmed.is_empty() {
        return false;
    }
    let mut saw_part = false;
    for part in trimmed.split('+') {
        let Some(vk) = key_name_to_vk(part.trim()) else {
            return false;
        };
        saw_part = true;
        let state = unsafe { GetAsyncKeyState(vk) } as u16;
        if (state & 0x8000) == 0 {
            return false;
        }
    }
    saw_part
}

#[must_use]
fn key_name_to_vk(name: &str) -> Option<i32> {
    let upper = name.trim().to_ascii_uppercase();
    if upper.len() == 1 {
        let ch = upper.as_bytes()[0];
        if ch.is_ascii_alphanumeric() {
            return Some(i32::from(ch));
        }
    }
    match upper.as_str() {
        "SPACE" => Some(i32::from(VK_SPACE.0)),
        "TAB" => Some(i32::from(VK_TAB.0)),
        "CAPSLOCK" | "CAPS" => Some(i32::from(VK_CAPITAL.0)),
        "CTRL" | "CONTROL" => Some(i32::from(VK_CONTROL.0)),
        "LCTRL" | "LEFTCONTROL" | "CONTROLLEFT" => Some(i32::from(VK_LCONTROL.0)),
        "RCTRL" | "RIGHTCONTROL" | "CONTROLRIGHT" => Some(i32::from(VK_RCONTROL.0)),
        "SHIFT" => Some(i32::from(VK_SHIFT.0)),
        "LSHIFT" | "LEFTSHIFT" | "SHIFTLEFT" => Some(i32::from(VK_LSHIFT.0)),
        "RSHIFT" | "RIGHTSHIFT" | "SHIFTRIGHT" => Some(i32::from(VK_RSHIFT.0)),
        "ALT" | "MENU" => Some(i32::from(VK_MENU.0)),
        "LALT" | "LEFTALT" | "ALTLEFT" => Some(i32::from(VK_LMENU.0)),
        "RALT" | "RIGHTALT" | "ALTRIGHT" => Some(i32::from(VK_RMENU.0)),
        "MOUSE3" | "MBUTTON" => Some(i32::from(VK_MBUTTON.0)),
        "MOUSE4" | "XBUTTON1" => Some(i32::from(VK_XBUTTON1.0)),
        "MOUSE5" | "XBUTTON2" => Some(i32::from(VK_XBUTTON2.0)),
        "F1" => Some(i32::from(VK_F1.0)),
        "F2" => Some(i32::from(VK_F2.0)),
        "F3" => Some(i32::from(VK_F3.0)),
        "F4" => Some(i32::from(VK_F4.0)),
        "F5" => Some(i32::from(VK_F5.0)),
        "F6" => Some(i32::from(VK_F6.0)),
        "F7" => Some(i32::from(VK_F7.0)),
        "F8" => Some(i32::from(VK_F8.0)),
        "F9" => Some(i32::from(VK_F9.0)),
        "F10" => Some(i32::from(VK_F10.0)),
        "F11" => Some(i32::from(VK_F11.0)),
        "F12" => Some(i32::from(VK_F12.0)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ptt_vk_parser_handles_common_keys() {
        assert_eq!(key_name_to_vk("v"), Some(i32::from(b'V')));
        assert_eq!(key_name_to_vk("Mouse5"), Some(i32::from(VK_XBUTTON2.0)));
        assert_eq!(key_name_to_vk("CapsLock"), Some(i32::from(VK_CAPITAL.0)));
        assert!(key_name_to_vk("NonExistentKeyXYZ").is_none());
    }

    #[test]
    fn pro_audio_guard_and_startup_barrier_synchronize_cleanly() {
        let _guard = AudioThreadGuard::enter_pro_audio(true);
        let clock = MonotonicClock::now();
        let barrier = Arc::new(StartupBarrier::new(2, clock.now_us()));
        let b2 = Arc::clone(&barrier);
        let h = thread::spawn(move || b2.mark_ready_and_wait(clock, Duration::from_millis(300)));
        thread::sleep(Duration::from_millis(15));
        let t1 = barrier.mark_ready_and_wait(clock, Duration::from_millis(300));
        let t2 = h.join().unwrap_or(0);
        assert_eq!(t1, t2);
        assert!(t1 >= 0);
    }

    #[test]
    fn per_track_aac_bitrate_targets_128k_and_96k_mic() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(target_aac_bitrate_kbps(TrackKind::AudioMixed, 192), 128);
        assert_eq!(target_aac_bitrate_kbps(TrackKind::AudioGame, 192), 128);
        assert_eq!(target_aac_bitrate_kbps(TrackKind::AudioSecondary, 192), 128);
        assert_eq!(target_aac_bitrate_kbps(TrackKind::AudioMic, 192), 96);
        assert_eq!(target_aac_bitrate_kbps(TrackKind::AudioMic, 128), 96);

        let mixed = MfAacEncoder::new(
            TrackKind::AudioMixed,
            target_aac_bitrate_kbps(TrackKind::AudioMixed, 192),
        )?;
        let game = MfAacEncoder::new(
            TrackKind::AudioGame,
            target_aac_bitrate_kbps(TrackKind::AudioGame, 192),
        )?;
        let mic = MfAacEncoder::new(
            TrackKind::AudioMic,
            target_aac_bitrate_kbps(TrackKind::AudioMic, 192),
        )?;

        assert_eq!(mixed.stream_header().bitrate_bps, 128_000);
        assert_eq!(game.stream_header().bitrate_bps, 128_000);
        assert_eq!(mic.stream_header().bitrate_bps, 96_000);
        Ok(())
    }

    #[test]
    fn mixer_never_stalls_when_mic_drops_or_skips_blocks() -> Result<(), Box<dyn std::error::Error>> {
        let clock = MonotonicClock::now();
        let startup = Arc::new(StartupBarrier::new(0, 0));
        let (pcm_tx, pcm_rx) = bounded::<CapturedBlock>(64);
        let (cmd_tx, cmd_rx) = bounded::<MixerControl>(16);
        let (out_tx, out_rx) = bounded::<AudioOutputEvent>(256);

        let mixed_tel = Arc::new(TrackTelemetryShared::new("Mixed"));
        let game_tel = Arc::new(TrackTelemetryShared::new("Game"));
        let mic_tel = Arc::new(TrackTelemetryShared::new("Mic"));
        let sec_tel = Arc::new(TrackTelemetryShared::new("Sec"));
        let has_sec = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::new(AtomicBool::new(false));

        let settings = AudioSettings {
            game_enabled: true,
            mic_enabled: true,
            mic_device: MicDeviceConfig::Default,
            separate_tracks: true,
            ..AudioSettings::default()
        };

        let handle = thread::spawn({
            let mixed_tel = Arc::clone(&mixed_tel);
            let game_tel = Arc::clone(&game_tel);
            let mic_tel = Arc::clone(&mic_tel);
            let sec_tel = Arc::clone(&sec_tel);
            let stop_flag = Arc::clone(&stop_flag);
            move || {
                let base_pts = Arc::new(AtomicI64::new(0));
                run_mixer_loop(
                    settings,
                    startup,
                    base_pts,
                    clock,
                    pcm_rx,
                    cmd_rx,
                    out_tx,
                    mixed_tel,
                    game_tel,
                    mic_tel,
                    sec_tel,
                    has_sec,
                    Vec::new(),
                    stop_flag,
                );
            }
        });

        let tone = [0.25f32; STEREO_BLOCK_SAMPLES];
        // Mic only sends blocks 0..2 then disappears; Game sends blocks 0..12 (with a gap at 5..7).
        for idx in [0u64, 1, 2, 3, 4, 8, 9, 10, 11, 12] {
            let _ = pcm_tx.send(CapturedBlock {
                slot: SourceSlot::Game,
                block_idx: idx,
                samples: tone,
            });
            if idx < 2 {
                let _ = pcm_tx.send(CapturedBlock {
                    slot: SourceSlot::Mic,
                    block_idx: idx,
                    samples: tone,
                });
            }
        }

        // Wait past `MIXER_SLOT_LATE_DEADLINE_US` (120 ms) + 12 * 21.3 ms (~380 ms).
        thread::sleep(Duration::from_millis(450));
        stop_flag.store(true, Ordering::Relaxed);
        let _ = cmd_tx.send(MixerControl::Stop);
        let _ = handle.join();

        let mut mixed_pkts = 0usize;
        let mut game_pkts = 0usize;
        while let Ok(evt) = out_rx.try_recv() {
            if let AudioOutputEvent::Packet(p) = evt {
                match p.track {
                    TrackKind::AudioMixed => mixed_pkts += 1,
                    TrackKind::AudioGame => game_pkts += 1,
                    _ => {}
                }
            }
        }

        assert!(
            mixed_pkts >= 8,
            "mixer must keep emitting AudioMixed even when Mic drops & Game skips slots, got {mixed_pkts}"
        );
        assert!(
            game_pkts >= 8,
            "mixer must keep emitting AudioGame even when Mic drops, got {game_pkts}"
        );
        Ok(())
    }

    #[test]
    fn mixer_aligns_late_game_blocks_without_dropping_after_jitter_or_silence(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let clock = MonotonicClock::now();
        let startup = Arc::new(StartupBarrier::new(0, 0));
        let (pcm_tx, pcm_rx) = bounded::<CapturedBlock>(64);
        let (cmd_tx, cmd_rx) = bounded::<MixerControl>(16);
        let (out_tx, out_rx) = bounded::<AudioOutputEvent>(256);

        let mixed_tel = Arc::new(TrackTelemetryShared::new("Mixed"));
        let game_tel = Arc::new(TrackTelemetryShared::new("Game"));
        let mic_tel = Arc::new(TrackTelemetryShared::new("Mic"));
        let sec_tel = Arc::new(TrackTelemetryShared::new("Sec"));
        let has_sec = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::new(AtomicBool::new(false));

        let settings = AudioSettings {
            game_enabled: true,
            mic_enabled: true,
            mic_device: MicDeviceConfig::Default,
            separate_tracks: true,
            ..AudioSettings::default()
        };

        let handle = thread::spawn({
            let mixed_tel = Arc::clone(&mixed_tel);
            let game_tel = Arc::clone(&game_tel);
            let mic_tel = Arc::clone(&mic_tel);
            let sec_tel = Arc::clone(&sec_tel);
            let stop_flag = Arc::clone(&stop_flag);
            move || {
                let base_pts = Arc::new(AtomicI64::new(0));
                run_mixer_loop(
                    settings,
                    startup,
                    base_pts,
                    clock,
                    pcm_rx,
                    cmd_rx,
                    out_tx,
                    mixed_tel,
                    game_tel,
                    mic_tel,
                    sec_tel,
                    has_sec,
                    Vec::new(),
                    stop_flag,
                );
            }
        });

        let mic_tone = [0.15f32; STEREO_BLOCK_SAMPLES];
        let game_tone = [0.35f32; STEREO_BLOCK_SAMPLES];

        // 1) Mic sends blocks 0..8 immediately, advancing `next_mix_idx` past 0..5 after deadline.
        for idx in 0..8u64 {
            let _ = pcm_tx.send(CapturedBlock {
                slot: SourceSlot::Mic,
                block_idx: idx,
                samples: mic_tone,
            });
        }
        thread::sleep(Duration::from_millis(260));

        // 2) Game arrives late with `block_idx` starting at 0 (behind `next_mix_idx`) + Mic continues 8..14.
        for idx in 8..14u64 {
            let _ = pcm_tx.send(CapturedBlock {
                slot: SourceSlot::Mic,
                block_idx: idx,
                samples: mic_tone,
            });
        }
        for late_idx in 0..6u64 {
            let _ = pcm_tx.send(CapturedBlock {
                slot: SourceSlot::Game,
                block_idx: late_idx,
                samples: game_tone,
            });
        }

        thread::sleep(Duration::from_millis(250));
        stop_flag.store(true, Ordering::Relaxed);
        let _ = cmd_tx.send(MixerControl::Stop);
        let _ = handle.join();

        let game_snap = game_tel.snapshot();
        assert!(
            game_snap.peak_dbfs > -20.0,
            "late Game blocks must NOT be dropped; expected audible peak_dbfs > -20, got {:.1}",
            game_snap.peak_dbfs
        );

        let mut game_pkts = 0usize;
        let mut mixed_pkts = 0usize;
        while let Ok(evt) = out_rx.try_recv() {
            if let AudioOutputEvent::Packet(p) = evt {
                match p.track {
                    TrackKind::AudioGame => game_pkts += 1,
                    TrackKind::AudioMixed => mixed_pkts += 1,
                    _ => {}
                }
            }
        }
        assert!(game_pkts >= 6, "expected >= 6 Game AAC packets, got {game_pkts}");
        assert!(mixed_pkts >= 10, "expected >= 10 Mixed AAC packets, got {mixed_pkts}");
        Ok(())
    }
}
