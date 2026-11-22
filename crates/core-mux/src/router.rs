//! Unified PacketRouter routing compressed `MediaPacket`s (`Arc<[u8]>`) to both the
//! GOP Ring Buffer and an optional Crash-Safe `Fmp4DiskWriter` with bounded backpressure.

use crate::mp4_writer::{write_faststart_mp4, Fmp4DiskWriter};
use crate::ring_buffer::GopRingBuffer;
use core_types::{
    AudioStreamHeader, MediaPacket, MuxError, StreamHeaders, TrackKind, VideoStreamHeader,
    DISK_MUX_CHANNEL_CAPACITY,
};
use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use tracing::{info, warn};

/// Summary metadata returned after saving a replay clip or finalizing a session recording.
#[derive(Debug, Clone)]
pub struct SavedMediaSummary {
    pub path: PathBuf,
    pub bytes_written: u64,
    pub duration_sec: f64,
    pub packet_count: usize,
    pub video_pkts: usize,
    pub mixed_pkts: usize,
    pub game_pkts: usize,
    pub mic_pkts: usize,
    pub sec_pkts: usize,
}

enum DiskTaskCmd {
    StartSession {
        path: PathBuf,
        headers: Box<StreamHeaders>,
        reply: Sender<Result<(), MuxError>>,
    },
    Packet(MediaPacket),
    StopSession {
        remux_faststart: bool,
        reply: Sender<Result<SavedMediaSummary, MuxError>>,
    },
    Shutdown,
}

/// Routes encoded packets simultaneously into the GOP Replay Ring Buffer and an active
/// disk session writer without duplicating encoding or leaking memory on slow disks.
pub struct PacketRouter {
    ring_buffer: Arc<Mutex<GopRingBuffer>>,
    video_header: Option<VideoStreamHeader>,
    audio_mixed_header: Option<AudioStreamHeader>,
    audio_game_header: Option<AudioStreamHeader>,
    audio_mic_header: Option<AudioStreamHeader>,
    audio_secondary_header: Option<AudioStreamHeader>,
    app_audio_headers: Vec<(u8, String, AudioStreamHeader)>,
    app_audio_labels: Vec<(u8, String)>,
    headers: Option<StreamHeaders>,
    disk_tx: Sender<DiskTaskCmd>,
    disk_thread: Option<JoinHandle<()>>,
    session_active: bool,
    dropped_disk_packets: Arc<AtomicU64>,
    session_start_pts_us: Option<i64>,
    last_session_pts_us: Option<i64>,
    session_packet_count: usize,
}

impl PacketRouter {
    /// Initializes a new [`PacketRouter`] with the specified replay buffer bounds and a
    /// dedicated bounded disk-writer thread (`DISK_MUX_CHANNEL_CAPACITY = 512`).
    pub fn new(replay_duration_secs: u32, replay_max_bytes: usize) -> Result<Self, MuxError> {
        let ring_buffer = Arc::new(Mutex::new(GopRingBuffer::new(
            replay_duration_secs,
            replay_max_bytes,
        )));
        let (disk_tx, disk_rx) = bounded::<DiskTaskCmd>(DISK_MUX_CHANNEL_CAPACITY);
        let dropped_disk_packets = Arc::new(AtomicU64::new(0));

        let disk_thread = thread::Builder::new()
            .name("rustclip-disk-mux".to_string())
            .spawn(move || run_disk_writer_loop(disk_rx))?;

        Ok(Self {
            ring_buffer,
            video_header: None,
            audio_mixed_header: None,
            audio_game_header: None,
            audio_mic_header: None,
            audio_secondary_header: None,
            app_audio_headers: Vec::new(),
            app_audio_labels: Vec::new(),
            headers: None,
            disk_tx,
            disk_thread: Some(disk_thread),
            session_active: false,
            dropped_disk_packets,
            session_start_pts_us: None,
            last_session_pts_us: None,
            session_packet_count: 0,
        })
    }

    /// Updates stream initialization headers in both the router and the replay ring buffer.
    pub fn set_stream_headers(&mut self, headers: StreamHeaders) {
        self.video_header = Some(headers.video.clone());
        self.audio_mixed_header = headers.audio_mixed.clone();
        self.audio_game_header = headers.audio_game.clone();
        self.audio_mic_header = headers.audio_mic.clone();
        self.audio_secondary_header = headers.audio_secondary.clone();
        self.app_audio_headers = headers.app_audio.clone();
        self.headers = Some(headers.clone());
        self.ring_buffer.lock().set_headers(headers);
    }

    /// Sets human-readable labels for dynamic per-application audio slots.
    pub fn set_app_audio_labels(&mut self, labels: Vec<(u8, String)>) {
        self.app_audio_labels = labels;
        for (idx, lbl, _) in &mut self.app_audio_headers {
            if let Some((_, new_lbl)) = self.app_audio_labels.iter().find(|(s, _)| s == idx) {
                *lbl = new_lbl.clone();
            }
        }
        self.sync_combined_headers();
    }

    /// Registers or refreshes the video stream header independently of audio headers.
    pub fn update_video_header(&mut self, video: VideoStreamHeader) {
        self.video_header = Some(video);
        self.sync_combined_headers();
    }

    /// Registers or refreshes an audio track header (`AudioMixed`, `AudioGame`, `AudioMic`, `AudioSecondary`, `AudioApp`).
    pub fn update_audio_header(&mut self, audio: AudioStreamHeader) {
        match audio.track {
            TrackKind::AudioMixed => self.audio_mixed_header = Some(audio),
            TrackKind::AudioGame => self.audio_game_header = Some(audio),
            TrackKind::AudioMic => self.audio_mic_header = Some(audio),
            TrackKind::AudioSecondary => self.audio_secondary_header = Some(audio),
            TrackKind::AudioApp(idx) => {
                let label = self
                    .app_audio_labels
                    .iter()
                    .find(|(s, _)| *s == idx)
                    .map(|(_, l)| l.clone())
                    .unwrap_or_else(|| format!("App {}", idx + 1));
                if let Some(existing) = self.app_audio_headers.iter_mut().find(|(s, _, _)| *s == idx)
                {
                    existing.1 = label;
                    existing.2 = audio;
                } else {
                    self.app_audio_headers.push((idx, label, audio));
                    self.app_audio_headers.sort_by_key(|(s, _, _)| *s);
                }
            }
            TrackKind::Video => return,
        }
        self.sync_combined_headers();
    }

    fn sync_combined_headers(&mut self) {
        if let Some(video) = self.video_header.clone() {
            let combined = StreamHeaders {
                video,
                audio_mixed: self.audio_mixed_header.clone(),
                audio_game: self.audio_game_header.clone(),
                audio_mic: self.audio_mic_header.clone(),
                audio_secondary: self.audio_secondary_header.clone(),
                app_audio: self.app_audio_headers.clone(),
            };
            self.headers = Some(combined.clone());
            self.ring_buffer.lock().set_headers(combined);
        }
    }

    /// Updates replay buffer retention limits dynamically.
    pub fn update_replay_limits(&self, duration_secs: u32, max_bytes: usize) {
        self.ring_buffer
            .lock()
            .update_limits(duration_secs, max_bytes);
    }

    /// Resets the GOP ring buffer when switching `CaptureTarget` or changing resolution while
    /// keeping active audio headers intact.
    pub fn on_source_reset(&mut self) {
        self.video_header = None;
        self.headers = None;
        self.ring_buffer.lock().reset_for_new_source();
    }

    /// Clears buffered replay GOPs while preserving stream headers (`reset_buffer_on_clip`).
    pub fn clear_replay_buffer(&mut self) {
        self.ring_buffer.lock().clear_keep_headers();
    }

    /// Clears cached audio track headers before restarting the audio pipeline on bitrate/track changes.
    pub fn clear_audio_headers(&mut self) {
        self.audio_mixed_header = None;
        self.audio_game_header = None;
        self.audio_mic_header = None;
        self.audio_secondary_header = None;
        self.ring_buffer.lock().clear_cached_audio_headers();
        self.sync_combined_headers();
    }

    /// Routes a single compressed packet (`Arc<[u8]>`) to the GOP ring buffer and (if active)
    /// to the bounded disk mux channel using non-blocking backpressure.
    pub fn push_packet(&mut self, packet: MediaPacket) {
        if self.session_active {
            if self.session_start_pts_us.is_none()
                && packet.track == TrackKind::Video
                && packet.is_keyframe
            {
                self.session_start_pts_us = Some(packet.pts_us);
            }
            if self.session_start_pts_us.is_some() {
                self.last_session_pts_us = Some(
                    packet
                        .pts_us
                        .saturating_add(i64::from(packet.duration_us)),
                );
                self.session_packet_count = self.session_packet_count.saturating_add(1);
            }
            match self.disk_tx.try_send(DiskTaskCmd::Packet(packet.clone())) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    // Controlled drop under disk I/O stall to prevent RAM leak.
                    self.dropped_disk_packets.fetch_add(1, Ordering::Relaxed);
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.session_active = false;
                }
            }
        }

        self.ring_buffer.lock().push_packet(packet);
    }

    /// Snapshots the current replay ring buffer (`t = 0.0` at oldest IDR) and writes a `faststart` MP4.
    pub fn save_replay_clip(&self, output_path: &Path) -> Result<SavedMediaSummary, MuxError> {
        let (headers, packets) = self.ring_buffer.lock().export_rebased_packets()?;
        let duration_sec = packets
            .iter()
            .map(|p| p.pts_us.saturating_add(i64::from(p.duration_us)))
            .max()
            .map_or(0.0, |end_us| (end_us.max(0) as f64) / 1_000_000.0);
        let packet_count = packets.len();
        let mut video_pkts = 0usize;
        let mut mixed_pkts = 0usize;
        let mut game_pkts = 0usize;
        let mut mic_pkts = 0usize;
        let mut sec_pkts = 0usize;
        for p in &packets {
            match p.track {
                TrackKind::Video => video_pkts += 1,
                TrackKind::AudioMixed => mixed_pkts += 1,
                TrackKind::AudioGame => game_pkts += 1,
                TrackKind::AudioMic => mic_pkts += 1,
                TrackKind::AudioSecondary | TrackKind::AudioApp(_) => sec_pkts += 1,
            }
        }

        let bytes_written = write_faststart_mp4(output_path, &headers, &packets)?;

        info!(
            "Saved replay clip '{}' ({:.2}s, {} bytes) -> video_pkts={}, mixed_pkts={}, game_pkts={}, mic_pkts={}, sec_pkts={}",
            output_path.display(),
            duration_sec,
            bytes_written,
            video_pkts,
            mixed_pkts,
            game_pkts,
            mic_pkts,
            sec_pkts
        );
        if mixed_pkts == 0 {
            warn!(
                "Saved replay clip '{}' has NO audio (mixed_pkts=0)! headers_audio_mixed={}, headers_audio_game={}, headers_audio_mic={}, video_pkts={}",
                output_path.display(),
                headers.audio_mixed.is_some(),
                headers.audio_game.is_some(),
                headers.audio_mic.is_some(),
                video_pkts
            );
        }

        Ok(SavedMediaSummary {
            path: output_path.to_path_buf(),
            bytes_written,
            duration_sec,
            packet_count,
            video_pkts,
            mixed_pkts,
            game_pkts,
            mic_pkts,
            sec_pkts,
        })
    }

    /// Starts a crash-safe fragmented MP4 full session recording on the disk writer thread.
    pub fn start_session(&mut self, output_path: &Path) -> Result<(), MuxError> {
        if self.session_active {
            return Err(MuxError::InvalidState(
                "Session recording is already active".to_string(),
            ));
        }
        let headers = self
            .headers
            .clone()
            .ok_or(MuxError::MissingStreamHeaders)?;

        let (reply_tx, reply_rx) = bounded(1);
        self.disk_tx
            .send(DiskTaskCmd::StartSession {
                path: output_path.to_path_buf(),
                headers: Box::new(headers),
                reply: reply_tx,
            })
            .map_err(|_| MuxError::InvalidState("Disk writer thread terminated".to_string()))?;

        reply_rx
            .recv()
            .map_err(|_| MuxError::InvalidState("Disk writer failed to reply".to_string()))??;

        self.session_active = true;
        self.session_start_pts_us = None;
        self.last_session_pts_us = None;
        self.session_packet_count = 0;
        Ok(())
    }

    /// Stops the active session recording and optionally remuxes the `.fmp4` into `faststart` MP4.
    pub fn stop_session(&mut self, remux_faststart: bool) -> Result<SavedMediaSummary, MuxError> {
        if !self.session_active {
            return Err(MuxError::InvalidState(
                "No active session recording".to_string(),
            ));
        }

        let (reply_tx, reply_rx) = bounded(1);
        self.disk_tx
            .send(DiskTaskCmd::StopSession {
                remux_faststart,
                reply: reply_tx,
            })
            .map_err(|_| MuxError::InvalidState("Disk writer thread terminated".to_string()))?;

        let mut summary = reply_rx
            .recv()
            .map_err(|_| MuxError::InvalidState("Disk writer failed to reply".to_string()))??;

        if let (Some(start), Some(end)) = (self.session_start_pts_us, self.last_session_pts_us) {
            summary.duration_sec = (end.saturating_sub(start).max(0) as f64) / 1_000_000.0;
        }
        summary.packet_count = self.session_packet_count;

        self.session_active = false;
        self.session_start_pts_us = None;
        self.last_session_pts_us = None;
        self.session_packet_count = 0;
        Ok(summary)
    }

    /// Returns current ring buffer fill duration, bytes, and dropped disk packet counter.
    #[must_use]
    pub fn stats(&self) -> (f64, usize, usize, u64, bool) {
        let rb = self.ring_buffer.lock();
        (
            rb.buffered_duration_sec(),
            rb.buffered_bytes(),
            rb.gop_count(),
            self.dropped_disk_packets.load(Ordering::Relaxed),
            self.session_active,
        )
    }
}

impl Drop for PacketRouter {
    fn drop(&mut self) {
        let _ = self.disk_tx.send(DiskTaskCmd::Shutdown);
        if let Some(handle) = self.disk_thread.take() {
            let _ = handle.join();
        }
    }
}

fn run_disk_writer_loop(rx: Receiver<DiskTaskCmd>) {
    let mut writer: Option<Fmp4DiskWriter> = None;

    while let Ok(cmd) = rx.recv() {
        match cmd {
            DiskTaskCmd::StartSession {
                path,
                headers,
                reply,
            } => {
                let res = Fmp4DiskWriter::create(&path, *headers).map(|w| {
                    writer = Some(w);
                });
                let _ = reply.send(res);
            }
            DiskTaskCmd::Packet(pkt) => {
                if let Some(w) = writer.as_mut() {
                    if let Err(err) = w.push_packet(pkt) {
                        warn!("fMP4 disk writer error: {err}");
                    }
                }
            }
            DiskTaskCmd::StopSession {
                remux_faststart,
                reply,
            } => {
                let res = match writer.take() {
                    Some(w) => {
                        let out_path = w
                            .temp_fmp4_path()
                            .to_string_lossy()
                            .trim_end_matches(".fmp4")
                            .to_string();
                        w.finalize(remux_faststart).map(|bytes| SavedMediaSummary {
                            path: PathBuf::from(out_path),
                            bytes_written: bytes,
                            duration_sec: 0.0,
                            packet_count: 0,
                            video_pkts: 0,
                            mixed_pkts: 0,
                            game_pkts: 0,
                            mic_pkts: 0,
                            sec_pkts: 0,
                        })
                    }
                    None => Err(MuxError::InvalidState(
                        "Disk writer had no open session".to_string(),
                    )),
                };
                let _ = reply.send(res);
            }
            DiskTaskCmd::Shutdown => {
                if let Some(w) = writer.take() {
                    let _ = w.finalize(false);
                }
                break;
            }
        }
    }
}
