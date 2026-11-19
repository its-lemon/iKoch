//! Annex-B NAL unit parser, SPS/PPS/VPS extractor, ISO BMFF `avcC`/`hvcC` box builder,
//! and length-prefixed MP4 sample converter.

use core_types::{VideoCodec, VideoStreamHeader};

/// Result of parsing one Annex-B access unit produced by a Media Foundation video encoder.
#[derive(Debug, Clone)]
pub struct ParsedAccessUnit {
    pub is_keyframe: bool,
    pub length_prefixed_payload: Vec<u8>,
    pub vps: Option<Vec<u8>>,
    pub sps: Option<Vec<u8>>,
    pub pps: Option<Vec<u8>>,
}

/// Splits an Annex-B byte stream (`00 00 01` or `00 00 00 01` start codes) into raw NAL units.
#[must_use]
pub fn split_annex_b_nalus(data: &[u8]) -> Vec<&[u8]> {
    let mut nalus = Vec::new();
    let len = data.len();
    let mut i = 0usize;
    let mut nalu_start: Option<usize> = None;

    while i < len {
        let sc_len = if i + 4 <= len && data[i..i + 4] == [0, 0, 0, 1] {
            4
        } else if i + 3 <= len && data[i..i + 3] == [0, 0, 1] {
            3
        } else {
            0
        };

        if sc_len > 0 {
            if let Some(start) = nalu_start {
                if i > start {
                    nalus.push(&data[start..i]);
                }
            }
            i += sc_len;
            nalu_start = Some(i);
        } else {
            i += 1;
        }
    }

    if let Some(start) = nalu_start {
        if start < len {
            nalus.push(&data[start..len]);
        }
    }

    nalus
}

/// Parses an Annex-B frame into MP4 length-prefixed (`avc1`/`hvc1`) sample bytes and extracts
/// parameter sets (`SPS`/`PPS`/`VPS`) and keyframe status.
#[must_use]
pub fn parse_annex_b_access_unit(codec: VideoCodec, annex_b: &[u8]) -> ParsedAccessUnit {
    let nalus = split_annex_b_nalus(annex_b);
    let mut out = Vec::with_capacity(annex_b.len() + 16);
    let mut is_keyframe = false;
    let mut vps: Option<Vec<u8>> = None;
    let mut sps: Option<Vec<u8>> = None;
    let mut pps: Option<Vec<u8>> = None;

    for nalu in nalus {
        if nalu.is_empty() {
            continue;
        }
        match codec {
            VideoCodec::H264 => {
                let nal_type = nalu[0] & 0x1F;
                match nal_type {
                    7 => {
                        sps = Some(nalu.to_vec());
                        continue;
                    }
                    8 => {
                        pps = Some(nalu.to_vec());
                        continue;
                    }
                    5 => is_keyframe = true,
                    9 => continue, // Strip Access Unit Delimiter (AUD) inside MP4 avc1
                    _ => {}
                }
            }
            VideoCodec::Hevc => {
                let nal_type = (nalu[0] >> 1) & 0x3F;
                match nal_type {
                    32 => {
                        vps = Some(nalu.to_vec());
                        continue;
                    }
                    33 => {
                        sps = Some(nalu.to_vec());
                        continue;
                    }
                    34 => {
                        pps = Some(nalu.to_vec());
                        continue;
                    }
                    19..=21 => is_keyframe = true,
                    35 => continue, // Strip AUD
                    _ => {}
                }
            }
            VideoCodec::Av1 => {}
        }

        let nalu_len = nalu.len() as u32;
        out.extend_from_slice(&nalu_len.to_be_bytes());
        out.extend_from_slice(nalu);
    }

    ParsedAccessUnit {
        is_keyframe,
        length_prefixed_payload: out,
        vps,
        sps,
        pps,
    }
}

/// Constructs an ISO/IEC 14496-15 `AVCDecoderConfigurationRecord` (`avcC` payload) from SPS & PPS.
#[must_use]
pub fn build_avcc_record(sps: &[u8], pps: &[u8]) -> Vec<u8> {
    let profile_idc = sps.get(1).copied().unwrap_or(100); // High profile fallback
    let profile_compat = sps.get(2).copied().unwrap_or(0);
    let level_idc = sps.get(3).copied().unwrap_or(41);

    let mut avcc = Vec::with_capacity(11 + sps.len() + pps.len());
    avcc.push(1); // configurationVersion
    avcc.push(profile_idc);
    avcc.push(profile_compat);
    avcc.push(level_idc);
    avcc.push(0xFF); // 6 bits reserved (1) + lengthSizeMinusOne (3 => 4-byte lengths)
    avcc.push(0xE1); // 3 bits reserved (1) + numOfSequenceParameterSets (1)
    avcc.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    avcc.extend_from_slice(sps);
    avcc.push(1); // numOfPictureParameterSets
    avcc.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    avcc.extend_from_slice(pps);
    avcc
}

/// Constructs a baseline ISO/IEC 14496-15 `HEVCDecoderConfigurationRecord` (`hvcC` payload).
#[must_use]
pub fn build_hvcc_record(vps: &[u8], sps: &[u8], pps: &[u8]) -> Vec<u8> {
    let mut hvcc = Vec::with_capacity(32 + vps.len() + sps.len() + pps.len());
    hvcc.push(1); // configurationVersion
    hvcc.push(0x01); // general_profile_space=0, tier=0, profile_idc=1 (Main)
    hvcc.extend_from_slice(&0x6000_0000u32.to_be_bytes()); // general_profile_compatibility_flags
    hvcc.extend_from_slice(&[0xB0, 0x00, 0x00, 0x00, 0x00, 0x00]); // constraint_indicator_flags
    hvcc.push(120); // general_level_idc (4.0)
    hvcc.extend_from_slice(&0xF000u16.to_be_bytes()); // min_spatial_segmentation_idc
    hvcc.push(0xFC); // parallelismType
    hvcc.push(0xFD); // chromaFormat = 1 (4:2:0)
    hvcc.push(0xF8); // bitDepthLumaMinus8 = 0
    hvcc.push(0xF8); // bitDepthChromaMinus8 = 0
    hvcc.extend_from_slice(&0u16.to_be_bytes()); // avgFrameRate
    hvcc.push(0x0F); // constantFrameRate=0, numTemporalLayers=1, temporalIdNested=1, lengthSizeMinusOne=3
    hvcc.push(3); // numOfArrays (VPS, SPS, PPS)

    for (nal_type, data) in [(32u8, vps), (33u8, sps), (34u8, pps)] {
        hvcc.push(0x80 | (nal_type & 0x3F)); // array_completeness=1 | nal_unit_type
        hvcc.extend_from_slice(&1u16.to_be_bytes()); // numNalus = 1
        hvcc.extend_from_slice(&(data.len() as u16).to_be_bytes());
        hvcc.extend_from_slice(data);
    }

    hvcc
}

/// Builds a complete [`VideoStreamHeader`] once parameter sets are available.
#[must_use]
pub fn build_video_stream_header(
    codec: VideoCodec,
    width: u32,
    height: u32,
    fps: u32,
    vps: Option<Vec<u8>>,
    sps: Vec<u8>,
    pps: Vec<u8>,
) -> VideoStreamHeader {
    let avcc_or_hvcc = match codec {
        VideoCodec::H264 => build_avcc_record(&sps, &pps),
        VideoCodec::Hevc => build_hvcc_record(vps.as_deref().unwrap_or(&[]), &sps, &pps),
        VideoCodec::Av1 => Vec::new(),
    };

    VideoStreamHeader {
        codec,
        width,
        height,
        fps_num: fps.max(1),
        fps_den: 1,
        sps,
        pps,
        vps,
        avcc_or_hvcc,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h264_annex_b_extracts_sps_pps_idr_and_builds_avcc() {
        // Fake Annex-B AU: SPS (type 7), PPS (type 8), IDR (type 5)
        let annex_b = [
            0x00, 0x00, 0x00, 0x01, 0x67, 0x64, 0x00, 0x28, 0xAC, // SPS
            0x00, 0x00, 0x00, 0x01, 0x68, 0xEE, 0x3C, 0x80, // PPS
            0x00, 0x00, 0x01, 0x65, 0x88, 0x84, 0x21, // IDR
        ];

        let parsed = parse_annex_b_access_unit(VideoCodec::H264, &annex_b);
        assert!(parsed.is_keyframe);
        assert_eq!(parsed.sps.as_deref(), Some(&[0x67, 0x64, 0x00, 0x28, 0xAC][..]));
        assert_eq!(parsed.pps.as_deref(), Some(&[0x68, 0xEE, 0x3C, 0x80][..]));
        // Length-prefixed payload must contain only the 4-byte length + 4-byte IDR slice
        assert_eq!(
            parsed.length_prefixed_payload,
            vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x21]
        );

        let avcc = build_avcc_record(
            parsed.sps.as_deref().unwrap_or(&[]),
            parsed.pps.as_deref().unwrap_or(&[]),
        );
        assert_eq!(avcc[0], 1);
        assert_eq!(avcc[1], 0x64); // High profile
        assert_eq!(avcc[4], 0xFF); // 4-byte NALU lengths
    }

    #[test]
    fn h264_non_idr_p_frame_detected_correctly() {
        let annex_b = [0x00, 0x00, 0x00, 0x01, 0x41, 0x9A, 0x22, 0x11];
        let parsed = parse_annex_b_access_unit(VideoCodec::H264, &annex_b);
        assert!(!parsed.is_keyframe);
        assert!(parsed.sps.is_none());
        assert_eq!(
            parsed.length_prefixed_payload,
            vec![0x00, 0x00, 0x00, 0x04, 0x41, 0x9A, 0x22, 0x11]
        );
    }
}
