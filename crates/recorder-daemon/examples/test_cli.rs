//! Stage 3 CLI Test Bench for the Complete Zero-Copy Video + Dual-Track Audio Pipeline:
//! - WGC (`core-capture`) -> `ID3D11VideoProcessor` (BGRA -> NV12) -> MF Hardware Video (`core-encode`)
//! - WASAPI Loopback + Microphone (`core-audio`) -> `rubato` 48kHz Stereo -> MF AAC MFT
//! - Unified `MonotonicClock` -> `PacketRouter` + `GopRingBuffer` / `Fmp4DiskWriter` (`core-mux`)
//!
//! Run interactive mode:
//!   cargo run --example test_cli
//!
//! Run automated end-to-end verification:
//!   cargo run --example test_cli -- --self-test

use core_audio::{
    enumerate_capture_devices, enumerate_render_devices, enumerate_render_devices_with_activity,
    start_audio_pipeline, verify_mp4_audio_with_mf_source_reader, AudioOutputEvent,
    AudioPipelineHandle,
};
use core_capture::{
    enumerate_monitors, enumerate_windows, start_capture_thread, CaptureEngineConfig,
    CaptureOutputEvent, CaptureSessionHandle, D3d11SharedContext,
};
use core_encode::{EncoderOutput, MfHardwareEncoder, MfVideoEncoderConfig};
use core_mux::{PacketRouter, SavedMediaSummary};
use core_types::{
    AppSettings, AudioOutputMode, AudioSettings, AudioStreamHeader, CaptureTarget, ClipMetadata,
    GpuVendorPreference, HdrMode, MediaPacket, MicActivationMode, MicDeviceConfig, MonotonicClock,
    RateControlMode, RecorderCommand, RecorderEvent, ResolutionPreset, SoundNotificationKind,
    VideoCodec, VideoStreamHeader, ENCODED_PACKET_CHANNEL_CAPACITY,
};
use recorder_daemon::{
    load_clip_metadata, verify_embedded_presets_decode, RecorderDaemon, StorageQuotaManager,
};
use crossbeam_channel::{bounded, select, Receiver, Sender};
use std::fs::{self, File};
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tracing::{error, info, warn};
#[cfg(windows)]
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
#[cfg(windows)]
use windows::Win32::System::Threading::GetCurrentProcess;

enum MuxThreadMsg {
    SourceReset { epoch: u64 },
    VideoHeader { epoch: u64, header: VideoStreamHeader },
    AudioHeader { header: AudioStreamHeader },
    Packet { epoch: Option<u64>, packet: MediaPacket },
    SaveClip {
        path: PathBuf,
        reply: Sender<Result<SavedMediaSummary, String>>,
    },
    StartSession {
        path: PathBuf,
        reply: Sender<Result<(), String>>,
    },
    StopSession {
        remux_faststart: bool,
        reply: Sender<Result<SavedMediaSummary, String>>,
    },
    QueryStats {
        reply: Sender<(f64, usize, usize, u64, bool, u64)>,
    },
    Shutdown,
}

struct PipelineController {
    capture: CaptureSessionHandle,
    audio: AudioPipelineHandle,
    mux_tx: Sender<MuxThreadMsg>,
    stop_flag: Arc<AtomicBool>,
    encode_thread: Option<JoinHandle<()>>,
    audio_bridge_thread: Option<JoinHandle<()>>,
    mux_thread: Option<JoinHandle<()>>,
}

impl PipelineController {
    fn start(
        initial_target: CaptureTarget,
        audio_settings: AudioSettings,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let clock = MonotonicClock::now();
        let shared_d3d = D3d11SharedContext::new()?;

        let capture_cfg = CaptureEngineConfig {
            initial_target,
            resolution: ResolutionPreset::P1080,
            fps: 60,
            capture_cursor: true,
            capture_border: false,
            hdr_mode: HdrMode::AutoToneMapToSdr,
            autogame_fallback_to_monitor: true,
        };

        let (capture, frame_rx) = start_capture_thread(shared_d3d.clone(), capture_cfg, clock)?;
        let (audio, audio_rx) = start_audio_pipeline(audio_settings, None, clock)?;
        let (mux_tx, mux_rx) = bounded::<MuxThreadMsg>(ENCODED_PACKET_CHANNEL_CAPACITY * 2);
        let stop_flag = Arc::new(AtomicBool::new(false));

        let mux_tx_enc = mux_tx.clone();
        let enc_stop = Arc::clone(&stop_flag);
        let encode_thread = thread::Builder::new()
            .name("rustclip-mf-encode".to_string())
            .spawn(move || run_encode_loop(shared_d3d, frame_rx, mux_tx_enc, enc_stop))?;

        let mux_tx_aud = mux_tx.clone();
        let aud_stop = Arc::clone(&stop_flag);
        let audio_bridge_thread = thread::Builder::new()
            .name("rustclip-audio-bridge".to_string())
            .spawn(move || run_audio_bridge_loop(audio_rx, mux_tx_aud, aud_stop))?;

        let mux_thread = thread::Builder::new()
            .name("rustclip-mux-router".to_string())
            .spawn(move || run_mux_loop(mux_rx))?;

        Ok(Self {
            capture,
            audio,
            mux_tx,
            stop_flag,
            encode_thread: Some(encode_thread),
            audio_bridge_thread: Some(audio_bridge_thread),
            mux_thread: Some(mux_thread),
        })
    }

    fn save_clip(&self, path: PathBuf) -> Result<SavedMediaSummary, String> {
        let (tx, rx) = bounded(1);
        self.mux_tx
            .send(MuxThreadMsg::SaveClip { path, reply: tx })
            .map_err(|_| "Mux thread disconnected".to_string())?;
        rx.recv().map_err(|_| "Mux reply failed".to_string())?
    }

    fn start_session(&self, path: PathBuf) -> Result<(), String> {
        let (tx, rx) = bounded(1);
        self.mux_tx
            .send(MuxThreadMsg::StartSession { path, reply: tx })
            .map_err(|_| "Mux thread disconnected".to_string())?;
        rx.recv().map_err(|_| "Mux reply failed".to_string())?
    }

    fn stop_session(&self, remux_faststart: bool) -> Result<SavedMediaSummary, String> {
        let (tx, rx) = bounded(1);
        self.mux_tx
            .send(MuxThreadMsg::StopSession {
                remux_faststart,
                reply: tx,
            })
            .map_err(|_| "Mux thread disconnected".to_string())?;
        rx.recv().map_err(|_| "Mux reply failed".to_string())?
    }

    fn stats(&self) -> Result<(f64, usize, usize, u64, bool, u64), String> {
        let (tx, rx) = bounded(1);
        self.mux_tx
            .send(MuxThreadMsg::QueryStats { reply: tx })
            .map_err(|_| "Mux thread disconnected".to_string())?;
        rx.recv().map_err(|_| "Mux reply failed".to_string())
    }

    fn switch_target(&self, target: CaptureTarget) -> Result<(), String> {
        self.capture
            .switch_target(target)
            .map_err(|e| e.to_string())
    }

    fn shutdown(mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        self.capture.stop();
        self.audio.stop();
        if let Some(h) = self.encode_thread.take() {
            let _ = h.join();
        }
        if let Some(h) = self.audio_bridge_thread.take() {
            let _ = h.join();
        }
        let _ = self.mux_tx.send(MuxThreadMsg::Shutdown);
        if let Some(h) = self.mux_thread.take() {
            let _ = h.join();
        }
    }
}

fn run_audio_bridge_loop(
    audio_rx: Receiver<AudioOutputEvent>,
    mux_tx: Sender<MuxThreadMsg>,
    stop_flag: Arc<AtomicBool>,
) {
    while !stop_flag.load(Ordering::Relaxed) {
        let Ok(evt) = audio_rx.recv_timeout(Duration::from_millis(50)) else {
            continue;
        };
        match evt {
            AudioOutputEvent::StreamHeader(header) => {
                let _ = mux_tx.send(MuxThreadMsg::AudioHeader { header });
            }
            AudioOutputEvent::Packet(packet) => {
                let _ = mux_tx.try_send(MuxThreadMsg::Packet {
                    epoch: None,
                    packet,
                });
            }
            AudioOutputEvent::RecoverableWarning(msg) => {
                warn!("Audio pipeline notice: {msg}");
            }
        }
    }
}

fn run_encode_loop(
    mut shared_d3d: D3d11SharedContext,
    frame_rx: Receiver<CaptureOutputEvent>,
    mux_tx: Sender<MuxThreadMsg>,
    stop_flag: Arc<AtomicBool>,
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
                target,
                display_label,
                width,
                height,
            } => {
                info!(
                    "Capture source ready [epoch={epoch}]: '{}' ({:?}) -> {}x{}",
                    display_label, target, width, height
                );
                if let Some(mut old_enc) = encoder.take() {
                    let _ = old_enc.flush();
                }
                active_epoch = epoch;
                let _ = mux_tx.send(MuxThreadMsg::SourceReset { epoch });

                let enc_cfg = MfVideoEncoderConfig {
                    codec: VideoCodec::H264,
                    vendor: GpuVendorPreference::Auto,
                    rate_control: RateControlMode::Vbr,
                    width,
                    height,
                    fps: 60,
                    bitrate_kbps: 15_000,
                    gop_seconds: 1,
                };
                match MfHardwareEncoder::new(&shared_d3d.device, enc_cfg) {
                    Ok(enc) => {
                        info!("Encoder active: {}", enc.friendly_name());
                        encoder = Some(enc);
                    }
                    Err(err) => {
                        error!("Failed to initialize hardware encoder: {err}");
                    }
                }
            }
            CaptureOutputEvent::DeviceRecreated(new_shared) => {
                warn!("GPU device recreated; resetting Media Foundation encoder");
                shared_d3d = new_shared;
                encoder = None;
            }
            CaptureOutputEvent::RecoverableWarning(msg) => {
                warn!("Capture warning: {msg}");
            }
            CaptureOutputEvent::Nv12Frame(frame) => {
                if frame.source_epoch != active_epoch {
                    continue;
                }
                let Some(enc) = encoder.as_mut() else {
                    continue;
                };
                match enc.encode_gpu_frame(&frame) {
                    Ok(outputs) => {
                        for out in outputs {
                            match out {
                                EncoderOutput::StreamHeader(header) => {
                                    let _ = mux_tx.send(MuxThreadMsg::VideoHeader {
                                        epoch: active_epoch,
                                        header,
                                    });
                                }
                                EncoderOutput::Packet(packet) => {
                                    let _ = mux_tx.try_send(MuxThreadMsg::Packet {
                                        epoch: Some(active_epoch),
                                        packet,
                                    });
                                }
                            }
                        }
                    }
                    Err(err) if err.is_recoverable() => {
                        warn!("Recoverable encoder error ({err}); resetting encoder");
                        encoder = None;
                    }
                    Err(err) => {
                        error!("Encoder error: {err}");
                    }
                }
            }
        }
    }

    if let Some(mut enc) = encoder.take() {
        if let Ok(outputs) = enc.flush() {
            for out in outputs {
                if let EncoderOutput::Packet(packet) = out {
                    let _ = mux_tx.try_send(MuxThreadMsg::Packet {
                        epoch: Some(active_epoch),
                        packet,
                    });
                }
            }
        }
    }
}

fn run_mux_loop(mux_rx: Receiver<MuxThreadMsg>) {
    let mut router = match PacketRouter::new(15, 256 * 1024 * 1024) {
        Ok(r) => r,
        Err(err) => {
            error!("PacketRouter init failed: {err}");
            return;
        }
    };
    let mut current_epoch: u64 = 0;

    loop {
        select! {
            recv(mux_rx) -> msg => {
                let Ok(cmd) = msg else { break; };
                match cmd {
                    MuxThreadMsg::SourceReset { epoch } => {
                        current_epoch = epoch;
                        router.on_source_reset();
                    }
                    MuxThreadMsg::VideoHeader { epoch, header } => {
                        if epoch == current_epoch {
                            router.update_video_header(header);
                        }
                    }
                    MuxThreadMsg::AudioHeader { header } => {
                        router.update_audio_header(header);
                    }
                    MuxThreadMsg::Packet { epoch, packet } => {
                        if epoch.is_none_or(|e| e == current_epoch) {
                            router.push_packet(packet);
                        }
                    }
                    MuxThreadMsg::SaveClip { path, reply } => {
                        let _ = reply.send(router.save_replay_clip(&path).map_err(|e| e.to_string()));
                    }
                    MuxThreadMsg::StartSession { path, reply } => {
                        let _ = reply.send(router.start_session(&path).map_err(|e| e.to_string()));
                    }
                    MuxThreadMsg::StopSession { remux_faststart, reply } => {
                        let _ = reply.send(router.stop_session(remux_faststart).map_err(|e| e.to_string()));
                    }
                    MuxThreadMsg::QueryStats { reply } => {
                        let (fill, bytes, gops, drops, sess) = router.stats();
                        let _ = reply.send((fill, bytes, gops, drops, sess, current_epoch));
                    }
                    MuxThreadMsg::Shutdown => {
                        let (_, _, _, _, sess) = router.stats();
                        if sess {
                            let _ = router.stop_session(true);
                        }
                        break;
                    }
                }
            }
        }
    }
}

#[cfg(windows)]
fn process_working_set_mb() -> f64 {
    let mut counters = PROCESS_MEMORY_COUNTERS::default();
    let ok = unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters,
            size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
    };
    if ok.is_ok() {
        (counters.WorkingSetSize as f64) / (1024.0 * 1024.0)
    } else {
        0.0
    }
}

#[cfg(not(windows))]
fn process_working_set_mb() -> f64 {
    0.0
}

struct Mp4Inspection {
    size_bytes: u64,
    video_tracks: usize,
    audio_tracks: usize,
    tkhd_flags: Vec<u32>,
    has_mixed_label: bool,
    has_game_label: bool,
    has_mic_label: bool,
    track_durations_sec: Vec<(String, f64)>,
}

fn inspect_mp4_file(path: &Path) -> Result<Mp4Inspection, String> {
    let meta = fs::metadata(path).map_err(|e| format!("metadata failed: {e}"))?;
    if meta.len() < 128 {
        return Err(format!(
            "MP4 '{}' is unexpectedly small ({} bytes)",
            path.display(),
            meta.len()
        ));
    }

    let mut f = File::open(path).map_err(|e| format!("open failed: {e}"))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)
        .map_err(|e| format!("read failed: {e}"))?;

    if buf.get(4..8) != Some(b"ftyp") {
        return Err(format!("Missing ftyp box in '{}'", path.display()));
    }
    let moov_pos = buf
        .windows(4)
        .position(|w| w == b"moov")
        .ok_or_else(|| format!("Missing moov box in '{}'", path.display()))?;
    let mdat_pos = buf
        .windows(4)
        .position(|w| w == b"mdat")
        .ok_or_else(|| format!("Missing mdat box in '{}'", path.display()))?;
    if moov_pos >= mdat_pos {
        return Err(format!(
            "Expected faststart moov before mdat in '{}'",
            path.display()
        ));
    }

    let stss_pos = buf
        .windows(4)
        .position(|w| w == b"stss")
        .ok_or_else(|| format!("Missing stss sync-sample table in '{}'", path.display()))?;
    let first_sync = buf
        .get(stss_pos + 12..stss_pos + 16)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
        .unwrap_or(0);
    if first_sync != 1 {
        return Err(format!(
            "First sample in '{}' is not an IDR keyframe (first stss entry = {first_sync})",
            path.display()
        ));
    }

    let moov_slice = &buf[moov_pos..mdat_pos];
    let video_tracks = moov_slice
        .windows(4)
        .filter(|w| *w == b"avc1" || *w == b"hvc1")
        .count();
    let audio_tracks = moov_slice.windows(4).filter(|w| *w == b"mp4a").count();
    let tkhd_flags: Vec<u32> = moov_slice
        .windows(8)
        .filter(|w| &w[0..4] == b"tkhd")
        .map(|w| u32::from_be_bytes([0, w[5], w[6], w[7]]))
        .collect();
    let has_mixed_label = moov_slice.windows(11).any(|w| w == b"Mixed Audio");
    let has_game_label = moov_slice.windows(10).any(|w| w == b"Game Audio");
    let has_mic_label = moov_slice.windows(10).any(|w| w == b"Microphone");

    let mut track_durations_sec = Vec::new();
    for (idx, win) in moov_slice.windows(4).enumerate() {
        if win == b"mdhd" && idx >= 4 {
            let mdhd_start = idx - 4;
            if let Some(mdhd) = moov_slice.get(mdhd_start..mdhd_start + 32) {
                let timescale = u32::from_be_bytes([mdhd[20], mdhd[21], mdhd[22], mdhd[23]]);
                let dur = u32::from_be_bytes([mdhd[24], mdhd[25], mdhd[26], mdhd[27]]);
                let sec = if timescale > 0 {
                    f64::from(dur) / f64::from(timescale)
                } else {
                    0.0
                };
                let label = match track_durations_sec.len() {
                    0 => "Video".to_string(),
                    1 => "Track1(Mixed)".to_string(),
                    2 => "Track2(Game)".to_string(),
                    3 => "Track3(Mic)".to_string(),
                    4 => "Track4(Secondary)".to_string(),
                    n => format!("Track{n}"),
                };
                track_durations_sec.push((label, sec));
            }
        }
    }

    Ok(Mp4Inspection {
        size_bytes: meta.len(),
        video_tracks,
        audio_tracks,
        tkhd_flags,
        has_mixed_label,
        has_game_label,
        has_mic_label,
        track_durations_sec,
    })
}

fn run_self_test() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== RustClip Stage 3 Diagnostic & 5-Second Live A/V Self-Test ===");

    let render_devs = enumerate_render_devices_with_activity()?;
    let capture_devs = enumerate_capture_devices()?;
    println!(
        "Discovered {} active Render device(s) (with live ~150ms meter peak):",
        render_devs.len()
    );
    for (idx, (d, peak, note)) in render_devs.iter().enumerate() {
        let def_badge = if d.is_default { "[DEFAULT]" } else { "         " };
        println!(
            "  [{idx:02}] {def_badge} {} | peak={:.4} | {} | id={}",
            d.name, peak, note, d.id
        );
    }
    println!("Discovered {} active Microphone device(s):", capture_devs.len());
    for (idx, d) in capture_devs.iter().enumerate() {
        let def_badge = if d.is_default { "[DEFAULT]" } else { "         " };
        println!("  [{idx:02}] {def_badge} {} | id={}", d.name, d.id);
    }

    let monitors = enumerate_monitors()?;
    let windows = enumerate_windows()?;
    println!(
        "Discovered {} monitor(s), {} capturable window(s)",
        monitors.len(),
        windows.len()
    );
    verify_embedded_presets_decode()?;
    println!("Verified embedded rodio WAV presets (chime, start, stop, tick) decode cleanly.");

    let primary = monitors
        .iter()
        .find(|m| m.is_primary)
        .or_else(|| monitors.first())
        .ok_or("No monitor available for self-test")?;

    let initial_target = CaptureTarget::Monitor {
        monitor_id: primary.monitor_id.clone(),
        name: primary.name.clone(),
    };

    let out_dir = PathBuf::from("clips");
    fs::create_dir_all(&out_dir)?;
    let clip_path = out_dir.join("selftest_av_clip.mp4");
    let session_path = out_dir.join("selftest_av_session.mp4");

    let audio_settings = AudioSettings {
        audio_output_mode: AudioOutputMode::DefaultDevice,
        mic_device: MicDeviceConfig::Default,
        ..AudioSettings::default()
    };
    println!(
        "Audio config: RNNoise={} (strength={:.2}, gate={:?} dBFS) | separate_tracks={}",
        audio_settings.mic_noise_suppression,
        audio_settings.mic_noise_suppression_strength,
        audio_settings.mic_noise_gate_db,
        audio_settings.separate_tracks
    );

    let ctrl = PipelineController::start(initial_target, audio_settings)?;

    // Prime buffer (1s+)
    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        thread::sleep(Duration::from_millis(200));
        if let Ok((fill_sec, bytes, gops, _, _, _)) = ctrl.stats() {
            if gops >= 1 && fill_sec >= 0.8 {
                println!(
                    "Initial A/V buffer primed: {:.2}s, {} GOP(s), {} bytes",
                    fill_sec, gops, bytes
                );
                break;
            }
        }
        if Instant::now() >= deadline {
            return Err("Timed out waiting for initial encoded GOP".into());
        }
    }

    ctrl.start_session(session_path.clone())?;
    println!("Session started (5-second live multi-track A/V capture) -> {}", session_path.display());

    // Record 5 seconds while monitoring live audio peaks.
    for step in 1..=10 {
        thread::sleep(Duration::from_millis(500));
        let tel = ctrl.audio.telemetry();
        println!(
            "  [{step:02}/10] Mixed peak={:.4} ({:.1} dBFS) | Game peak={:.4} ({:.1} dBFS, real={}) | Mic(post-RNNoise) peak={:.4} ({:.1} dBFS, real={})",
            tel.mixed.peak_linear,
            tel.mixed.peak_dbfs,
            tel.game.peak_linear,
            tel.game.peak_dbfs,
            tel.game.real_frames_read,
            tel.mic.peak_linear,
            tel.mic.peak_dbfs,
            tel.mic.real_frames_read
        );
    }

    let tel_final = ctrl.audio.telemetry();
    println!("\n=== Final Multi-Track Audio Telemetry ===");
    println!(
        "  Track 1 (MixedAudio)   : device='{}' | aac_packets={} | peak={:.4} ({:.1} dBFS)",
        tel_final.mixed.active_device,
        tel_final.mixed.encoded_packets,
        tel_final.mixed.peak_linear,
        tel_final.mixed.peak_dbfs
    );
    println!(
        "  Track 2 (GameAudio)    : device='{}' | real_frames={} | silent_injected={} (blocks={}) | discontinuities={} | mixer_underruns={} | aac_packets={} | peak={:.4} ({:.1} dBFS)",
        tel_final.game.active_device,
        tel_final.game.real_frames_read,
        tel_final.game.silent_frames_injected,
        tel_final.game.silent_injected_blocks,
        tel_final.game.discontinuities,
        tel_final.game.mixer_underruns,
        tel_final.game.encoded_packets,
        tel_final.game.peak_linear,
        tel_final.game.peak_dbfs
    );
    println!(
        "  Track 3 (Microphone)   : device='{}' | real_frames={} | silent_injected={} (blocks={}) | discontinuities={} | mixer_underruns={} | aac_packets={} | peak={:.4} ({:.1} dBFS)",
        tel_final.mic.active_device,
        tel_final.mic.real_frames_read,
        tel_final.mic.silent_frames_injected,
        tel_final.mic.silent_injected_blocks,
        tel_final.mic.discontinuities,
        tel_final.mic.mixer_underruns,
        tel_final.mic.encoded_packets,
        tel_final.mic.peak_linear,
        tel_final.mic.peak_dbfs
    );
    if let Some(sec) = &tel_final.secondary {
        println!(
            "  Track 4 (Secondary)    : device='{}' | real_frames={} | discontinuities={} | mixer_underruns={} | aac_packets={} | peak={:.4} ({:.1} dBFS)",
            sec.active_device,
            sec.real_frames_read,
            sec.discontinuities,
            sec.mixer_underruns,
            sec.encoded_packets,
            sec.peak_linear,
            sec.peak_dbfs
        );
    }
    if tel_final.game.discontinuities > 0
        || tel_final.mic.discontinuities > 0
        || tel_final.game.mixer_underruns > 0
        || tel_final.mic.mixer_underruns > 0
    {
        return Err(format!(
            "Audio glitch detected: game(disc={}, underruns={}), mic(disc={}, underruns={})",
            tel_final.game.discontinuities,
            tel_final.game.mixer_underruns,
            tel_final.mic.discontinuities,
            tel_final.mic.mixer_underruns
        )
        .into());
    }
    println!();

    let clip_summary = ctrl.save_clip(clip_path.clone())?;
    println!(
        "Replay A/V clip saved: {} ({:.2}s, {} packets, {} bytes)",
        clip_summary.path.display(),
        clip_summary.duration_sec,
        clip_summary.packet_count,
        clip_summary.bytes_written
    );

    let session_summary = ctrl.stop_session(true)?;
    println!(
        "Session A/V stopped & faststart remuxed: {} ({:.2}s, {} packets, {} bytes)",
        session_summary.path.display(),
        session_summary.duration_sec,
        session_summary.packet_count,
        session_summary.bytes_written
    );

    let (_, buf_bytes, _, dropped_disk, _, _) = ctrl.stats()?;
    let total_ws_mb = process_working_set_mb();
    let buf_mb = (buf_bytes as f64) / (1024.0 * 1024.0);
    let non_buffer_mb = (total_ws_mb - buf_mb).max(0.0);

    ctrl.shutdown();

    for p in [&clip_path, &session_path] {
        let info = inspect_mp4_file(p)?;
        println!(
            "Verified MP4 '{}' -> {} bytes | video_tracks={} | audio_tracks={} | tkhd_flags={:?} | labels=[Mixed:{}, Game:{}, Mic:{}] | durations={:?}",
            p.display(),
            info.size_bytes,
            info.video_tracks,
            info.audio_tracks,
            info.tkhd_flags,
            info.has_mixed_label,
            info.has_game_label,
            info.has_mic_label,
            info.track_durations_sec
        );
        if info.video_tracks != 1 || info.audio_tracks != 3 {
            return Err(format!(
                "Expected 1 video and 3 audio tracks (Mixed + Game + Mic) in '{}', got video={}, audio={}",
                p.display(),
                info.video_tracks,
                info.audio_tracks
            )
            .into());
        }
        if info.tkhd_flags != vec![0x03, 0x03, 0x02, 0x02]
            || !info.has_mixed_label
            || !info.has_game_label
            || !info.has_mic_label
        {
            return Err(format!(
                "Unexpected track flags/labels in '{}': flags={:?}, mixed={}, game={}, mic={}",
                p.display(),
                info.tkhd_flags,
                info.has_mixed_label,
                info.has_game_label,
                info.has_mic_label
            )
            .into());
        }
        if info.track_durations_sec.len() == 4 {
            let v_dur = info.track_durations_sec[0].1;
            let mix_dur = info.track_durations_sec[1].1;
            let game_dur = info.track_durations_sec[2].1;
            let mic_dur = info.track_durations_sec[3].1;
            let skew_mix_ms = (v_dur - mix_dur).abs() * 1000.0;
            let skew_game_ms = (v_dur - game_dur).abs() * 1000.0;
            let skew_mic_ms = (v_dur - mic_dur).abs() * 1000.0;
            let skew_stems_ms = (mix_dur - game_dur).abs().max((mix_dur - mic_dur).abs()) * 1000.0;
            println!(
                "  A/V sync delta -> |V - Mixed|={:.1} ms | |V - Game|={:.1} ms | |V - Mic|={:.1} ms | |Mixed - Stems|={:.1} ms",
                skew_mix_ms, skew_game_ms, skew_mic_ms, skew_stems_ms
            );
        }

        let mf_report = verify_mp4_audio_with_mf_source_reader(p)?;
        println!(
            "  MFCreateSourceReaderFromURL OK -> {} Hz, {} ch, {} frames ({:.2}s), peak={:.4} ({:.1} dBFS)",
            mf_report.sample_rate_hz,
            mf_report.channels,
            mf_report.decoded_frames,
            mf_report.duration_sec,
            mf_report.peak_linear,
            mf_report.peak_dbfs
        );
    }

    println!(
        "Memory telemetry -> Total Working Set: {:.2} MiB | Buffer: {:.2} MiB | Overhead (excl. buffer): {:.2} MiB | Disk drops: {}",
        total_ws_mb, buf_mb, non_buffer_mb, dropped_disk
    );

    // Stage 4 Daemon end-to-end verification: Auto-buffer + F8 Bookmark + JSON Sidecar + Privacy Mode + Quota
    println!("\n=== Stage 4 RecorderDaemon Integration Self-Test ===");
    let daemon_dir = PathBuf::from("clips/daemon_selftest");
    let _ = fs::remove_dir_all(&daemon_dir);
    fs::create_dir_all(&daemon_dir)?;

    let mut d_settings = AppSettings::default();
    d_settings.capture.target = CaptureTarget::Monitor {
        monitor_id: primary.monitor_id.clone(),
        name: primary.name.clone(),
    };
    d_settings.storage.output_dir = daemon_dir.clone();
    d_settings.storage.temp_dir = core_types::ikoch_temp_dir();
    d_settings.storage.max_storage_gb = Some(50);

    let (daemon, daemon_rx) = RecorderDaemon::start(d_settings)?;
    daemon.send_command(RecorderCommand::StartBuffering)?;

    let d_deadline = Instant::now() + Duration::from_secs(7);
    loop {
        thread::sleep(Duration::from_millis(150));
        let snap = daemon.state_snapshot();
        if snap.buffer_fill_sec >= 0.85 {
            println!(
                "  Daemon buffer primed: {:.2}s ({} bytes)",
                snap.buffer_fill_sec, snap.buffer_bytes
            );
            break;
        }
        if Instant::now() >= d_deadline {
            return Err("Timed out waiting for RecorderDaemon buffer to prime".into());
        }
    }

    daemon.send_command(RecorderCommand::AddBookmark {
        label: Some("Ace Bookmark (F8)".to_string()),
    })?;
    daemon.send_command(RecorderCommand::PlayTestSound {
        kind: SoundNotificationKind::MarkerAdded,
    })?;
    thread::sleep(Duration::from_millis(150));
    daemon.send_command(RecorderCommand::SaveReplayClip)?;

    let saved_meta = {
        let wait_clip = Instant::now() + Duration::from_secs(5);
        let mut found: Option<ClipMetadata> = None;
        while Instant::now() < wait_clip {
            if let Ok(RecorderEvent::ClipSaved(meta)) =
                daemon_rx.recv_timeout(Duration::from_millis(100))
            {
                found = Some(*meta);
                break;
            }
        }
        found.ok_or("Timed out waiting for RecorderEvent::ClipSaved from daemon")?
    };

    let sidecar_path = ClipMetadata::sidecar_path_for(&saved_meta.file_path);
    assert!(
        !ClipMetadata::legacy_sidecar_path_for(&saved_meta.file_path).exists(),
        "daemon output dir must not contain adjacent .json sidecar"
    );
    assert!(
        !daemon_dir.join(".tmp").exists(),
        "daemon output dir must not contain .tmp folder"
    );
    let loaded_meta = load_clip_metadata(&sidecar_path)?;
    assert!(
        loaded_meta
            .markers
            .iter()
            .any(|m| m.label == "Ace Bookmark (F8)"),
        "Expected F8 bookmark inside JSON sidecar"
    );
    let mf_daemon = verify_mp4_audio_with_mf_source_reader(&saved_meta.file_path)?;
    println!(
        "  Daemon clip + sidecar verified: '{}' + '{}' | markers={} | MF audio={} frames @ {} Hz",
        saved_meta.file_path.display(),
        sidecar_path.display(),
        loaded_meta.markers.len(),
        mf_daemon.decoded_frames,
        mf_daemon.sample_rate_hz
    );

    // Privacy Mode must immediately pause capture & wipe replay buffer tail.
    daemon.send_command(RecorderCommand::TogglePrivacyMode)?;
    let p_deadline = Instant::now() + Duration::from_secs(3);
    loop {
        thread::sleep(Duration::from_millis(50));
        let snap = daemon.state_snapshot();
        if snap.privacy_mode && snap.buffer_bytes == 0 {
            println!("  Privacy Mode verified: privacy_mode=true, buffer_bytes=0 (wiped)");
            break;
        }
        if Instant::now() >= p_deadline {
            return Err("Privacy Mode failed to wipe replay buffer".into());
        }
    }
    daemon.send_command(RecorderCommand::TogglePrivacyMode)?;
    let _quota = StorageQuotaManager::enforce_quota(&daemon_dir, Some(50), Some(30))?;
    daemon.shutdown();
    println!("=== Stage 3 + Stage 4 Self-Test PASSED ===");
    Ok(())
}

fn run_interactive() -> Result<(), Box<dyn std::error::Error>> {
    let render_devs = enumerate_render_devices()?;
    let capture_devs = enumerate_capture_devices()?;
    println!(
        "Audio devices: {} output(s), {} microphone(s)",
        render_devs.len(),
        capture_devs.len()
    );

    let monitors = enumerate_monitors()?;
    let primary = monitors
        .iter()
        .find(|m| m.is_primary)
        .or_else(|| monitors.first())
        .ok_or("No monitor available")?;

    let initial_target = CaptureTarget::Monitor {
        monitor_id: primary.monitor_id.clone(),
        name: primary.name.clone(),
    };
    let ctrl = PipelineController::start(initial_target, AudioSettings::default())?;
    fs::create_dir_all("clips")?;

    println!("\n=== RustClip Stage 3 CLI Test Bench (Video + Dual AAC Audio) ===");
    println!("Commands:");
    println!("  [Enter] or c : Save 15s replay clip (with Track 1 Game + Track 2 Mic)");
    println!("  s            : Start / Stop full session recording");
    println!("  m            : List & switch Monitor target");
    println!("  w            : List & switch SpecificWindow target");
    println!("  a            : Switch to AutoGame target");
    println!("  o            : List & switch Output Loopback audio device");
    println!("  k            : List & switch Microphone device (or mute toggle: 'mute')");
    println!("  mute         : Toggle microphone mute");
    println!("  ptt          : Toggle Microphone AlwaysOn <-> Push-to-Talk (V)");
    println!("  i            : Print live stats & RAM usage");
    println!("  q            : Quit\n");

    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();
    let mut session_active = false;
    let mut mic_muted = false;
    let mut ptt_enabled = false;
    let mut clip_counter = 1u32;
    let mut session_counter = 1u32;

    loop {
        print!("> ");
        io::stdout().flush()?;
        let Some(Ok(line)) = lines.next() else {
            break;
        };
        let cmd = line.trim();
        match cmd {
            "" | "c" => {
                let path = PathBuf::from(format!("clips/clip_{clip_counter:03}.mp4"));
                match ctrl.save_clip(path) {
                    Ok(sum) => {
                        println!(
                            "Saved clip: {} ({:.2}s, {} bytes)",
                            sum.path.display(),
                            sum.duration_sec,
                            sum.bytes_written
                        );
                        clip_counter += 1;
                    }
                    Err(err) => println!("Save clip failed: {err}"),
                }
            }
            "s" => {
                if !session_active {
                    let path = PathBuf::from(format!("clips/session_{session_counter:03}.mp4"));
                    match ctrl.start_session(path.clone()) {
                        Ok(()) => {
                            session_active = true;
                            println!("Started session -> {}", path.display());
                        }
                        Err(err) => println!("Start session failed: {err}"),
                    }
                } else {
                    match ctrl.stop_session(true) {
                        Ok(sum) => {
                            session_active = false;
                            session_counter += 1;
                            println!(
                                "Stopped session: {} ({:.2}s, {} bytes)",
                                sum.path.display(),
                                sum.duration_sec,
                                sum.bytes_written
                            );
                        }
                        Err(err) => println!("Stop session failed: {err}"),
                    }
                }
            }
            "m" => {
                let mons = enumerate_monitors()?;
                for (idx, m) in mons.iter().enumerate() {
                    println!(
                        "  [{idx}] {} ({}x{}, primary={})",
                        m.name, m.width, m.height, m.is_primary
                    );
                }
                print!("Monitor index: ");
                io::stdout().flush()?;
                if let Some(Ok(sel)) = lines.next() {
                    if let Ok(idx) = sel.trim().parse::<usize>() {
                        if let Some(m) = mons.get(idx) {
                            ctrl.switch_target(CaptureTarget::Monitor {
                                monitor_id: m.monitor_id.clone(),
                                name: m.name.clone(),
                            })?;
                        }
                    }
                }
            }
            "w" => {
                let wins = enumerate_windows()?;
                for (idx, w) in wins.iter().take(20).enumerate() {
                    println!(
                        "  [{idx}] [{}] {} ({}x{})",
                        w.process_name, w.title, w.width, w.height
                    );
                }
                print!("Window index: ");
                io::stdout().flush()?;
                if let Some(Ok(sel)) = lines.next() {
                    if let Ok(idx) = sel.trim().parse::<usize>() {
                        if let Some(w) = wins.get(idx) {
                            ctrl.switch_target(CaptureTarget::SpecificWindow {
                                hwnd: w.hwnd,
                                process_name: w.process_name.clone(),
                                title: w.title.clone(),
                            })?;
                        }
                    }
                }
            }
            "a" => {
                ctrl.switch_target(CaptureTarget::AutoGame)?;
                println!("Switched to CaptureTarget::AutoGame");
            }
            "o" => {
                let outs = enumerate_render_devices()?;
                println!("  [d] Default Output Device");
                for (idx, d) in outs.iter().enumerate() {
                    println!("  [{idx}] {} (default={})", d.name, d.is_default);
                }
                print!("Output device index or 'd': ");
                io::stdout().flush()?;
                if let Some(Ok(sel)) = lines.next() {
                    let s = sel.trim();
                    if s == "d" {
                        ctrl.audio.set_output_mode(AudioOutputMode::DefaultDevice);
                    } else if let Ok(idx) = s.parse::<usize>() {
                        if let Some(d) = outs.get(idx) {
                            ctrl.audio
                                .set_output_mode(AudioOutputMode::SpecificDevice(d.id.clone()));
                        }
                    }
                }
            }
            "k" => {
                let mics = enumerate_capture_devices()?;
                println!("  [d] Default Microphone | [n] None (Disable)");
                for (idx, d) in mics.iter().enumerate() {
                    println!("  [{idx}] {} (default={})", d.name, d.is_default);
                }
                print!("Mic index, 'd', or 'n': ");
                io::stdout().flush()?;
                if let Some(Ok(sel)) = lines.next() {
                    let s = sel.trim();
                    match s {
                        "d" => ctrl.audio.set_mic_device(MicDeviceConfig::Default),
                        "n" => ctrl.audio.set_mic_device(MicDeviceConfig::None),
                        other => {
                            if let Ok(idx) = other.parse::<usize>() {
                                if let Some(d) = mics.get(idx) {
                                    ctrl.audio
                                        .set_mic_device(MicDeviceConfig::SpecificDevice(d.id.clone()));
                                }
                            }
                        }
                    }
                }
            }
            "mute" => {
                mic_muted = !mic_muted;
                ctrl.audio.set_mic_muted(mic_muted);
                println!("Microphone muted = {mic_muted}");
            }
            "ptt" => {
                ptt_enabled = !ptt_enabled;
                if ptt_enabled {
                    ctrl.audio.set_mic_activation_mode(MicActivationMode::PushToTalk {
                        key: "V".to_string(),
                        release_delay_ms: 150,
                    });
                    println!("Microphone set to Push-to-Talk (hold V)");
                } else {
                    ctrl.audio.set_mic_activation_mode(MicActivationMode::AlwaysOn);
                    println!("Microphone set to AlwaysOn");
                }
            }
            "i" => {
                if let Ok((fill, bytes, gops, drops, sess, epoch)) = ctrl.stats() {
                    let ws_mb = process_working_set_mb();
                    let buf_mb = (bytes as f64) / (1024.0 * 1024.0);
                    println!(
                        "Stats [epoch={epoch}]: fill={fill:.2}s | gops={gops} | buffer={buf_mb:.2} MiB | RAM total={ws_mb:.2} MiB (excl buf {:.2} MiB) | session={sess} | disk_drops={drops}",
                        (ws_mb - buf_mb).max(0.0)
                    );
                }
            }
            "q" => break,
            other => println!("Unknown command '{other}'"),
        }
    }

    ctrl.shutdown();
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();
    if let Some(pos) = args.iter().position(|a| a == "--migrate-cache") {
        let dir = args
            .get(pos + 1)
            .ok_or("Expected directory path after --migrate-cache")?;
        recorder_daemon::migrate_legacy_files_in_output_dir(Path::new(dir));
        let _ = recorder_daemon::list_clip_metadata(Path::new(dir));
        recorder_daemon::cleanup_temp_dir(&core_types::ikoch_temp_dir());
        println!("Migrated legacy sidecars/previews/tmp from '{}'", dir);
        return Ok(());
    }
    let self_test = args.iter().any(|arg| arg == "--self-test");
    if self_test {
        run_self_test()
    } else {
        run_interactive()
    }
}
