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

/// One parsed AV1 Open Bitstream Unit (OBU) slice from an encoded sample.
#[derive(Debug, Clone, Copy)]
pub struct Av1Obu<'a> {
    pub obu_type: u8,
    pub has_extension: bool,
    pub has_size_field: bool,
    pub raw: &'a [u8],
    pub payload: &'a [u8],
}

fn read_leb128(data: &[u8]) -> Option<(usize, usize)> {
    let mut value: usize = 0;
    for (idx, &byte) in data.iter().take(8).enumerate() {
        value |= usize::from(byte & 0x7F) << (idx * 7);
        if (byte & 0x80) == 0 {
            return Some((value, idx + 1));
        }
    }
    None
}

fn write_leb128(mut value: usize, out: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

/// Splits an AV1 OBU bitstream into individual [`Av1Obu`] units.
#[must_use]
pub fn split_av1_obus(data: &[u8]) -> Vec<Av1Obu<'_>> {
    let mut obus = Vec::new();
    let mut pos = 0usize;

    while pos < data.len() {
        let header = data[pos];
        let obu_type = (header >> 3) & 0x0F;
        let has_extension = ((header >> 2) & 0x01) != 0;
        let has_size_field = ((header >> 1) & 0x01) != 0;

        let mut cursor = pos + 1;
        if has_extension {
            if cursor >= data.len() {
                break;
            }
            cursor += 1;
        }

        let payload_len = if has_size_field {
            let Some((sz, leb_len)) = read_leb128(&data[cursor..]) else {
                break;
            };
            cursor += leb_len;
            sz
        } else {
            data.len().saturating_sub(cursor)
        };

        let end = cursor.saturating_add(payload_len).min(data.len());
        obus.push(Av1Obu {
            obu_type,
            has_extension,
            has_size_field,
            raw: &data[pos..end],
            payload: &data[cursor..end],
        });
        pos = end;
    }

    obus
}

/// Ensures an OBU carries `obu_has_size_field = 1` + LEB128 size (required inside `av1C` and ISOBMFF).
fn normalize_obu_with_size(obu: &Av1Obu<'_>) -> Vec<u8> {
    if obu.has_size_field {
        return obu.raw.to_vec();
    }
    let mut out = Vec::with_capacity(obu.raw.len() + 4);
    out.push(obu.raw[0] | 0x02);
    if obu.has_extension && obu.raw.len() >= 2 {
        out.push(obu.raw[1]);
    }
    write_leb128(obu.payload.len(), &mut out);
    out.extend_from_slice(obu.payload);
    out
}

struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }

    fn read_bit(&mut self) -> Option<u8> {
        let byte_idx = self.bit_pos / 8;
        let bit_idx = 7 - (self.bit_pos % 8);
        let byte = *self.data.get(byte_idx)?;
        self.bit_pos += 1;
        Some((byte >> bit_idx) & 1)
    }

    fn read_bits(&mut self, count: usize) -> Option<u32> {
        let mut val = 0u32;
        for _ in 0..count {
            val = (val << 1) | u32::from(self.read_bit()?);
        }
        Some(val)
    }

    fn skip_uvlc(&mut self) -> Option<()> {
        let mut leading_zeros = 0usize;
        while self.read_bit()? == 0 {
            leading_zeros += 1;
            if leading_zeros >= 32 {
                return Some(());
            }
        }
        if leading_zeros > 0 {
            let _ = self.read_bits(leading_zeros)?;
        }
        Some(())
    }
}

/// Parsed fields from `OBU_SEQUENCE_HEADER` needed for `AV1CodecConfigurationRecord` (`av1C`).
#[derive(Debug, Clone, Copy)]
struct Av1SeqHeaderParams {
    seq_profile: u8,
    seq_level_idx_0: u8,
    seq_tier_0: u8,
    high_bitdepth: u8,
    twelve_bit: u8,
    monochrome: u8,
    chroma_subsampling_x: u8,
    chroma_subsampling_y: u8,
    chroma_sample_position: u8,
}

impl Default for Av1SeqHeaderParams {
    fn default() -> Self {
        Self {
            seq_profile: 0,
            seq_level_idx_0: 8, // 4.0
            seq_tier_0: 0,
            high_bitdepth: 0,
            twelve_bit: 0,
            monochrome: 0,
            chroma_subsampling_x: 1,
            chroma_subsampling_y: 1,
            chroma_sample_position: 0,
        }
    }
}

fn parse_av1_seq_header_params(payload: &[u8]) -> Av1SeqHeaderParams {
    let mut out = Av1SeqHeaderParams::default();
    let mut r = BitReader::new(payload);

    let Some(seq_profile) = r.read_bits(3) else {
        return out;
    };
    out.seq_profile = seq_profile as u8;

    let Some(_still_picture) = r.read_bit() else {
        return out;
    };
    let Some(reduced_still_picture_header) = r.read_bit() else {
        return out;
    };

    if reduced_still_picture_header == 1 {
        if let Some(lvl) = r.read_bits(5) {
            out.seq_level_idx_0 = lvl as u8;
        }
    } else {
        let Some(timing_info_present_flag) = r.read_bit() else {
            return out;
        };
        let mut decoder_model_info_present_flag = 0u8;
        let mut buffer_delay_length_minus_1 = 0usize;
        if timing_info_present_flag == 1 {
            if r.read_bits(32).is_none() || r.read_bits(32).is_none() {
                return out;
            }
            if r.read_bit() == Some(1) && r.skip_uvlc().is_none() {
                return out;
            }
            decoder_model_info_present_flag = r.read_bit().unwrap_or(0);
            if decoder_model_info_present_flag == 1 {
                buffer_delay_length_minus_1 = r.read_bits(5).unwrap_or(0) as usize;
                if r.read_bits(32).is_none() || r.read_bits(10).is_none() {
                    return out;
                }
            }
        }
        let initial_display_delay_present_flag = r.read_bit().unwrap_or(0);
        let Some(operating_points_cnt_minus_1) = r.read_bits(5) else {
            return out;
        };
        for i in 0..=operating_points_cnt_minus_1 {
            if r.read_bits(12).is_none() {
                return out;
            }
            let Some(seq_level_idx) = r.read_bits(5) else {
                return out;
            };
            let seq_tier = if seq_level_idx > 7 {
                r.read_bit().unwrap_or(0)
            } else {
                0
            };
            if i == 0 {
                out.seq_level_idx_0 = seq_level_idx as u8;
                out.seq_tier_0 = seq_tier;
            }
            if decoder_model_info_present_flag == 1 && r.read_bit() == Some(1) {
                let n = buffer_delay_length_minus_1 + 1;
                if r.read_bits(n).is_none() || r.read_bits(n).is_none() || r.read_bit().is_none() {
                    return out;
                }
            }
            if initial_display_delay_present_flag == 1
                && r.read_bit() == Some(1)
                && r.read_bits(4).is_none()
            {
                return out;
            }
        }
    }

    let Some(w_bits) = r.read_bits(4) else {
        return out;
    };
    let Some(h_bits) = r.read_bits(4) else {
        return out;
    };
    if r.read_bits((w_bits + 1) as usize).is_none() || r.read_bits((h_bits + 1) as usize).is_none()
    {
        return out;
    }

    if reduced_still_picture_header == 0 && r.read_bit() == Some(1) && r.read_bits(7).is_none() {
        return out;
    }
    if r.read_bits(3).is_none() {
        return out;
    }
    if reduced_still_picture_header == 0 {
        if r.read_bits(4).is_none() {
            return out;
        }
        let enable_order_hint = r.read_bit().unwrap_or(0);
        if enable_order_hint == 1 && r.read_bits(2).is_none() {
            return out;
        }
        let seq_force_screen_content_tools = if r.read_bit() == Some(1) {
            2
        } else {
            r.read_bit().unwrap_or(0)
        };
        if seq_force_screen_content_tools > 0
            && r.read_bit() == Some(0)
            && r.read_bit().is_none()
        {
            return out;
        }
        if enable_order_hint == 1 && r.read_bits(3).is_none() {
            return out;
        }
    }
    if r.read_bits(3).is_none() {
        return out;
    }

    // color_config()
    let Some(high_bitdepth) = r.read_bit() else {
        return out;
    };
    out.high_bitdepth = high_bitdepth;
    if out.seq_profile == 2 && high_bitdepth == 1 {
        out.twelve_bit = r.read_bit().unwrap_or(0);
    }
    out.monochrome = if out.seq_profile != 1 {
        r.read_bit().unwrap_or(0)
    } else {
        0
    };
    let mut cp = 2u32;
    let mut tc = 2u32;
    let mut mc = 2u32;
    if r.read_bit() == Some(1) {
        cp = r.read_bits(8).unwrap_or(2);
        tc = r.read_bits(8).unwrap_or(2);
        mc = r.read_bits(8).unwrap_or(2);
    }
    if out.monochrome == 1 {
        let _ = r.read_bit();
        out.chroma_subsampling_x = 1;
        out.chroma_subsampling_y = 1;
        out.chroma_sample_position = 0;
        return out;
    }
    if cp == 1 && tc == 13 && mc == 0 {
        out.chroma_subsampling_x = 0;
        out.chroma_subsampling_y = 0;
    } else {
        let _color_range = r.read_bit();
        if out.seq_profile == 0 {
            out.chroma_subsampling_x = 1;
            out.chroma_subsampling_y = 1;
        } else if out.seq_profile == 1 {
            out.chroma_subsampling_x = 0;
            out.chroma_subsampling_y = 0;
        } else if out.twelve_bit == 1 {
            out.chroma_subsampling_x = r.read_bit().unwrap_or(1);
            out.chroma_subsampling_y = if out.chroma_subsampling_x == 1 {
                r.read_bit().unwrap_or(1)
            } else {
                0
            };
        } else {
            out.chroma_subsampling_x = 1;
            out.chroma_subsampling_y = 0;
        }
        if out.chroma_subsampling_x == 1 && out.chroma_subsampling_y == 1 {
            out.chroma_sample_position = r.read_bits(2).unwrap_or(0) as u8;
        }
    }

    out
}

/// Parses an AV1 OBU access unit produced by a Media Foundation encoder:
/// extracts `OBU_SEQUENCE_HEADER` (`obu_type == 1`), detects keyframes, and strips
/// `OBU_TEMPORAL_DELIMITER` (`2`) / `OBU_PADDING` (`15`) per AV1-in-ISOBMFF.
#[must_use]
pub fn parse_av1_access_unit(raw_obus: &[u8], sample_clean_point: bool) -> ParsedAccessUnit {
    let obus = split_av1_obus(raw_obus);
    let mut out = Vec::with_capacity(raw_obus.len());
    let mut seq_header: Option<Vec<u8>> = None;
    let mut is_keyframe = sample_clean_point;

    for obu in obus {
        match obu.obu_type {
            1 => {
                // OBU_SEQUENCE_HEADER
                let normalized = normalize_obu_with_size(&obu);
                seq_header = Some(normalized.clone());
                is_keyframe = true;
                out.extend_from_slice(&normalized);
            }
            2 | 15 => {
                // Strip OBU_TEMPORAL_DELIMITER and OBU_PADDING inside ISOBMFF sample
                continue;
            }
            3 | 6 => {
                // OBU_FRAME_HEADER (3) or OBU_FRAME (6): check frame_type == KEY_FRAME (0)
                if !obu.payload.is_empty() && (obu.payload[0] & 0x80) == 0 {
                    let frame_type = (obu.payload[0] >> 5) & 0x03;
                    if frame_type == 0 {
                        is_keyframe = true;
                    }
                }
                let normalized = normalize_obu_with_size(&obu);
                out.extend_from_slice(&normalized);
            }
            _ => {
                let normalized = normalize_obu_with_size(&obu);
                out.extend_from_slice(&normalized);
            }
        }
    }

    ParsedAccessUnit {
        is_keyframe,
        length_prefixed_payload: out,
        vps: None,
        sps: seq_header,
        pps: None,
    }
}

/// Constructs an ISO BMFF `AV1CodecConfigurationRecord` (`av1C` box payload)
/// from an `OBU_SEQUENCE_HEADER` OBU.
#[must_use]
pub fn build_av1c_record(seq_header_obu: &[u8]) -> Vec<u8> {
    let params = split_av1_obus(seq_header_obu)
        .into_iter()
        .find(|o| o.obu_type == 1)
        .map(|o| parse_av1_seq_header_params(o.payload))
        .unwrap_or_default();

    let mut av1c = Vec::with_capacity(4 + seq_header_obu.len());
    av1c.push(0x81); // marker = 1 (bit 7), version = 1 (bits 6..0)
    av1c.push(((params.seq_profile & 0x07) << 5) | (params.seq_level_idx_0 & 0x1F));
    av1c.push(
        ((params.seq_tier_0 & 0x01) << 7)
            | ((params.high_bitdepth & 0x01) << 6)
            | ((params.twelve_bit & 0x01) << 5)
            | ((params.monochrome & 0x01) << 4)
            | ((params.chroma_subsampling_x & 0x01) << 3)
            | ((params.chroma_subsampling_y & 0x01) << 2)
            | (params.chroma_sample_position & 0x03),
    );
    av1c.push(0x00); // initial_presentation_delay_present = 0
    av1c.extend_from_slice(seq_header_obu);
    av1c
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
        VideoCodec::Av1 => build_av1c_record(&sps),
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

    #[test]
    fn av1_obu_parser_extracts_seq_header_strips_td_and_builds_av1c() {
        // OBU_TEMPORAL_DELIMITER (type 2, size 0): [0x12, 0x00]
        // OBU_SEQUENCE_HEADER (type 1, has_size=1, size=3): [0x0A, 0x03, 0x08, 0x00, 0x00]
        //   (seq_profile=0, still=0, reduced=1, seq_level_idx[0]=8 => 0b000_0_1_000 = 0x08 << wait: 0b00001010 = 0x0A? let's encode real 8-bit 4:2:0 reduced_still=1, level=8:
        //    seq_profile(3)=000, still(1)=0, reduced(1)=1, seq_level_idx(5)=01000 => 00001_010 00_000000 = 0x0A, 0x00)
        // OBU_FRAME (type 6, has_size=1, size=2, key_frame=0): [0x32, 0x02, 0x00, 0xAB]
        let sample = [
            0x12, 0x00, // TD
            0x0A, 0x03, 0x0A, 0x00, 0x00, // SeqHdr
            0x32, 0x02, 0x00, 0xAB, // KeyFrame
        ];

        let parsed = parse_av1_access_unit(&sample, false);
        assert!(parsed.is_keyframe);
        assert_eq!(
            parsed.sps.as_deref(),
            Some(&[0x0A, 0x03, 0x0A, 0x00, 0x00][..])
        );
        // TD must be stripped; SeqHdr + Frame kept
        assert_eq!(
            parsed.length_prefixed_payload,
            vec![0x0A, 0x03, 0x0A, 0x00, 0x00, 0x32, 0x02, 0x00, 0xAB]
        );

        let av1c = build_av1c_record(parsed.sps.as_deref().unwrap_or(&[]));
        assert_eq!(av1c[0], 0x81); // marker=1, version=1
        assert_eq!(av1c[1], 0x08); // seq_profile=0, seq_level_idx_0=8
        assert_eq!(av1c[2], 0x0C); // 8-bit 4:2:0 (subsampling_x=1, subsampling_y=1)
        assert_eq!(av1c[3], 0x00);
        assert_eq!(&av1c[4..], &[0x0A, 0x03, 0x0A, 0x00, 0x00]);
    }
}
