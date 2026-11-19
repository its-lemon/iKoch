//! GOP-aware replay ring buffer keeping compressed video & audio packets bounded by
//! target duration and RAM cap, evicting strictly along IDR keyframe boundaries.

use core_types::{MediaPacket, MuxError, StreamHeaders, TrackKind};
use std::collections::VecDeque;

#[derive(Debug, Clone)]
struct GopSegment {
    start_pts_us: i64,
    end_pts_us: i64,
    packets: Vec<MediaPacket>,
    bytes: usize,
}

/// Compressed circular buffer preserving stream headers (`SPS`/`PPS`/`ASC`) and evicting
/// expired data strictly one complete GOP at a time.
pub struct GopRingBuffer {
    target_duration_us: i64,
    max_bytes: usize,
    headers: Option<StreamHeaders>,
    gops: VecDeque<GopSegment>,
    total_bytes: usize,
    latest_pts_us: Option<i64>,
}

impl GopRingBuffer {
    /// Creates a new GOP ring buffer with `duration_secs` window and `max_bytes` RAM cap.
    #[must_use]
    pub fn new(duration_secs: u32, max_bytes: usize) -> Self {
        let clamped_secs = duration_secs.clamp(15, 300);
        Self {
            target_duration_us: i64::from(clamped_secs) * 1_000_000,
            max_bytes: max_bytes.max(4 * 1024 * 1024),
            headers: None,
            gops: VecDeque::new(),
            total_bytes: 0,
            latest_pts_us: None,
        }
    }

    /// Updates retention bounds (e.g. after settings change) and evicts excess GOPs immediately.
    pub fn update_limits(&mut self, duration_secs: u32, max_bytes: usize) {
        let clamped_secs = duration_secs.clamp(15, 300);
        self.target_duration_us = i64::from(clamped_secs) * 1_000_000;
        self.max_bytes = max_bytes.max(4 * 1024 * 1024);
        self.evict_expired();
    }

    /// Stores or updates stream initialization headers (`avcC`/`hvcC` and AAC `AudioSpecificConfig`).
    pub fn set_headers(&mut self, headers: StreamHeaders) {
        self.headers = Some(headers);
    }

    /// Returns a reference to the current stream headers, if initialized.
    #[must_use]
    pub fn headers(&self) -> Option<&StreamHeaders> {
        self.headers.as_ref()
    }

    /// Clears buffered GOPs and video header when switching `CaptureTarget` or changing
    /// resolution while preserving active audio stream headers.
    pub fn reset_for_new_source(&mut self) {
        self.headers = None;
        self.gops.clear();
        self.total_bytes = 0;
        self.latest_pts_us = None;
    }

    /// Inserts one compressed packet (`Video`, `AudioGame`, or `AudioMic`).
    ///
    /// Non-keyframe packets arriving before the very first IDR keyframe are discarded.
    pub fn push_packet(&mut self, packet: MediaPacket) {
        let pkt_bytes = packet.size_bytes();
        let pkt_end = packet.pts_us.saturating_add(i64::from(packet.duration_us));

        if packet.track == TrackKind::Video && packet.is_keyframe {
            self.gops.push_back(GopSegment {
                start_pts_us: packet.pts_us,
                end_pts_us: pkt_end,
                packets: vec![packet],
                bytes: pkt_bytes,
            });
            self.total_bytes = self.total_bytes.saturating_add(pkt_bytes);
            self.latest_pts_us = Some(
                self.latest_pts_us
                    .map_or(pkt_end, |prev| prev.max(pkt_end)),
            );
            self.evict_expired();
            return;
        }

        // Discard leading P-frames or audio before the first complete IDR keyframe arrives.
        let Some(last_gop) = self.gops.back_mut() else {
            return;
        };

        last_gop.end_pts_us = last_gop.end_pts_us.max(pkt_end);
        last_gop.bytes = last_gop.bytes.saturating_add(pkt_bytes);
        last_gop.packets.push(packet);
        self.total_bytes = self.total_bytes.saturating_add(pkt_bytes);
        self.latest_pts_us = Some(
            self.latest_pts_us
                .map_or(pkt_end, |prev| prev.max(pkt_end)),
        );
        self.evict_expired();
    }

    /// Returns currently buffered payload size in bytes.
    #[must_use]
    pub fn buffered_bytes(&self) -> usize {
        self.total_bytes
    }

    /// Returns the number of complete GOP segments currently retained.
    #[must_use]
    pub fn gop_count(&self) -> usize {
        self.gops.len()
    }

    /// Returns the currently buffered time span in seconds.
    #[must_use]
    pub fn buffered_duration_sec(&self) -> f64 {
        let Some(first) = self.gops.front() else {
            return 0.0;
        };
        let Some(latest) = self.latest_pts_us else {
            return 0.0;
        };
        let span_us = latest.saturating_sub(first.start_pts_us).max(0);
        (span_us as f64) / 1_000_000.0
    }

    /// Snapshots all retained packets starting cleanly at the oldest IDR keyframe with timestamps
    /// rebased so the first IDR frame sits at `t = 0.0`.
    pub fn export_rebased_packets(&self) -> Result<(StreamHeaders, Vec<MediaPacket>), MuxError> {
        let headers = self
            .headers
            .clone()
            .ok_or(MuxError::MissingStreamHeaders)?;
        let first_gop = self.gops.front().ok_or(MuxError::NoKeyframeInBuffer)?;
        let base_pts_us = first_gop.start_pts_us;

        let total_packets: usize = self.gops.iter().map(|g| g.packets.len()).sum();
        let mut out = Vec::with_capacity(total_packets);

        for gop in &self.gops {
            for pkt in &gop.packets {
                // Include the AAC frame covering `base_pts_us` (`[pts, pts + 21.3ms)`) and clamp
                // to `t = 0` so startup A/V skew stays well below 20 ms without negative timestamps.
                if pkt.track.is_audio() && pkt.pts_us < base_pts_us.saturating_sub(21_333) {
                    continue;
                }
                let mut rebased = pkt.with_rebased_timestamps(base_pts_us);
                if rebased.track.is_audio() && rebased.pts_us < 0 {
                    rebased.pts_us = 0;
                    rebased.dts_us = 0;
                }
                out.push(rebased);
            }
        }

        Ok((headers, out))
    }

    fn evict_expired(&mut self) {
        let Some(latest) = self.latest_pts_us else {
            return;
        };

        while self.gops.len() > 1 {
            let exceeds_bytes = self.total_bytes > self.max_bytes;
            let exceeds_duration = self
                .gops
                .get(1)
                .is_some_and(|next_gop| latest.saturating_sub(next_gop.start_pts_us) >= self.target_duration_us);

            if !exceeds_bytes && !exceeds_duration {
                break;
            }

            if let Some(evicted) = self.gops.pop_front() {
                self.total_bytes = self.total_bytes.saturating_sub(evicted.bytes);
            }
        }
    }
}
