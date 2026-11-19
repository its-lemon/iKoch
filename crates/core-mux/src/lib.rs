//! Compressed media packet routing (`PacketRouter`), GOP-aware Replay Ring Buffer,
//! crash-safe fragmented MP4 (`fMP4`) disk writer, and progressive `faststart` MP4 muxer.

pub mod mp4_writer;
pub mod ring_buffer;
pub mod router;

pub use core_types::{MediaPacket, MuxError, StreamHeaders, TrackKind};
pub use mp4_writer::{write_faststart_mp4, Fmp4DiskWriter};
pub use ring_buffer::GopRingBuffer;
pub use router::{PacketRouter, SavedMediaSummary};

#[cfg(test)]
mod tests {
    use super::*;
    use core_types::{VideoCodec, VideoStreamHeader};
    use std::fs;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn dummy_headers() -> StreamHeaders {
        let sps = vec![0x67, 0x64, 0x00, 0x28, 0xAC, 0xD9];
        let pps = vec![0x68, 0xEE, 0x3C, 0x80];
        let mut avcc = vec![1, 0x64, 0x00, 0x28, 0xFF, 0xE1, 0, sps.len() as u8];
        avcc.extend_from_slice(&sps);
        avcc.push(1);
        avcc.extend_from_slice(&[0, pps.len() as u8]);
        avcc.extend_from_slice(&pps);

        StreamHeaders {
            video: VideoStreamHeader {
                codec: VideoCodec::H264,
                width: 1920,
                height: 1080,
                fps_num: 60,
                fps_den: 1,
                sps,
                pps,
                vps: None,
                avcc_or_hvcc: avcc,
            },
            audio_mixed: None,
            audio_game: None,
            audio_mic: None,
            audio_secondary: None,
        }
    }

    fn dummy_video_pkt(seq: u64, pts_us: i64, is_keyframe: bool, payload_size: usize) -> MediaPacket {
        let mut payload = vec![0u8; payload_size];
        payload[0..4].copy_from_slice(&((payload_size - 4) as u32).to_be_bytes());
        payload[4] = if is_keyframe { 0x65 } else { 0x41 };
        MediaPacket::new(
            TrackKind::Video,
            pts_us,
            pts_us,
            500_000,
            is_keyframe,
            seq,
            payload,
        )
    }

    #[test]
    fn gop_ring_buffer_drops_pre_idr_and_evicts_by_whole_gop_and_resets_source() {
        let mut rb = GopRingBuffer::new(15, 64 * 1024 * 1024);
        rb.set_headers(dummy_headers());

        // Leading P-frames before first IDR must be discarded.
        rb.push_packet(dummy_video_pkt(1, 0, false, 100));
        rb.push_packet(dummy_video_pkt(2, 500_000, false, 100));
        assert_eq!(rb.gop_count(), 0);

        // Push 20 seconds of 1-second GOPs (2 frames per GOP: 1 IDR + 1 P).
        for sec in 1..=20 {
            let base = sec * 1_000_000;
            rb.push_packet(dummy_video_pkt((sec * 2) as u64, base, true, 128));
            rb.push_packet(dummy_video_pkt((sec * 2 + 1) as u64, base + 500_000, false, 64));
        }

        // Oldest GOPs evicted along IDR boundaries while retaining >= 15s window.
        assert!(rb.gop_count() >= 15 && rb.gop_count() <= 16);
        let (_, exported) = match rb.export_rebased_packets() {
            Ok(val) => val,
            Err(err) => panic!("export_rebased_packets failed: {err}"),
        };
        assert!(exported[0].is_keyframe, "exported clip must start with IDR");
        assert_eq!(exported[0].pts_us, 0, "first IDR must be rebased to t = 0");

        // Source switch reset clears all buffered frames cleanly.
        rb.reset_for_new_source();
        assert_eq!(rb.gop_count(), 0);
        assert!(rb.export_rebased_packets().is_err());
    }

    #[test]
    fn faststart_mp4_and_fmp4_remux_produce_valid_box_layout() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir().join(format!("rustclip_mux_test_{}", Uuid::new_v4()));
        fs::create_dir_all(&temp_dir)?;
        let headers = dummy_headers();

        let packets = vec![
            dummy_video_pkt(1, 0, true, 64),
            dummy_video_pkt(2, 500_000, false, 32),
            dummy_video_pkt(3, 1_000_000, true, 64),
            dummy_video_pkt(4, 1_500_000, false, 32),
        ];

        let faststart_path: PathBuf = temp_dir.join("clip_faststart.mp4");
        write_faststart_mp4(&faststart_path, &headers, &packets)?;
        let bytes = fs::read(&faststart_path)?;
        assert_eq!(&bytes[4..8], b"ftyp");
        let moov_pos = bytes.windows(4).position(|w| w == b"moov").unwrap_or(usize::MAX);
        let mdat_pos = bytes.windows(4).position(|w| w == b"mdat").unwrap_or(0);
        assert!(moov_pos < mdat_pos, "moov must precede mdat in faststart MP4");
        assert!(bytes.windows(4).any(|w| w == b"stss"));

        let session_path: PathBuf = temp_dir.join("session_remux.mp4");
        let mut fmp4 = Fmp4DiskWriter::create(&session_path, headers)?;
        for pkt in packets {
            fmp4.push_packet(pkt)?;
        }
        assert!(fmp4.temp_fmp4_path().exists());
        fmp4.finalize(true)?;
        assert!(session_path.exists());

        let remuxed = fs::read(&session_path)?;
        let remux_moov = remuxed.windows(4).position(|w| w == b"moov").unwrap_or(usize::MAX);
        let remux_mdat = remuxed.windows(4).position(|w| w == b"mdat").unwrap_or(0);
        assert!(remux_moov < remux_mdat);

        let _ = fs::remove_dir_all(&temp_dir);
        Ok(())
    }

    #[test]
    fn multi_track_audio_and_video_muxing_sets_mixed_default_and_clean_stems() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir().join(format!("rustclip_multi_audio_{}", Uuid::new_v4()));
        fs::create_dir_all(&temp_dir)?;

        let make_hdr = |track: TrackKind| core_types::AudioStreamHeader {
            track,
            sample_rate: 48_000,
            channels: 2,
            bitrate_bps: 192_000,
            audio_specific_config: vec![0x11, 0x90],
        };

        let mut headers = dummy_headers();
        headers.audio_mixed = Some(make_hdr(TrackKind::AudioMixed));
        headers.audio_game = Some(make_hdr(TrackKind::AudioGame));
        headers.audio_mic = Some(make_hdr(TrackKind::AudioMic));
        headers.audio_secondary = Some(make_hdr(TrackKind::AudioSecondary));

        let mut rb = GopRingBuffer::new(15, 64 * 1024 * 1024);
        rb.set_headers(headers);

        rb.push_packet(dummy_video_pkt(1, 1_000_000, true, 64));
        for i in 0..94i64 {
            let pts = 1_000_000 + i * 21_333;
            for track in [
                TrackKind::AudioMixed,
                TrackKind::AudioGame,
                TrackKind::AudioMic,
                TrackKind::AudioSecondary,
            ] {
                rb.push_packet(MediaPacket::new(
                    track,
                    pts,
                    pts,
                    21_333,
                    true,
                    (i + 1) as u64,
                    vec![0x21, 0x10, 0x05, 0x00],
                ));
            }
        }
        rb.push_packet(dummy_video_pkt(2, 2_000_000, true, 64));

        let (exported_hdrs, exported_pkts) = rb.export_rebased_packets()?;
        assert!(exported_pkts.iter().any(|p| p.track == TrackKind::AudioMixed));
        assert!(exported_pkts.iter().any(|p| p.track == TrackKind::AudioGame));
        assert!(exported_pkts.iter().any(|p| p.track == TrackKind::AudioMic));
        assert!(exported_pkts.iter().any(|p| p.track == TrackKind::AudioSecondary));

        let out_mp4 = temp_dir.join("multi_audio_clip.mp4");
        write_faststart_mp4(&out_mp4, &exported_hdrs, &exported_pkts)?;
        let bytes = fs::read(&out_mp4)?;
        let mp4a_count = bytes.windows(4).filter(|w| *w == b"mp4a").count();
        assert_eq!(mp4a_count, 4, "expected 4 mp4a audio tracks in moov");
        assert!(bytes.windows(11).any(|w| w == b"Mixed Audio"));
        assert!(bytes.windows(10).any(|w| w == b"Game Audio"));
        assert!(bytes.windows(10).any(|w| w == b"Microphone"));
        assert!(bytes.windows(15).any(|w| w == b"Secondary Audio"));

        // Verify tkhd flags: [Video=0x03, Mixed=0x03, Game=0x02, Mic=0x02, Secondary=0x02].
        let tkhd_flags: Vec<u32> = bytes
            .windows(8)
            .filter(|w| &w[0..4] == b"tkhd")
            .map(|w| u32::from_be_bytes([0, w[5], w[6], w[7]]))
            .collect();
        assert_eq!(tkhd_flags, vec![0x03, 0x03, 0x02, 0x02, 0x02]);

        let _ = fs::remove_dir_all(&temp_dir);
        Ok(())
    }

    #[test]
    fn mf_source_reader_decodes_aac_from_faststart_mp4() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir().join(format!("rustclip_mf_verify_{}", Uuid::new_v4()));
        fs::create_dir_all(&temp_dir)?;

        let mut enc_mixed = core_audio::MfAacEncoder::new(TrackKind::AudioMixed, 192)?;
        let mut enc_game = core_audio::MfAacEncoder::new(TrackKind::AudioGame, 192)?;
        let mut enc_mic = core_audio::MfAacEncoder::new(TrackKind::AudioMic, 192)?;

        let mut headers = dummy_headers();
        headers.audio_mixed = Some(enc_mixed.stream_header().clone());
        headers.audio_game = Some(enc_game.stream_header().clone());
        headers.audio_mic = Some(enc_mic.stream_header().clone());

        let mut packets = vec![dummy_video_pkt(1, 0, true, 64)];
        for block_idx in 0..48u64 {
            let pts_us = core_audio::sample_index_to_pts_us(
                0,
                block_idx * (core_audio::AAC_FRAME_SAMPLES as u64),
            );
            let mut pcm = [0i16; core_audio::AAC_FRAME_SAMPLES * core_audio::TARGET_CHANNELS];
            for f in 0..core_audio::AAC_FRAME_SAMPLES {
                let sample_idx = block_idx * (core_audio::AAC_FRAME_SAMPLES as u64) + (f as u64);
                let t = (sample_idx as f32) / 48_000.0;
                let s = ((0.35 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()) * 32_767.0) as i16;
                pcm[f * 2] = s;
                pcm[f * 2 + 1] = s;
            }
            packets.extend(enc_mixed.encode_pcm_block(&pcm, pts_us)?);
            packets.extend(enc_game.encode_pcm_block(&pcm, pts_us)?);
            packets.extend(enc_mic.encode_pcm_block(&pcm, pts_us)?);
        }
        packets.push(dummy_video_pkt(2, 1_000_000, true, 64));

        let mp4_path = temp_dir.join("mf_check.mp4");
        write_faststart_mp4(&mp4_path, &headers, &packets)?;

        let report = core_audio::verify_mp4_audio_with_mf_source_reader(&mp4_path)?;
        assert_eq!(report.sample_rate_hz, 48_000);
        assert_eq!(report.channels, 2);
        assert!(report.decoded_frames >= 40_000);
        assert!(
            report.peak_linear > 0.15,
            "expected audible decoded peak from MF SourceReader, got {}",
            report.peak_linear
        );

        let _ = fs::remove_dir_all(&temp_dir);
        Ok(())
    }

    #[test]
    fn gop_ring_buffer_retains_full_30_plus_seconds_without_premature_eviction() {
        // Default replay window = 30s, generous 512 MiB cap matching AppSettings::default().
        let mut rb = GopRingBuffer::new(30, 512 * 1024 * 1024);
        rb.set_headers(dummy_headers());

        // Push 36 seconds of 1-second GOPs (~256 KB per IDR GOP, simulating 2 Mbps+ bursts).
        for sec in 0..36 {
            let base = sec * 1_000_000;
            rb.push_packet(dummy_video_pkt((sec * 2 + 1) as u64, base, true, 160 * 1024));
            rb.push_packet(dummy_video_pkt(
                (sec * 2 + 2) as u64,
                base + 500_000,
                false,
                96 * 1024,
            ));
        }

        assert!(
            rb.buffered_duration_sec() >= 30.0,
            "expected >= 30.0s retained in ring buffer, got {:.2}s",
            rb.buffered_duration_sec()
        );
        assert!(
            rb.gop_count() >= 30 && rb.gop_count() <= 32,
            "expected 30..=32 GOPs retained for 30s window, got {}",
            rb.gop_count()
        );
    }
}
