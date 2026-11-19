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
        self.headers = Some(headers.clone());
        self.ring_buffer.lock().set_headers(headers);
    }

    /// Registers or refreshes the video stream header independently of audio headers.
    pub fn update_video_header(&mut self, video: VideoStreamHeader) {
        self.video_header = Some(video);
        self.sync_combined_headers();
    }

    /// Registers or refreshes an audio track header (`AudioMixed`, `AudioGame`, `AudioMic`, `AudioSecondary`).
    pub fn update_audio_header(&mut self, audio: AudioStreamHeader) {
        match audio.track {
            TrackKind::AudioMixed => self.audio_mixed_header = Some(audio),
            TrackKind::AudioGame => self.audio_game_header = Some(audio),
            TrackKind::AudioMic => self.audio_mic_header = Some(audio),
            TrackKind::AudioSecondary => self.audio_secondary_header = Some(audio),
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

    /// Clears cached audio track headers before restarting the audio pipeline on bitrate/track changes.
    pub fn clear_audio_headers(&mut self) {
        self.audio_mixed_header = None;
        self.audio_game_header = None;
        self.audio_mic_header = None;
        self.audio_secondary_header = None;
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
        let bytes_written = write_faststart_mp4(output_path, &headers, &packets)?;

        info!(
            "Saved replay clip '{}' ({:.2}s, {} packets, {} bytes)",
            output_path.display(),
            duration_sec,
            packet_count,
            bytes_written
        );

        Ok(SavedMediaSummary {
            path: output_path.to_path_buf(),
            bytes_written,
            duration_sec,
            packet_count,
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
