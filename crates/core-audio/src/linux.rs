//! Linux PipeWire / PulseAudio multi-track audio capture (`libpulse-binding` + GStreamer),
//! pure-Rust RNNoise mic denoising (`nnnoiseless`), Soft Limiter, and GStreamer AAC encoding.

use crate::denoise::{mix_stereo_blocks_soft_clip, MicNoiseProcessor};
use crate::resampler::{
    float_frame_to_i16, linear_to_dbfs, AAC_FRAME_SAMPLES, TARGET_CHANNELS, TARGET_SAMPLE_RATE_HZ,
};
use core_types::{
    AudioDeviceInfo, AudioError, AudioOutputMode, AudioSettings, AudioStreamHeader, MediaPacket,
    MicActivationMode, MicDeviceConfig, MonotonicClock, TrackKind, AUDIO_CHUNK_CHANNEL_CAPACITY,
};
use crossbeam_channel::{bounded, Receiver, Sender};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use libpulse_binding as pulse;
use parking_lot::Mutex;
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tracing::{info, warn};

const STEREO_BLOCK_SAMPLES: usize = AAC_FRAME_SAMPLES * TARGET_CHANNELS;
const AAC_FRAME_DURATION_US: u32 = 21_333;

/// Computes exact sample-index PTS in microseconds (`t0 + idx * 1_000_000 / 48_000`).
#[must_use]
pub const fn sample_index_to_pts_us(base_pts_us: i64, sample_idx: u64) -> i64 {
    let delta_us = (sample_idx.saturating_mul(1_000_000) / (TARGET_SAMPLE_RATE_HZ as u64)) as i64;
    base_pts_us.saturating_add(delta_us)
}

/// Computes elapsed 48 kHz frames since `base_pts_us`.
#[must_use]
pub fn elapsed_48k_frames(base_pts_us: i64, now_us: i64) -> u64 {
    let delta_us = now_us.saturating_sub(base_pts_us).max(0) as u128;
    ((delta_us * u128::from(TARGET_SAMPLE_RATE_HZ)) / 1_000_000u128) as u64
}

/// Maps track role to AAC bitrate (`128 kbps` for Game/Mixed/Secondary, `96 kbps` for Mic).
#[must_use]
pub const fn target_aac_bitrate_kbps(track: TrackKind, configured_kbps: u32) -> u32 {
    let _ = configured_kbps;
    match track {
        TrackKind::AudioMic => 96,
        _ => 128,
    }
}

/// Live diagnostic telemetry per audio track.
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
            active_device: "PipeWire/PulseAudio".to_string(),
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

#[derive(Debug, Clone, Default)]
pub struct AudioPipelineTelemetry {
    pub mixed: AudioTrackTelemetry,
    pub game: AudioTrackTelemetry,
    pub mic: AudioTrackTelemetry,
    pub secondary: Option<AudioTrackTelemetry>,
}

/// Events emitted by the Linux audio capture & AAC encoder pipeline.
#[derive(Debug, Clone)]
pub enum AudioOutputEvent {
    StreamHeader(AudioStreamHeader),
    Packet(MediaPacket),
    RecoverableWarning(String),
}

/// Dynamic audio control commands.
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
}

/// Always returns `false` on headless/Wayland without evdev root access; PTT falls back to OpenMic.
#[must_use]
pub fn is_ptt_key_down(_key_descriptor: &str) -> bool {
    false
}

/// Report returned by [`verify_mp4_audio_with_mf_source_reader`].
#[derive(Debug, Clone)]
pub struct MfAudioVerifyReport {
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub decoded_frames: usize,
    pub peak_linear: f32,
}

/// Decodes the primary audio track of an `.mp4` via GStreamer on Linux to verify AAC playback.
pub fn verify_mp4_audio_with_mf_source_reader(path: &Path) -> Result<MfAudioVerifyReport, AudioError> {
    gst::init().map_err(|e| AudioError::AacEncodeFailed(format!("GStreamer init: {e}")))?;
    let pipeline_str = format!(
        "filesrc location=\"{}\" ! qtdemux ! aacparse ! avdec_aac ! audioconvert ! audio/x-raw,format=S16LE,rate=48000,channels=2 ! appsink name=sink emit-signals=false sync=false",
        path.display()
    );
    let el = gst::parse::launch(&pipeline_str)
        .map_err(|e| AudioError::AacEncodeFailed(format!("GStreamer verify parse: {e}")))?;
    let pipeline = el
        .downcast::<gst::Pipeline>()
        .map_err(|_| AudioError::AacEncodeFailed("Not a pipeline".to_string()))?;
    let sink = pipeline
        .by_name("sink")
        .and_then(|e| e.downcast::<gst_app::AppSink>().ok())
        .ok_or_else(|| AudioError::AacEncodeFailed("Missing appsink".to_string()))?;

    pipeline
        .set_state(gst::State::Playing)
        .map_err(|e| AudioError::AacEncodeFailed(format!("Start verify pipeline: {e}")))?;

    let mut decoded_frames = 0usize;
    let mut peak_linear = 0.0f32;

    while let Some(sample) = sink.try_pull_sample(gst::ClockTime::from_mseconds(200)) {
        if let Some(buf) = sample.buffer() {
            if let Ok(map) = buf.map_readable() {
                let bytes = map.as_slice();
                for chunk in bytes.chunks_exact(2) {
                    let s = i16::from_le_bytes([chunk[0], chunk[1]]);
                    peak_linear = peak_linear.max((f32::from(s) / 32_768.0).abs());
                }
                decoded_frames += bytes.len() / 4;
            }
        }
    }

    let _ = pipeline.set_state(gst::State::Null);
    Ok(MfAudioVerifyReport {
        sample_rate_hz: TARGET_SAMPLE_RATE_HZ,
        channels: TARGET_CHANNELS as u16,
        decoded_frames,
        peak_linear,
    })
}

/// Enumerates PulseAudio/PipeWire render endpoints (`.monitor` sources for system loopback).
pub fn enumerate_render_devices() -> Result<Vec<AudioDeviceInfo>, AudioError> {
    let devs = enumerate_pulse_devices(true);
    if devs.is_empty() {
        Ok(vec![AudioDeviceInfo {
            id: "default".to_string(),
            name: "Default PipeWire/PulseAudio Output".to_string(),
            is_default: true,
        }])
    } else {
        Ok(devs)
    }
}

/// Enumerates PulseAudio/PipeWire render endpoints with activity flag.
pub fn enumerate_render_devices_with_activity(
) -> Result<Vec<(AudioDeviceInfo, f32, String)>, AudioError> {
    let devs = enumerate_render_devices()?;
    Ok(devs
        .into_iter()
        .map(|d| {
            let role = if d.is_default {
                "Default".to_string()
            } else {
                String::new()
            };
            (d, 0.0, role)
        })
        .collect())
}

/// Enumerates PulseAudio/PipeWire microphone capture endpoints.
pub fn enumerate_capture_devices() -> Result<Vec<AudioDeviceInfo>, AudioError> {
    let devs = enumerate_pulse_devices(false);
    if devs.is_empty() {
        Ok(vec![AudioDeviceInfo {
            id: "default".to_string(),
            name: "Default Microphone (PipeWire/PulseAudio)".to_string(),
            is_default: true,
        }])
    } else {
        Ok(devs)
    }
}

fn enumerate_pulse_devices(want_monitors: bool) -> Vec<AudioDeviceInfo> {
    let Some(mut mainloop) = pulse::mainloop::standard::Mainloop::new() else {
        return Vec::new();
    };
    let Some(mut ctx) = pulse::context::Context::new(&mainloop, "ikoch-enum") else {
        return Vec::new();
    };
    if ctx
        .connect(None, pulse::context::FlagSet::NOFLAGS, None)
        .is_err()
    {
        return Vec::new();
    }

    let start = Instant::now();
    loop {
        if start.elapsed() > Duration::from_secs(2) {
            return Vec::new();
        }
        match mainloop.iterate(false) {
            pulse::mainloop::standard::IterateResult::Quit(_)
            | pulse::mainloop::standard::IterateResult::Err(_) => return Vec::new(),
            pulse::mainloop::standard::IterateResult::Success(_) => {}
        }
        match ctx.get_state() {
            pulse::context::State::Ready => break,
            pulse::context::State::Failed | pulse::context::State::Terminated => return Vec::new(),
            _ => thread::sleep(Duration::from_millis(5)),
        }
    }

    let results: Rc<RefCell<Vec<AudioDeviceInfo>>> = Rc::new(RefCell::new(Vec::new()));
    let done = Rc::new(RefCell::new(false));
    let introspect = ctx.introspect();

    let res_c = Rc::clone(&results);
    let done_c = Rc::clone(&done);
    let op = introspect.get_source_info_list(move |list_res| match list_res {
        pulse::callbacks::ListResult::Item(info) => {
            let is_monitor = info.monitor_of_sink.is_some();
            if is_monitor == want_monitors {
                let id = info
                    .name
                    .as_ref()
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                let name = info
                    .description
                    .as_ref()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| id.clone());
                if !id.is_empty() {
                    let is_first = res_c.borrow().is_empty();
                    res_c.borrow_mut().push(AudioDeviceInfo {
                        id,
                        name,
                        is_default: is_first,
                    });
                }
            }
        }
        pulse::callbacks::ListResult::End | pulse::callbacks::ListResult::Error => {
            *done_c.borrow_mut() = true;
        }
    });

    let wait_start = Instant::now();
    while !*done.borrow() && wait_start.elapsed() < Duration::from_secs(2) {
        if let pulse::mainloop::standard::IterateResult::Err(_) = mainloop.iterate(false) {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    let _ = op;
    ctx.disconnect();
    results.take()
}

/// GStreamer-backed AAC-LC encoder (`48 kHz Stereo S16LE -> raw AAC-LC`).
pub struct MfAacEncoder {
    track: TrackKind,
    header: AudioStreamHeader,
    pipeline: gst::Pipeline,
    appsrc: gst_app::AppSrc,
    appsink: gst_app::AppSink,
    sequence: u64,
}

fn find_aac_encoder_element(bitrate_bps: u32) -> Option<String> {
    if gst::ElementFactory::find("fdkaacenc").is_some() {
        return Some(format!("fdkaacenc bitrate={bitrate_bps}"));
    }
    if gst::ElementFactory::find("avenc_aac").is_some() {
        return Some(format!("avenc_aac bitrate={bitrate_bps}"));
    }
    if gst::ElementFactory::find("faac").is_some() {
        return Some(format!("faac bitrate={bitrate_bps}"));
    }
    if gst::ElementFactory::find("voaacenc").is_some() {
        return Some(format!("voaacenc bitrate={bitrate_bps}"));
    }
    None
}

impl MfAacEncoder {
    /// Creates a GStreamer AAC-LC encoder for `track`.
    pub fn new(track: TrackKind, bitrate_kbps: u32) -> Result<Self, AudioError> {
        gst::init().map_err(|e| AudioError::AacEncodeFailed(format!("GStreamer init: {e}")))?;
        let bitrate_bps = bitrate_kbps.clamp(64, 320) * 1000;
        let enc_el = find_aac_encoder_element(bitrate_bps).ok_or_else(|| {
            AudioError::AacEncodeFailed(
                "No GStreamer AAC encoder found (install gstreamer1.0-libav or gstreamer1.0-plugins-bad)"
                    .to_string(),
            )
        })?;

        let pipeline_str = format!(
            "appsrc name=src is-live=true format=time ! audioconvert ! audioresample ! {enc_el} ! aacparse ! audio/mpeg,mpegversion=4,stream-format=raw ! appsink name=sink emit-signals=false sync=false"
        );
        let el = gst::parse::launch(&pipeline_str)
            .map_err(|e| AudioError::AacEncodeFailed(format!("AAC parse failed: {e}")))?;
        let pipeline = el
            .downcast::<gst::Pipeline>()
            .map_err(|_| AudioError::AacEncodeFailed("Not a pipeline".to_string()))?;
        let appsrc = pipeline
            .by_name("src")
            .and_then(|e| e.downcast::<gst_app::AppSrc>().ok())
            .ok_or_else(|| AudioError::AacEncodeFailed("Missing appsrc".to_string()))?;
        let appsink = pipeline
            .by_name("sink")
            .and_then(|e| e.downcast::<gst_app::AppSink>().ok())
            .ok_or_else(|| AudioError::AacEncodeFailed("Missing appsink".to_string()))?;

        let caps = gst::Caps::builder("audio/x-raw")
            .field("format", "S16LE")
            .field("layout", "interleaved")
            .field("rate", TARGET_SAMPLE_RATE_HZ as i32)
            .field("channels", TARGET_CHANNELS as i32)
            .build();
        appsrc.set_caps(Some(&caps));
        pipeline
            .set_state(gst::State::Playing)
            .map_err(|e| AudioError::AacEncodeFailed(format!("AAC start failed: {e}")))?;

        let header = AudioStreamHeader {
            track,
            sample_rate: TARGET_SAMPLE_RATE_HZ,
            channels: TARGET_CHANNELS as u16,
            bitrate_bps,
            // Standard 48 kHz Stereo AAC-LC AudioSpecificConfig (`0x11 0x90`).
            audio_specific_config: vec![0x11, 0x90],
        };

        Ok(Self {
            track,
            header,
            pipeline,
            appsrc,
            appsink,
            sequence: 0,
        })
    }

    #[must_use]
    pub const fn stream_header(&self) -> &AudioStreamHeader {
        &self.header
    }

    pub fn encode_pcm_block(
        &mut self,
        samples_i16: &[i16; STEREO_BLOCK_SAMPLES],
        pts_us: i64,
    ) -> Result<Vec<MediaPacket>, AudioError> {
        let mut bytes = Vec::with_capacity(STEREO_BLOCK_SAMPLES * 2);
        for &s in samples_i16 {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        let mut buf = gst::Buffer::from_mut_slice(bytes);
        if let Some(buf_ref) = buf.get_mut() {
            buf_ref.set_pts(gst::ClockTime::from_useconds(pts_us.max(0) as u64));
            buf_ref.set_duration(gst::ClockTime::from_useconds(u64::from(AAC_FRAME_DURATION_US)));
        }
        let _ = self.appsrc.push_buffer(buf);

        let mut out = Vec::new();
        while let Some(sample) = self.appsink.try_pull_sample(gst::ClockTime::ZERO) {
            if let Some(b) = sample.buffer() {
                if let Ok(map) = b.map_readable() {
                    let pkt_pts = b
                        .pts()
                        .map_or(pts_us, |p| (p.nseconds() / 1000) as i64);
                    self.sequence = self.sequence.saturating_add(1);
                    out.push(MediaPacket::new(
                        self.track,
                        pkt_pts,
                        pkt_pts,
                        AAC_FRAME_DURATION_US,
                        true,
                        self.sequence,
                        map.as_slice().to_vec(),
                    ));
                }
            }
        }
        Ok(out)
    }

    pub fn flush(&mut self, tail_pts_us: i64) -> Result<Vec<MediaPacket>, AudioError> {
        let _ = self.appsrc.end_of_stream();
        let mut out = Vec::new();
        while let Some(sample) = self.appsink.try_pull_sample(gst::ClockTime::from_mseconds(10)) {
            if let Some(b) = sample.buffer() {
                if let Ok(map) = b.map_readable() {
                    let pkt_pts = b
                        .pts()
                        .map_or(tail_pts_us, |p| (p.nseconds() / 1000) as i64);
                    self.sequence = self.sequence.saturating_add(1);
                    out.push(MediaPacket::new(
                        self.track,
                        pkt_pts,
                        pkt_pts,
                        AAC_FRAME_DURATION_US,
                        true,
                        self.sequence,
                        map.as_slice().to_vec(),
                    ));
                }
            }
        }
        let _ = self.pipeline.set_state(gst::State::Null);
        Ok(out)
    }
}

impl Drop for MfAacEncoder {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

/// Handle controlling Linux PipeWire/PulseAudio capture & mixer threads.
pub struct AudioPipelineHandle {
    cmd_tx: Sender<AudioControlCommand>,
    stop_flag: Arc<AtomicBool>,
    telemetry: Arc<Mutex<AudioPipelineTelemetry>>,
    threads: Vec<JoinHandle<()>>,
}

impl AudioPipelineHandle {
    pub fn set_output_mode(&self, mode: AudioOutputMode) {
        let _ = self.cmd_tx.send(AudioControlCommand::SetOutputMode(mode));
    }

    pub fn set_mic_device(&self, cfg: MicDeviceConfig) {
        let _ = self.cmd_tx.send(AudioControlCommand::SetMicDevice(cfg));
    }

    pub fn set_secondary_output_device(&self, device_id: Option<String>) {
        let _ = self
            .cmd_tx
            .send(AudioControlCommand::SetSecondaryOutputDevice(device_id));
    }

    pub fn set_target_process_id(&self, pid: Option<u32>) {
        let _ = self
            .cmd_tx
            .send(AudioControlCommand::SetTargetProcessId(pid));
    }

    pub fn set_volumes(&self, game_volume: f32, mic_volume: f32, secondary_volume: Option<f32>) {
        let _ = self.cmd_tx.send(AudioControlCommand::SetVolumes {
            game_volume,
            mic_volume,
            secondary_volume,
        });
    }

    pub fn set_game_muted(&self, muted: bool) {
        let _ = self.cmd_tx.send(AudioControlCommand::SetGameMuted(muted));
    }

    pub fn set_mic_muted(&self, muted: bool) {
        let _ = self.cmd_tx.send(AudioControlCommand::SetMicMuted(muted));
    }

    pub fn set_secondary_muted(&self, muted: bool) {
        let _ = self
            .cmd_tx
            .send(AudioControlCommand::SetSecondaryMuted(muted));
    }

    pub fn set_mic_noise_suppression(
        &self,
        enabled: bool,
        strength: f32,
        noise_gate_db: Option<f32>,
    ) {
        let _ = self.cmd_tx.send(AudioControlCommand::SetMicNoiseSuppression {
            enabled,
            strength,
            noise_gate_db,
        });
    }

    pub fn set_mic_activation_mode(&self, mode: MicActivationMode) {
        let _ = self
            .cmd_tx
            .send(AudioControlCommand::SetMicActivationMode(mode));
    }

    pub fn set_mic_soft_limiter(&self, enabled: bool) {
        let _ = self
            .cmd_tx
            .send(AudioControlCommand::SetMicSoftLimiter(enabled));
    }

    pub fn send_command(&self, cmd: AudioControlCommand) {
        let _ = self.cmd_tx.send(cmd);
    }

    #[must_use]
    pub fn telemetry(&self) -> AudioPipelineTelemetry {
        self.telemetry.lock().clone()
    }

    pub fn stop(mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        for h in self.threads.drain(..) {
            let _ = h.join();
        }
    }
}

impl Drop for AudioPipelineHandle {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        for h in self.threads.drain(..) {
            let _ = h.join();
        }
    }
}

fn spawn_pulse_pcm_reader(
    device_name: Option<String>,
    stop_flag: Arc<AtomicBool>,
) -> Option<(Receiver<[f32; STEREO_BLOCK_SAMPLES]>, JoinHandle<()>)> {
    let dev_arg = device_name
        .map(|d| format!("device=\"{d}\""))
        .unwrap_or_default();
    let pipeline_str = format!(
        "pulsesrc {dev_arg} ! audioconvert ! audioresample ! audio/x-raw,format=F32LE,layout=interleaved,rate={TARGET_SAMPLE_RATE_HZ},channels={TARGET_CHANNELS} ! appsink name=sink emit-signals=false sync=false max-buffers=16 drop=true"
    );
    let el = gst::parse::launch(&pipeline_str).ok()?;
    let pipeline = el.downcast::<gst::Pipeline>().ok()?;
    let sink = pipeline
        .by_name("sink")
        .and_then(|e| e.downcast::<gst_app::AppSink>().ok())?;
    if pipeline.set_state(gst::State::Playing).is_err() {
        let _ = pipeline.set_state(gst::State::Null);
        return None;
    }

    let (tx, rx) = bounded::<[f32; STEREO_BLOCK_SAMPLES]>(32);
    let handle = thread::Builder::new()
        .name("ikoch-linux-pulse".to_string())
        .spawn(move || {
            let mut accum: Vec<f32> = Vec::with_capacity(STEREO_BLOCK_SAMPLES * 2);
            while !stop_flag.load(Ordering::Relaxed) {
                if let Some(sample) = sink.try_pull_sample(gst::ClockTime::from_mseconds(20)) {
                    if let Some(buf) = sample.buffer() {
                        if let Ok(map) = buf.map_readable() {
                            for chunk in map.as_slice().chunks_exact(4) {
                                let v = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                                accum.push(if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 });
                            }
                            while accum.len() >= STEREO_BLOCK_SAMPLES {
                                let mut block = [0.0f32; STEREO_BLOCK_SAMPLES];
                                block.copy_from_slice(&accum[..STEREO_BLOCK_SAMPLES]);
                                accum.drain(..STEREO_BLOCK_SAMPLES);
                                let _ = tx.try_send(block);
                            }
                        }
                    }
                }
            }
            let _ = pipeline.set_state(gst::State::Null);
        })
        .ok()?;

    Some((rx, handle))
}

/// Starts the Linux multi-track audio capture + RNNoise + AAC encoding pipeline.
pub fn start_audio_pipeline(
    mut settings: AudioSettings,
    _target_process_id: Option<u32>,
    clock: MonotonicClock,
) -> Result<(AudioPipelineHandle, Receiver<AudioOutputEvent>), AudioError> {
    gst::init().map_err(|e| AudioError::WasapiInitFailed(format!("GStreamer init: {e}")))?;
    let (cmd_tx, cmd_rx) = bounded::<AudioControlCommand>(32);
    let (out_tx, out_rx) = bounded::<AudioOutputEvent>(AUDIO_CHUNK_CHANNEL_CAPACITY);
    let stop_flag = Arc::new(AtomicBool::new(false));
    let telemetry = Arc::new(Mutex::new(AudioPipelineTelemetry::default()));

    let stop_c = Arc::clone(&stop_flag);
    let tel_c = Arc::clone(&telemetry);

    let main_thread = thread::Builder::new()
        .name("ikoch-linux-audio-mixer".to_string())
        .spawn(move || {
            let mixed_kbps = target_aac_bitrate_kbps(TrackKind::AudioMixed, settings.aac_bitrate_kbps);
            let game_kbps = target_aac_bitrate_kbps(TrackKind::AudioGame, settings.aac_bitrate_kbps);
            let mic_kbps = target_aac_bitrate_kbps(TrackKind::AudioMic, settings.aac_bitrate_kbps);

            let mut enc_mixed = match MfAacEncoder::new(TrackKind::AudioMixed, mixed_kbps) {
                Ok(e) => e,
                Err(err) => {
                    warn!("Linux AAC encoder init failed: {err}");
                    return;
                }
            };
            let _ = out_tx.send(AudioOutputEvent::StreamHeader(enc_mixed.stream_header().clone()));

            let mut enc_game = if settings.separate_tracks && settings.game_enabled {
                MfAacEncoder::new(TrackKind::AudioGame, game_kbps).ok().inspect(|e| {
                    let _ = out_tx.send(AudioOutputEvent::StreamHeader(e.stream_header().clone()));
                })
            } else {
                None
            };
            let mut enc_mic = if settings.separate_tracks
                && settings.mic_enabled
                && settings.mic_device != MicDeviceConfig::None
            {
                MfAacEncoder::new(TrackKind::AudioMic, mic_kbps).ok().inspect(|e| {
                    let _ = out_tx.send(AudioOutputEvent::StreamHeader(e.stream_header().clone()));
                })
            } else {
                None
            };

            let game_dev = match &settings.audio_output_mode {
                AudioOutputMode::SpecificDevice(id) => Some(id.clone()),
                _ => enumerate_render_devices()
                    .ok()
                    .and_then(|v| v.into_iter().next().map(|d| d.id)),
            };
            let mic_dev = match &settings.mic_device {
                MicDeviceConfig::None => None,
                MicDeviceConfig::Default => Some("default".to_string()),
                MicDeviceConfig::SpecificDevice(id) => Some(id.clone()),
            };

            let mut sub_threads = Vec::new();
            let game_rx = if settings.game_enabled {
                spawn_pulse_pcm_reader(game_dev, Arc::clone(&stop_c)).map(|(rx, h)| {
                    sub_threads.push(h);
                    rx
                })
            } else {
                None
            };
            let mic_rx = if settings.mic_enabled && settings.mic_device != MicDeviceConfig::None {
                let target = mic_dev.filter(|d| d != "default");
                spawn_pulse_pcm_reader(target, Arc::clone(&stop_c)).map(|(rx, h)| {
                    sub_threads.push(h);
                    rx
                })
            } else {
                None
            };

            let mut mic_denoiser = MicNoiseProcessor::new(
                settings.mic_noise_suppression,
                settings.mic_noise_suppression_strength,
                settings.mic_noise_gate_db,
            );
            mic_denoiser.set_soft_limiter(settings.mic_soft_limiter);

            let base_pts_us = clock.now_us();
            let mut block_idx: u64 = 0;
            let zero_block = [0.0f32; STEREO_BLOCK_SAMPLES];

            info!("Linux PipeWire/PulseAudio mixer started");

            while !stop_c.load(Ordering::Relaxed) {
                while let Ok(cmd) = cmd_rx.try_recv() {
                    match cmd {
                        AudioControlCommand::SetVolumes {
                            game_volume,
                            mic_volume,
                            secondary_volume: _,
                        } => {
                            settings.game_volume = game_volume.clamp(0.0, 2.0);
                            settings.mic_volume = mic_volume.clamp(0.0, 2.0);
                        }
                        AudioControlCommand::SetGameMuted(m) => settings.game_muted = m,
                        AudioControlCommand::SetMicMuted(m) => settings.mic_muted = m,
                        AudioControlCommand::SetSecondaryMuted(_) => {}
                        AudioControlCommand::SetMicNoiseSuppression {
                            enabled,
                            strength,
                            noise_gate_db,
                        } => mic_denoiser.update_config(enabled, strength, noise_gate_db),
                        AudioControlCommand::SetMicSoftLimiter(en) => {
                            mic_denoiser.set_soft_limiter(en)
                        }
                        _ => {}
                    }
                }

                let target_blocks =
                    elapsed_48k_frames(base_pts_us, clock.now_us()) / (AAC_FRAME_SAMPLES as u64);
                if block_idx >= target_blocks {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }

                let game_blk = game_rx
                    .as_ref()
                    .and_then(|rx| rx.try_recv().ok())
                    .unwrap_or(zero_block);
                let mut mic_blk = mic_rx
                    .as_ref()
                    .and_then(|rx| rx.try_recv().ok())
                    .unwrap_or(zero_block);
                mic_denoiser.process_stereo_1024(&mut mic_blk);

                let pts_us = sample_index_to_pts_us(base_pts_us, block_idx * (AAC_FRAME_SAMPLES as u64));
                block_idx = block_idx.saturating_add(1);

                let mixed_blk = mix_stereo_blocks_soft_clip(&[
                    (&game_blk, settings.game_volume, settings.game_muted),
                    (&mic_blk, settings.mic_volume, settings.mic_muted),
                ]);

                let g_peak = game_blk.iter().fold(0.0f32, |a, &b| a.max(b.abs()));
                let m_peak = mic_blk.iter().fold(0.0f32, |a, &b| a.max(b.abs()));
                {
                    let mut t = tel_c.lock();
                    t.game.peak_linear = t.game.peak_linear.max(g_peak);
                    t.game.peak_dbfs = linear_to_dbfs(t.game.peak_linear);
                    t.mic.peak_linear = t.mic.peak_linear.max(m_peak);
                    t.mic.peak_dbfs = linear_to_dbfs(t.mic.peak_linear);
                }

                if let Ok(pkts) = enc_mixed.encode_pcm_block(&float_frame_to_i16(&mixed_blk), pts_us) {
                    for p in pkts {
                        let _ = out_tx.try_send(AudioOutputEvent::Packet(p));
                    }
                }
                if let Some(ref mut enc) = enc_game {
                    if let Ok(pkts) = enc.encode_pcm_block(&float_frame_to_i16(&game_blk), pts_us) {
                        for p in pkts {
                            let _ = out_tx.try_send(AudioOutputEvent::Packet(p));
                        }
                    }
                }
                if let Some(ref mut enc) = enc_mic {
                    if let Ok(pkts) = enc.encode_pcm_block(&float_frame_to_i16(&mic_blk), pts_us) {
                        for p in pkts {
                            let _ = out_tx.try_send(AudioOutputEvent::Packet(p));
                        }
                    }
                }
            }

            let tail = sample_index_to_pts_us(base_pts_us, block_idx * (AAC_FRAME_SAMPLES as u64));
            let _ = enc_mixed.flush(tail);
            if let Some(ref mut enc) = enc_game {
                let _ = enc.flush(tail);
            }
            if let Some(ref mut enc) = enc_mic {
                let _ = enc.flush(tail);
            }
            for h in sub_threads {
                let _ = h.join();
            }
        })
        .map_err(|e| AudioError::WasapiInitFailed(format!("Spawn Linux audio thread: {e}")))?;

    Ok((
        AudioPipelineHandle {
            cmd_tx,
            stop_flag,
            telemetry,
            threads: vec![main_thread],
        },
        out_rx,
    ))
}
