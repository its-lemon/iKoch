//! ISO BMFF (`MP4` faststart and crash-safe `fMP4` fragmented) writers.

use core_types::{
    us_to_timescale, MediaPacket, MuxError, StreamHeaders, TrackKind, VideoCodec,
    VIDEO_TIMESCALE_90KHZ,
};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use tracing::debug;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Mp4TrackOrder {
    Video,
    AudioMixed,
    AudioGame,
    AudioMic,
    AudioSecondary,
    AudioApp(u8),
}

impl From<TrackKind> for Mp4TrackOrder {
    fn from(kind: TrackKind) -> Self {
        match kind {
            TrackKind::Video => Self::Video,
            TrackKind::AudioMixed => Self::AudioMixed,
            TrackKind::AudioGame => Self::AudioGame,
            TrackKind::AudioMic => Self::AudioMic,
            TrackKind::AudioSecondary => Self::AudioSecondary,
            TrackKind::AudioApp(idx) => Self::AudioApp(idx),
        }
    }
}

impl Mp4TrackOrder {
    const fn to_track_kind(self) -> TrackKind {
        match self {
            Self::Video => TrackKind::Video,
            Self::AudioMixed => TrackKind::AudioMixed,
            Self::AudioGame => TrackKind::AudioGame,
            Self::AudioMic => TrackKind::AudioMic,
            Self::AudioSecondary => TrackKind::AudioSecondary,
            Self::AudioApp(idx) => TrackKind::AudioApp(idx),
        }
    }
}

struct PreparedSample<'a> {
    duration_ticks: u32,
    size: u32,
    is_keyframe: bool,
    payload: &'a [u8],
}

struct PreparedChunk<'a> {
    track: TrackKind,
    rel_offset_in_mdat: u64,
    sample_count: u32,
    payloads: Vec<&'a [u8]>,
}

struct TrackBuildState<'a> {
    kind: TrackKind,
    timescale: u32,
    total_duration_ticks: u64,
    samples: Vec<PreparedSample<'a>>,
    chunk_offsets: Vec<u64>,
    chunk_sample_counts: Vec<u32>,
}

/// Writes a progressive `faststart` MP4 (`ftyp` + `moov` BEFORE `mdat`) from in-memory packets
/// (e.g. a replay buffer clip) with zero extra heap copy of the `mdat` payload.
pub fn write_faststart_mp4(
    path: &Path,
    headers: &StreamHeaders,
    packets: &[MediaPacket],
) -> Result<u64, MuxError> {
    if packets.is_empty() {
        return Err(MuxError::NoKeyframeInBuffer);
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let mut per_track: BTreeMap<Mp4TrackOrder, Vec<&MediaPacket>> = BTreeMap::new();
    for pkt in packets {
        per_track.entry(pkt.track.into()).or_default().push(pkt);
    }

    let mut track_states: BTreeMap<Mp4TrackOrder, TrackBuildState<'_>> = BTreeMap::new();
    for (order, list) in &per_track {
        let kind = order.to_track_kind();
        let timescale = track_timescale(kind, headers);
        let state = build_track_state(kind, timescale, list);
        track_states.insert(*order, state);
    }

    // Interleave chunks following the chronological packet stream so players read sequentially.
    let mut chunks: Vec<PreparedChunk<'_>> = Vec::new();
    let mut mdat_body_len: u64 = 0;

    // Map (TrackKind, packet index in packets) -> chunk grouping by contiguous runs of same track.
    let mut current_track: Option<TrackKind> = None;
    let mut current_payloads: Vec<&[u8]> = Vec::new();
    let mut current_start_offset: u64 = 0;

    for pkt in packets {
        let should_flush = current_track.is_some_and(|t| t != pkt.track)
            || current_payloads.len() >= 120;
        if should_flush {
            if let Some(t) = current_track.take() {
                let sample_count = current_payloads.len() as u32;
                let order: Mp4TrackOrder = t.into();
                if let Some(ts) = track_states.get_mut(&order) {
                    ts.chunk_offsets.push(current_start_offset);
                    ts.chunk_sample_counts.push(sample_count);
                }
                chunks.push(PreparedChunk {
                    track: t,
                    rel_offset_in_mdat: current_start_offset,
                    sample_count,
                    payloads: std::mem::take(&mut current_payloads),
                });
            }
        }
        if current_track.is_none() {
            current_track = Some(pkt.track);
            current_start_offset = mdat_body_len;
        }
        mdat_body_len = mdat_body_len.saturating_add(pkt.payload.len() as u64);
        current_payloads.push(&pkt.payload);
    }

    if let Some(t) = current_track.take() {
        let sample_count = current_payloads.len() as u32;
        let order: Mp4TrackOrder = t.into();
        if let Some(ts) = track_states.get_mut(&order) {
            ts.chunk_offsets.push(current_start_offset);
            ts.chunk_sample_counts.push(sample_count);
        }
        chunks.push(PreparedChunk {
            track: t,
            rel_offset_in_mdat: current_start_offset,
            sample_count,
            payloads: current_payloads,
        });
    }

    let ftyp = build_ftyp_box(headers.video.codec, false);
    let moov_placeholder = build_moov_box(headers, &track_states, 0, false);
    let mdat_header_size: u64 = if mdat_body_len + 8 <= u64::from(u32::MAX) {
        8
    } else {
        16
    };
    let mdat_data_start = ftyp.len() as u64 + moov_placeholder.len() as u64 + mdat_header_size;
    let moov = build_moov_box(headers, &track_states, mdat_data_start, false);
    debug_assert_eq!(moov.len(), moov_placeholder.len());

    let file = File::create(path)?;
    let mut writer = BufWriter::with_capacity(256 * 1024, file);
    writer.write_all(&ftyp)?;
    writer.write_all(&moov)?;
    write_mdat_header(&mut writer, mdat_body_len)?;
    for chunk in &chunks {
        let _ = (chunk.track, chunk.rel_offset_in_mdat, chunk.sample_count);
        for slice in &chunk.payloads {
            writer.write_all(slice)?;
        }
    }
    writer.flush()?;

    Ok(mdat_data_start + mdat_body_len)
}

#[derive(Debug, Clone)]
struct RecordedSampleEntry {
    track: TrackKind,
    pts_us: i64,
    duration_us: u32,
    is_keyframe: bool,
    file_offset: u64,
    size: u32,
}

/// Crash-safe fragmented MP4 (`fMP4`) writer for full sessions.
///
/// Flushes periodic `moof` + `mdat` fragments (every GOP) so power loss or app crash leaves a
/// playable fragmented MP4 on disk, and optionally remuxes sequentially into `faststart` MP4 on stop.
pub struct Fmp4DiskWriter {
    temp_fmp4_path: PathBuf,
    final_mp4_path: PathBuf,
    headers: StreamHeaders,
    writer: BufWriter<File>,
    file_pos: u64,
    fragment_seq: u32,
    pending_gop: Vec<MediaPacket>,
    pending_start_pts_us: Option<i64>,
    base_pts_us: Option<i64>,
    last_pts_ticks: BTreeMap<Mp4TrackOrder, u64>,
    recorded_samples: Vec<RecordedSampleEntry>,
    finalized: bool,
}

impl Fmp4DiskWriter {
    /// Creates the initialization `ftyp` + `moov` (`mvex`/`trex`) segment on disk immediately.
    pub fn create(final_mp4_path: &Path, headers: StreamHeaders) -> Result<Self, MuxError> {
        if let Some(parent) = final_mp4_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }

        let temp_fmp4_path = PathBuf::from(format!("{}.fmp4", final_mp4_path.display()));
        let file = File::create(&temp_fmp4_path)?;
        let mut writer = BufWriter::with_capacity(256 * 1024, file);

        let ftyp = build_ftyp_box(headers.video.codec, true);
        let empty_tracks = BTreeMap::new();
        let moov = build_moov_box(&headers, &empty_tracks, 0, true);

        writer.write_all(&ftyp)?;
        writer.write_all(&moov)?;
        writer.flush()?;

        let file_pos = (ftyp.len() + moov.len()) as u64;

        Ok(Self {
            temp_fmp4_path,
            final_mp4_path: final_mp4_path.to_path_buf(),
            headers,
            writer,
            file_pos,
            fragment_seq: 0,
            pending_gop: Vec::new(),
            pending_start_pts_us: None,
            base_pts_us: None,
            last_pts_ticks: BTreeMap::new(),
            recorded_samples: Vec::new(),
            finalized: false,
        })
    }

    /// Returns the active crash-safe `.fmp4` path on disk.
    #[must_use]
    pub fn temp_fmp4_path(&self) -> &Path {
        &self.temp_fmp4_path
    }

    /// Returns total bytes flushed to disk so far.
    #[must_use]
    pub fn bytes_written(&self) -> u64 {
        self.file_pos
    }

    /// Appends one packet to the current GOP buffer, flushing a `moof` + `mdat` fragment on
    /// every new video IDR boundary after the first GOP.
    pub fn push_packet(&mut self, packet: MediaPacket) -> Result<(), MuxError> {
        if self.base_pts_us.is_none() {
            if packet.track == TrackKind::Video && packet.is_keyframe {
                self.base_pts_us = Some(packet.pts_us);
            } else {
                // Drop non-IDR frames prior to the first session keyframe.
                return Ok(());
            }
        }

        let base_pts = self.base_pts_us.unwrap_or(0);
        if packet.track.is_audio() && packet.pts_us < base_pts.saturating_sub(21_333) {
            return Ok(());
        }

        if packet.track == TrackKind::Video && packet.is_keyframe && !self.pending_gop.is_empty() {
            self.flush_pending_fragment()?;
        }

        if self.pending_start_pts_us.is_none() {
            self.pending_start_pts_us = Some(packet.pts_us);
        }
        self.pending_gop.push(packet);

        // Safety flush if keyframe interval was larger than 2 seconds.
        if let (Some(start), Some(last)) = (self.pending_start_pts_us, self.pending_gop.last()) {
            if last.pts_us.saturating_sub(start) >= 2_500_000 {
                self.flush_pending_fragment()?;
            }
        }

        Ok(())
    }

    /// Flushes any buffered packets into a `moof` + `mdat` atom and flushes OS buffers.
    pub fn flush_pending_fragment(&mut self) -> Result<(), MuxError> {
        if self.pending_gop.is_empty() {
            return Ok(());
        }
        let base_pts = self.base_pts_us.unwrap_or(0);
        let packets = std::mem::take(&mut self.pending_gop);
        self.pending_start_pts_us = None;

        self.fragment_seq = self.fragment_seq.saturating_add(1);

        let mut per_track: BTreeMap<Mp4TrackOrder, Vec<MediaPacket>> = BTreeMap::new();
        for pkt in packets {
            let rebased = pkt.with_rebased_timestamps(base_pts);
            per_track.entry(rebased.track.into()).or_default().push(rebased);
        }

        // Build moof box with `trun` data_offset patch locations.
        struct TrunPatch {
            offset_in_moof: usize,
            track_payload_rel_offset: u64,
        }
        let mut moof_buf = Vec::with_capacity(4096);
        let mut trun_patches: Vec<TrunPatch> = Vec::new();
        let mut total_mdat_payload: u64 = 0;

        write_box(&mut moof_buf, b"moof", |moof| {
            write_full_box(moof, b"mfhd", 0, 0, |b| {
                b.extend_from_slice(&self.fragment_seq.to_be_bytes());
            });

            for (order, list) in &per_track {
                if list.is_empty() {
                    continue;
                }
                let kind = list[0].track;
                let timescale = track_timescale(kind, &self.headers);
                let first_pts_ticks = us_to_timescale(list[0].pts_us.max(0), timescale) as u64;
                let base_decode_time = self
                    .last_pts_ticks
                    .get(order)
                    .copied()
                    .unwrap_or(first_pts_ticks)
                    .max(first_pts_ticks);

                let mut durations: Vec<u32> = Vec::with_capacity(list.len());
                let mut cursor_ticks = base_decode_time;
                for i in 0..list.len() {
                    let dur_ticks = if i + 1 < list.len() {
                        let next_ticks =
                            us_to_timescale(list[i + 1].pts_us.max(0), timescale) as u64;
                        let delta = next_ticks.saturating_sub(cursor_ticks);
                        u32::try_from(delta.max(1)).unwrap_or(1)
                    } else {
                        u32::try_from(
                            us_to_timescale(i64::from(list[i].duration_us), timescale).max(1),
                        )
                        .unwrap_or(1)
                    };
                    cursor_ticks = cursor_ticks.saturating_add(u64::from(dur_ticks));
                    durations.push(dur_ticks);
                }
                self.last_pts_ticks.insert(*order, cursor_ticks);

                let track_rel_offset = total_mdat_payload;
                for pkt in list {
                    total_mdat_payload =
                        total_mdat_payload.saturating_add(pkt.payload.len() as u64);
                }

                write_box(moof, b"traf", |traf| {
                    // tfhd: flags = 0x020000 (default-base-is-moof)
                    write_full_box(traf, b"tfhd", 0, 0x02_0000, |b| {
                        b.extend_from_slice(&kind.mp4_track_id().to_be_bytes());
                    });
                    // tfdt: version 1 (64-bit baseMediaDecodeTime)
                    write_full_box(traf, b"tfdt", 1, 0, |b| {
                        b.extend_from_slice(&base_decode_time.to_be_bytes());
                    });
                    // trun: data_offset + sample_duration + sample_size + sample_flags
                    let flags = 0x0001 | 0x0100 | 0x0200 | 0x0400;
                    write_full_box(traf, b"trun", 0, flags, |b| {
                        b.extend_from_slice(&(list.len() as u32).to_be_bytes());
                        let patch_pos = b.len();
                        b.extend_from_slice(&0i32.to_be_bytes()); // patched below
                        trun_patches.push(TrunPatch {
                            offset_in_moof: patch_pos,
                            track_payload_rel_offset: track_rel_offset,
                        });

                        for (idx, pkt) in list.iter().enumerate() {
                            let sample_flags: u32 = if pkt.track == TrackKind::Video {
                                if pkt.is_keyframe {
                                    0x0200_0000
                                } else {
                                    0x0101_0000
                                }
                            } else {
                                0x0200_0000
                            };
                            b.extend_from_slice(&durations[idx].to_be_bytes());
                            b.extend_from_slice(&(pkt.payload.len() as u32).to_be_bytes());
                            b.extend_from_slice(&sample_flags.to_be_bytes());
                        }
                    });
                });
            }
        });

        let moof_len = moof_buf.len() as u64;
        let mdat_hdr_size: u64 = if total_mdat_payload + 8 <= u64::from(u32::MAX) {
            8
        } else {
            16
        };
        for patch in trun_patches {
            let data_offset = i32::try_from(
                moof_len
                    .saturating_add(mdat_hdr_size)
                    .saturating_add(patch.track_payload_rel_offset),
            )
            .unwrap_or(i32::MAX);
            moof_buf[patch.offset_in_moof..patch.offset_in_moof + 4]
                .copy_from_slice(&data_offset.to_be_bytes());
        }

        self.writer.write_all(&moof_buf)?;
        write_mdat_header(&mut self.writer, total_mdat_payload)?;
        let mut cursor_file_offset = self.file_pos + moof_len + mdat_hdr_size;

        for list in per_track.values() {
            for pkt in list {
                let sz = pkt.payload.len() as u32;
                self.writer.write_all(&pkt.payload)?;
                self.recorded_samples.push(RecordedSampleEntry {
                    track: pkt.track,
                    pts_us: pkt.pts_us,
                    duration_us: pkt.duration_us,
                    is_keyframe: pkt.is_keyframe,
                    file_offset: cursor_file_offset,
                    size: sz,
                });
                cursor_file_offset = cursor_file_offset.saturating_add(u64::from(sz));
            }
        }

        self.file_pos = cursor_file_offset;
        self.writer.flush()?;
        Ok(())
    }

    /// Completes the recording, flushing remaining samples and optionally remuxing sequentially
    /// into a standard `faststart` MP4 (`moov` before `mdat`) with minimal RAM usage.
    pub fn finalize(mut self, remux_to_faststart: bool) -> Result<u64, MuxError> {
        self.flush_pending_fragment()?;
        self.writer.flush()?;
        self.finalized = true;

        // Close writer before renaming/remuxing the underlying `.fmp4` file.
        drop(self.writer);

        if !remux_to_faststart || self.recorded_samples.is_empty() {
            if self.final_mp4_path.exists() {
                let _ = fs::remove_file(&self.final_mp4_path);
            }
            fs::rename(&self.temp_fmp4_path, &self.final_mp4_path)?;
            return Ok(self.file_pos);
        }

        let written = remux_indexed_fmp4_to_faststart(
            &self.temp_fmp4_path,
            &self.final_mp4_path,
            &self.headers,
            &self.recorded_samples,
        )?;
        let _ = fs::remove_file(&self.temp_fmp4_path);
        debug!(
            "Remuxed crash-safe fMP4 into faststart MP4 ({} bytes)",
            written
        );
        Ok(written)
    }
}

fn remux_indexed_fmp4_to_faststart(
    src_fmp4_path: &Path,
    dst_mp4_path: &Path,
    headers: &StreamHeaders,
    samples: &[RecordedSampleEntry],
) -> Result<u64, MuxError> {
    let mut per_track: BTreeMap<Mp4TrackOrder, Vec<&RecordedSampleEntry>> = BTreeMap::new();
    for s in samples {
        per_track.entry(s.track.into()).or_default().push(s);
    }

    let mut track_states: BTreeMap<Mp4TrackOrder, TrackBuildState<'_>> = BTreeMap::new();
    let mut mdat_body_len: u64 = 0;

    for (order, list) in &per_track {
        let kind = order.to_track_kind();
        let timescale = track_timescale(kind, headers);
        let mut cursor_ticks: u64 = 0;
        let mut prepared = Vec::with_capacity(list.len());
        let mut chunk_offsets = Vec::new();
        let mut chunk_sample_counts = Vec::new();

        // Write one contiguous chunk per track in faststart remux (or split every 1024 samples
        // for clean streaming) using constant 64 KB copy buffer.
        for chunk in list.chunks(1024) {
            chunk_offsets.push(mdat_body_len);
            chunk_sample_counts.push(chunk.len() as u32);
            for s in chunk {
                mdat_body_len = mdat_body_len.saturating_add(u64::from(s.size));
            }
        }

        for i in 0..list.len() {
            let dur = if i + 1 < list.len() {
                let next_ticks = us_to_timescale(list[i + 1].pts_us.max(0), timescale) as u64;
                let delta = next_ticks.saturating_sub(cursor_ticks);
                u32::try_from(delta.max(1)).unwrap_or(1)
            } else {
                u32::try_from(us_to_timescale(i64::from(list[i].duration_us), timescale).max(1))
                    .unwrap_or(1)
            };
            cursor_ticks = cursor_ticks.saturating_add(u64::from(dur));
            prepared.push(PreparedSample {
                duration_ticks: dur,
                size: list[i].size,
                is_keyframe: list[i].is_keyframe,
                payload: &[],
            });
        }

        track_states.insert(
            *order,
            TrackBuildState {
                kind,
                timescale,
                total_duration_ticks: cursor_ticks,
                samples: prepared,
                chunk_offsets,
                chunk_sample_counts,
            },
        );
    }

    let ftyp = build_ftyp_box(headers.video.codec, false);
    let moov_placeholder = build_moov_box(headers, &track_states, 0, false);
    let mdat_hdr_size: u64 = if mdat_body_len + 8 <= u64::from(u32::MAX) {
        8
    } else {
        16
    };
    let mdat_data_start = ftyp.len() as u64 + moov_placeholder.len() as u64 + mdat_hdr_size;
    let moov = build_moov_box(headers, &track_states, mdat_data_start, false);

    let mut src = File::open(src_fmp4_path)?;
    let dst_file = File::create(dst_mp4_path)?;
    let mut dst = BufWriter::with_capacity(256 * 1024, dst_file);

    dst.write_all(&ftyp)?;
    dst.write_all(&moov)?;
    write_mdat_header(&mut dst, mdat_body_len)?;

    let mut copy_buf = vec![0u8; 64 * 1024];
    for list in per_track.values() {
        for sample in list {
            src.seek(SeekFrom::Start(sample.file_offset))?;
            let mut remaining = sample.size as usize;
            while remaining > 0 {
                let to_read = remaining.min(copy_buf.len());
                src.read_exact(&mut copy_buf[..to_read])?;
                dst.write_all(&copy_buf[..to_read])?;
                remaining -= to_read;
            }
        }
    }
    dst.flush()?;
    Ok(mdat_data_start + mdat_body_len)
}

fn build_track_state<'a>(
    kind: TrackKind,
    timescale: u32,
    packets: &[&'a MediaPacket],
) -> TrackBuildState<'a> {
    let mut samples = Vec::with_capacity(packets.len());
    let mut total_ticks: u64 = 0;

    for i in 0..packets.len() {
        let dur_ticks = if i + 1 < packets.len() {
            let next_ticks = us_to_timescale(packets[i + 1].pts_us.max(0), timescale) as u64;
            let delta = next_ticks.saturating_sub(total_ticks);
            u32::try_from(delta.max(1)).unwrap_or(1)
        } else {
            u32::try_from(us_to_timescale(i64::from(packets[i].duration_us), timescale).max(1))
                .unwrap_or(1)
        };
        total_ticks = total_ticks.saturating_add(u64::from(dur_ticks));
        samples.push(PreparedSample {
            duration_ticks: dur_ticks,
            size: packets[i].payload.len() as u32,
            is_keyframe: packets[i].is_keyframe,
            payload: &packets[i].payload,
        });
    }

    let _ = samples.first().map(|s| s.payload.len());

    TrackBuildState {
        kind,
        timescale,
        total_duration_ticks: total_ticks,
        samples,
        chunk_offsets: Vec::new(),
        chunk_sample_counts: Vec::new(),
    }
}

fn header_for_audio_track(kind: TrackKind, headers: &StreamHeaders) -> Option<&core_types::AudioStreamHeader> {
    match kind {
        TrackKind::Video => None,
        TrackKind::AudioMixed => headers.audio_mixed.as_ref(),
        TrackKind::AudioGame => headers.audio_game.as_ref(),
        TrackKind::AudioMic => headers.audio_mic.as_ref(),
        TrackKind::AudioSecondary => headers.audio_secondary.as_ref(),
        TrackKind::AudioApp(idx) => headers
            .app_audio
            .iter()
            .find(|(slot, _, _)| *slot == idx)
            .map(|(_, _, hdr)| hdr),
    }
}

fn track_timescale(kind: TrackKind, headers: &StreamHeaders) -> u32 {
    if kind == TrackKind::Video {
        VIDEO_TIMESCALE_90KHZ
    } else {
        header_for_audio_track(kind, headers).map_or(48_000, |h| h.sample_rate.max(8_000))
    }
}

fn build_ftyp_box(codec: VideoCodec, fragmented: bool) -> Vec<u8> {
    let mut buf = Vec::with_capacity(36);
    write_box(&mut buf, b"ftyp", |b| {
        b.extend_from_slice(b"isom");
        b.extend_from_slice(&0x0000_0200u32.to_be_bytes());
        b.extend_from_slice(b"isom");
        b.extend_from_slice(b"iso2");
        match codec {
            VideoCodec::H264 => b.extend_from_slice(b"avc1"),
            VideoCodec::Hevc => b.extend_from_slice(b"hvc1"),
            VideoCodec::Av1 => b.extend_from_slice(b"av01"),
        }
        b.extend_from_slice(b"mp41");
        if fragmented {
            b.extend_from_slice(b"iso6");
        }
    });
    buf
}

fn build_moov_box(
    headers: &StreamHeaders,
    tracks: &BTreeMap<Mp4TrackOrder, TrackBuildState<'_>>,
    mdat_data_start: u64,
    fragmented: bool,
) -> Vec<u8> {
    let mvhd_timescale = 1_000u32;
    let max_duration_ms: u32 = tracks
        .values()
        .map(|t| {
            let ms = (u128::from(t.total_duration_ticks) * u128::from(mvhd_timescale))
                / u128::from(t.timescale.max(1));
            u32::try_from(ms).unwrap_or(u32::MAX)
        })
        .max()
        .unwrap_or(0);

    let mut active_orders: Vec<(Mp4TrackOrder, TrackKind)> = Vec::new();
    if fragmented {
        active_orders.push((Mp4TrackOrder::Video, TrackKind::Video));
        if headers.audio_mixed.is_some() {
            active_orders.push((Mp4TrackOrder::AudioMixed, TrackKind::AudioMixed));
        }
        if headers.audio_game.is_some() {
            active_orders.push((Mp4TrackOrder::AudioGame, TrackKind::AudioGame));
        }
        if headers.audio_mic.is_some() {
            active_orders.push((Mp4TrackOrder::AudioMic, TrackKind::AudioMic));
        }
        if headers.audio_secondary.is_some() {
            active_orders.push((Mp4TrackOrder::AudioSecondary, TrackKind::AudioSecondary));
        }
        for (idx, _, _) in &headers.app_audio {
            active_orders.push((Mp4TrackOrder::AudioApp(*idx), TrackKind::AudioApp(*idx)));
        }
    } else {
        for (order, ts) in tracks {
            active_orders.push((*order, ts.kind));
        }
    }

    let next_track_id = active_orders
        .iter()
        .map(|(_, k)| k.mp4_track_id())
        .max()
        .unwrap_or(1)
        + 1;

    let mut buf = Vec::with_capacity(2048);
    write_box(&mut buf, b"moov", |moov| {
        // mvhd (version 0)
        write_full_box(moov, b"mvhd", 0, 0, |b| {
            b.extend_from_slice(&0u32.to_be_bytes()); // creation_time
            b.extend_from_slice(&0u32.to_be_bytes()); // modification_time
            b.extend_from_slice(&mvhd_timescale.to_be_bytes());
            b.extend_from_slice(&max_duration_ms.to_be_bytes());
            b.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // rate 1.0
            b.extend_from_slice(&0x0100u16.to_be_bytes()); // volume 1.0
            b.extend_from_slice(&[0u8; 10]); // reserved
            write_identity_matrix(b);
            b.extend_from_slice(&[0u8; 24]); // pre_defined
            b.extend_from_slice(&next_track_id.to_be_bytes());
        });

        for (order, kind) in &active_orders {
            let state = tracks.get(order);
            write_trak_box(moov, headers, *kind, state, mvhd_timescale, mdat_data_start);
        }

        if fragmented {
            write_box(moov, b"mvex", |mvex| {
                for (_, kind) in &active_orders {
                    write_full_box(mvex, b"trex", 0, 0, |b| {
                        b.extend_from_slice(&kind.mp4_track_id().to_be_bytes());
                        b.extend_from_slice(&1u32.to_be_bytes()); // default_sample_description_index
                        b.extend_from_slice(&0u32.to_be_bytes()); // default_sample_duration
                        b.extend_from_slice(&0u32.to_be_bytes()); // default_sample_size
                        b.extend_from_slice(&0u32.to_be_bytes()); // default_sample_flags
                    });
                }
            });
        }
    });

    buf
}

fn write_trak_box(
    moov: &mut Vec<u8>,
    headers: &StreamHeaders,
    kind: TrackKind,
    state: Option<&TrackBuildState<'_>>,
    mvhd_timescale: u32,
    mdat_data_start: u64,
) {
    let timescale = state.map_or_else(|| track_timescale(kind, headers), |s| s.timescale);
    let track_ticks = state.map_or(0, |s| s.total_duration_ticks);
    let mvhd_duration = u32::try_from(
        (u128::from(track_ticks) * u128::from(mvhd_timescale)) / u128::from(timescale.max(1)),
    )
    .unwrap_or(u32::MAX);

    write_box(moov, b"trak", |trak| {
        // Mark Video and AudioMixed as enabled by default (`flags = 0x03`) so standard players
        // (Windows Media Player, browsers, Discord) play the combined mix automatically.
        // Mark independent stems (`AudioGame`, `AudioMic`, `AudioSecondary`) as `in_movie`
        // (`flags = 0x02`) in `alternate_group = 1` for non-destructive editor stems.
        let tkhd_flags: u32 = match kind {
            TrackKind::Video | TrackKind::AudioMixed => 0x03,
            TrackKind::AudioGame
            | TrackKind::AudioMic
            | TrackKind::AudioSecondary
            | TrackKind::AudioApp(_) => 0x02,
        };
        write_full_box(trak, b"tkhd", 0, tkhd_flags, |b| {
            b.extend_from_slice(&0u32.to_be_bytes());
            b.extend_from_slice(&0u32.to_be_bytes());
            b.extend_from_slice(&kind.mp4_track_id().to_be_bytes());
            b.extend_from_slice(&0u32.to_be_bytes());
            b.extend_from_slice(&mvhd_duration.to_be_bytes());
            b.extend_from_slice(&[0u8; 8]); // reserved
            b.extend_from_slice(&0u16.to_be_bytes()); // layer
            let alternate_group: u16 = match kind {
                TrackKind::Video | TrackKind::AudioMixed => 0,
                TrackKind::AudioGame
                | TrackKind::AudioMic
                | TrackKind::AudioSecondary
                | TrackKind::AudioApp(_) => 1,
            };
            b.extend_from_slice(&alternate_group.to_be_bytes()); // alternate_group
            b.extend_from_slice(&if kind.is_audio() { 0x0100u16 } else { 0u16 }.to_be_bytes()); // volume (1.0 = 0x0100 for audio)
            b.extend_from_slice(&0u16.to_be_bytes()); // reserved
            write_identity_matrix(b);
            if kind == TrackKind::Video {
                b.extend_from_slice(&(headers.video.width << 16).to_be_bytes());
                b.extend_from_slice(&(headers.video.height << 16).to_be_bytes());
            } else {
                b.extend_from_slice(&0u32.to_be_bytes());
                b.extend_from_slice(&0u32.to_be_bytes());
            }
        });

        write_box(trak, b"mdia", |mdia| {
            write_full_box(mdia, b"mdhd", 0, 0, |b| {
                b.extend_from_slice(&0u32.to_be_bytes());
                b.extend_from_slice(&0u32.to_be_bytes());
                b.extend_from_slice(&timescale.to_be_bytes());
                b.extend_from_slice(&(u32::try_from(track_ticks).unwrap_or(u32::MAX)).to_be_bytes());
                b.extend_from_slice(&0x55C4u16.to_be_bytes()); // "und"
                b.extend_from_slice(&0u16.to_be_bytes());
            });

            write_full_box(mdia, b"hdlr", 0, 0, |b| {
                b.extend_from_slice(&0u32.to_be_bytes());
                let (handler_type, owned_name): (&[u8; 4], Vec<u8>) = match kind {
                    TrackKind::Video => (b"vide", b"VideoHandler\0".to_vec()),
                    TrackKind::AudioMixed => (b"soun", b"Mixed Audio\0".to_vec()),
                    TrackKind::AudioGame => (b"soun", b"Game Audio\0".to_vec()),
                    TrackKind::AudioMic => (b"soun", b"Microphone\0".to_vec()),
                    TrackKind::AudioSecondary => (b"soun", b"Secondary Audio\0".to_vec()),
                    TrackKind::AudioApp(idx) => {
                        let label = headers
                            .app_audio
                            .iter()
                            .find(|(slot, _, _)| *slot == idx)
                            .map(|(_, lbl, _)| lbl.as_str())
                            .unwrap_or("App Audio");
                        let mut bytes = label.as_bytes().to_vec();
                        bytes.push(0);
                        (b"soun", bytes)
                    }
                };
                b.extend_from_slice(handler_type);
                b.extend_from_slice(&[0u8; 12]);
                b.extend_from_slice(&owned_name);
            });

            write_box(mdia, b"minf", |minf| {
                if kind == TrackKind::Video {
                    write_full_box(minf, b"vmhd", 0, 1, |b| {
                        b.extend_from_slice(&[0u8; 8]);
                    });
                } else {
                    write_full_box(minf, b"smhd", 0, 0, |b| {
                        b.extend_from_slice(&[0u8; 4]);
                    });
                }

                write_box(minf, b"dinf", |dinf| {
                    write_full_box(dinf, b"dref", 0, 0, |b| {
                        b.extend_from_slice(&1u32.to_be_bytes());
                        write_full_box(b, b"url ", 0, 1, |_| {});
                    });
                });

                write_stbl_box(minf, headers, kind, state, mdat_data_start);
            });
        });
    });
}

fn write_stbl_box(
    minf: &mut Vec<u8>,
    headers: &StreamHeaders,
    kind: TrackKind,
    state: Option<&TrackBuildState<'_>>,
    mdat_data_start: u64,
) {
    write_box(minf, b"stbl", |stbl| {
        // stsd
        write_full_box(stbl, b"stsd", 0, 0, |b| {
            b.extend_from_slice(&1u32.to_be_bytes());
            match kind {
                TrackKind::Video => {
                    let fourcc = match headers.video.codec {
                        VideoCodec::H264 => b"avc1",
                        VideoCodec::Hevc => b"hvc1",
                        VideoCodec::Av1 => b"av01",
                    };
                    write_box(b, fourcc, |entry| {
                        entry.extend_from_slice(&[0u8; 6]);
                        entry.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
                        entry.extend_from_slice(&[0u8; 16]); // pre_defined + reserved
                        entry.extend_from_slice(&(headers.video.width as u16).to_be_bytes());
                        entry.extend_from_slice(&(headers.video.height as u16).to_be_bytes());
                        entry.extend_from_slice(&0x0048_0000u32.to_be_bytes()); // 72 dpi
                        entry.extend_from_slice(&0x0048_0000u32.to_be_bytes()); // 72 dpi
                        entry.extend_from_slice(&0u32.to_be_bytes());
                        entry.extend_from_slice(&1u16.to_be_bytes()); // frame_count
                        entry.extend_from_slice(&[0u8; 32]); // compressorname
                        entry.extend_from_slice(&0x0018u16.to_be_bytes()); // depth 24
                        entry.extend_from_slice(&(-1i16).to_be_bytes()); // pre_defined
                        let cfg_fourcc = match headers.video.codec {
                            VideoCodec::H264 => b"avcC",
                            VideoCodec::Hevc => b"hvcC",
                            VideoCodec::Av1 => b"av1C",
                        };
                        write_box(entry, cfg_fourcc, |cfg| {
                            cfg.extend_from_slice(&headers.video.avcc_or_hvcc);
                        });
                        if headers.video.codec == VideoCodec::Av1 {
                            write_box(entry, b"colr", |colr| {
                                colr.extend_from_slice(b"nclx");
                                colr.extend_from_slice(&1u16.to_be_bytes()); // BT.709 primaries
                                colr.extend_from_slice(&1u16.to_be_bytes()); // BT.709 transfer
                                colr.extend_from_slice(&1u16.to_be_bytes()); // BT.709 matrix
                                colr.push(0x00); // limited range
                            });
                        }
                    });
                }
                TrackKind::AudioMixed
                | TrackKind::AudioGame
                | TrackKind::AudioMic
                | TrackKind::AudioSecondary
                | TrackKind::AudioApp(_) => {
                    let audio_hdr = header_for_audio_track(kind, headers);
                    let channels = audio_hdr.map_or(2, |h| h.channels);
                    let sample_rate = audio_hdr.map_or(48_000, |h| h.sample_rate);
                    let bitrate = audio_hdr.map_or(192_000, |h| h.bitrate_bps);
                    let asc = audio_hdr.map_or(&[][..], |h| h.audio_specific_config.as_slice());

                    write_box(b, b"mp4a", |entry| {
                        entry.extend_from_slice(&[0u8; 6]);
                        entry.extend_from_slice(&1u16.to_be_bytes());
                        entry.extend_from_slice(&[0u8; 8]);
                        entry.extend_from_slice(&channels.to_be_bytes());
                        entry.extend_from_slice(&16u16.to_be_bytes()); // sample size
                        entry.extend_from_slice(&0u16.to_be_bytes());
                        entry.extend_from_slice(&0u16.to_be_bytes());
                        entry.extend_from_slice(&(sample_rate << 16).to_be_bytes());
                        write_esds_box(entry, kind.mp4_track_id() as u16, bitrate, asc);
                    });
                }
            }
        });

        // stts (run-length encoded sample durations)
        write_full_box(stbl, b"stts", 0, 0, |b| {
            let Some(st) = state else {
                b.extend_from_slice(&0u32.to_be_bytes());
                return;
            };
            let mut runs: Vec<(u32, u32)> = Vec::new();
            for s in &st.samples {
                if let Some((count, dur)) = runs.last_mut() {
                    if *dur == s.duration_ticks {
                        *count += 1;
                        continue;
                    }
                }
                runs.push((1, s.duration_ticks));
            }
            b.extend_from_slice(&(runs.len() as u32).to_be_bytes());
            for (count, dur) in runs {
                b.extend_from_slice(&count.to_be_bytes());
                b.extend_from_slice(&dur.to_be_bytes());
            }
        });

        // stss (sync sample table for video IDR frames)
        if kind == TrackKind::Video {
            if let Some(st) = state {
                let sync_indices: Vec<u32> = st
                    .samples
                    .iter()
                    .enumerate()
                    .filter_map(|(i, s)| s.is_keyframe.then_some((i + 1) as u32))
                    .collect();
                if !sync_indices.is_empty() {
                    write_full_box(stbl, b"stss", 0, 0, |b| {
                        b.extend_from_slice(&(sync_indices.len() as u32).to_be_bytes());
                        for idx in sync_indices {
                            b.extend_from_slice(&idx.to_be_bytes());
                        }
                    });
                }
            }
        }

        // stsc (sample-to-chunk table, run-length compressed)
        write_full_box(stbl, b"stsc", 0, 0, |b| {
            let Some(st) = state else {
                b.extend_from_slice(&0u32.to_be_bytes());
                return;
            };
            let mut runs: Vec<(u32, u32)> = Vec::new();
            for (i, &cnt) in st.chunk_sample_counts.iter().enumerate() {
                let first_chunk = (i + 1) as u32;
                if runs.last().is_some_and(|(_, prev_cnt)| *prev_cnt == cnt) {
                    continue;
                }
                runs.push((first_chunk, cnt));
            }
            b.extend_from_slice(&(runs.len() as u32).to_be_bytes());
            for (first_chunk, cnt) in runs {
                b.extend_from_slice(&first_chunk.to_be_bytes());
                b.extend_from_slice(&cnt.to_be_bytes());
                b.extend_from_slice(&1u32.to_be_bytes()); // sample_description_index
            }
        });

        // stsz (sample sizes)
        write_full_box(stbl, b"stsz", 0, 0, |b| {
            let Some(st) = state else {
                b.extend_from_slice(&0u32.to_be_bytes());
                b.extend_from_slice(&0u32.to_be_bytes());
                return;
            };
            b.extend_from_slice(&0u32.to_be_bytes());
            b.extend_from_slice(&(st.samples.len() as u32).to_be_bytes());
            for s in &st.samples {
                b.extend_from_slice(&s.size.to_be_bytes());
            }
        });

        // stco (32-bit chunk offsets for universal MF / Explorer compatibility) or co64 when > ~3.87 GiB.
        let use_stco = state.is_none_or(|st| {
            st.chunk_offsets
                .iter()
                .copied()
                .max()
                .unwrap_or(0)
                <= u64::from(u32::MAX) - 128 * 1024 * 1024
        });
        if use_stco {
            write_full_box(stbl, b"stco", 0, 0, |b| {
                let Some(st) = state else {
                    b.extend_from_slice(&0u32.to_be_bytes());
                    return;
                };
                b.extend_from_slice(&(st.chunk_offsets.len() as u32).to_be_bytes());
                for &rel in &st.chunk_offsets {
                    let abs_offset = mdat_data_start.saturating_add(rel);
                    let off32 = u32::try_from(abs_offset).unwrap_or(u32::MAX);
                    b.extend_from_slice(&off32.to_be_bytes());
                }
            });
        } else {
            write_full_box(stbl, b"co64", 0, 0, |b| {
                let Some(st) = state else {
                    b.extend_from_slice(&0u32.to_be_bytes());
                    return;
                };
                b.extend_from_slice(&(st.chunk_offsets.len() as u32).to_be_bytes());
                for &rel in &st.chunk_offsets {
                    let abs_offset = mdat_data_start.saturating_add(rel);
                    b.extend_from_slice(&abs_offset.to_be_bytes());
                }
            });
        }
    });
}

fn descr_len_encoded_size(len: usize) -> usize {
    if len < 128 {
        1
    } else {
        4
    }
}

fn write_esds_box(parent: &mut Vec<u8>, es_id: u16, bitrate: u32, asc: &[u8]) {
    write_full_box(parent, b"esds", 0, 0, |b| {
        // Ensure DecoderSpecificInfo receives a clean 2-byte AudioSpecificConfig (`0x11 0x90`).
        let clean_asc: &[u8] = if asc.len() >= 2 && asc[..2] != [0, 0] {
            &asc[..2]
        } else {
            &[0x11, 0x90]
        };
        let effective_bitrate = bitrate.max(192_000);

        let dsi_len = clean_asc.len();
        let dsi_total = 1 + descr_len_encoded_size(dsi_len) + dsi_len;
        let dec_cfg_len = 13 + dsi_total;
        let dec_cfg_total = 1 + descr_len_encoded_size(dec_cfg_len) + dec_cfg_len;
        let sl_total = 3; // tag (1) + len (1) + value 0x02 (1)
        let es_desc_len = 3 + dec_cfg_total + sl_total;

        // ES_Descriptor (tag 0x03)
        b.push(0x03);
        write_descr_len(b, es_desc_len);
        b.extend_from_slice(&es_id.to_be_bytes());
        b.push(0x00);

        // DecoderConfigDescriptor (tag 0x04)
        b.push(0x04);
        write_descr_len(b, dec_cfg_len);
        b.push(0x40); // objectTypeIndication: Audio ISO/IEC 14496-3
        b.push(0x15); // streamType: AudioStream (0x05 << 2 | 1)
        b.extend_from_slice(&[0x00, 0x18, 0x00]); // bufferSizeDB = 6144 bytes
        b.extend_from_slice(&effective_bitrate.to_be_bytes()); // maxBitrate
        b.extend_from_slice(&effective_bitrate.to_be_bytes()); // avgBitrate

        // DecoderSpecificInfo (tag 0x05)
        b.push(0x05);
        write_descr_len(b, dsi_len);
        b.extend_from_slice(clean_asc);

        // SLConfigDescriptor (tag 0x06)
        b.push(0x06);
        write_descr_len(b, 1);
        b.push(0x02);
    });
}

fn write_descr_len(out: &mut Vec<u8>, len: usize) {
    if len < 128 {
        out.push(len as u8);
    } else {
        // Expandable 4-byte size encoding for MPEG-4 descriptors (ISO 14496-1).
        let l = len as u32;
        out.push(((l >> 21) as u8 & 0x7F) | 0x80);
        out.push(((l >> 14) as u8 & 0x7F) | 0x80);
        out.push(((l >> 7) as u8 & 0x7F) | 0x80);
        out.push((l as u8) & 0x7F);
    }
}

fn write_mdat_header(writer: &mut BufWriter<File>, body_len: u64) -> Result<(), MuxError> {
    if body_len + 8 <= u64::from(u32::MAX) {
        let total = (body_len + 8) as u32;
        writer.write_all(&total.to_be_bytes())?;
        writer.write_all(b"mdat")?;
    } else {
        writer.write_all(&1u32.to_be_bytes())?;
        writer.write_all(b"mdat")?;
        writer.write_all(&(body_len + 16).to_be_bytes())?;
    }
    Ok(())
}

fn write_identity_matrix(b: &mut Vec<u8>) {
    let matrix: [u32; 9] = [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000];
    for val in matrix {
        b.extend_from_slice(&val.to_be_bytes());
    }
}

fn write_box(out: &mut Vec<u8>, fourcc: &[u8; 4], body: impl FnOnce(&mut Vec<u8>)) {
    let start = out.len();
    out.extend_from_slice(&[0u8; 4]);
    out.extend_from_slice(fourcc);
    body(out);
    let size = (out.len() - start) as u32;
    out[start..start + 4].copy_from_slice(&size.to_be_bytes());
}

fn write_full_box(
    out: &mut Vec<u8>,
    fourcc: &[u8; 4],
    version: u8,
    flags: u32,
    body: impl FnOnce(&mut Vec<u8>),
) {
    write_box(out, fourcc, |b| {
        let vf = (u32::from(version) << 24) | (flags & 0x00FF_FFFF);
        b.extend_from_slice(&vf.to_be_bytes());
        body(b);
    });
}
