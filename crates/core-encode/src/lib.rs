//! Hardware video encoding (H.264 / HEVC) via Windows Media Foundation
//! (`IMFTransform` + `IMFDXGIDeviceManager`) with vendor auto-selection (NVENC / AMF / QSV).

pub mod bitstream;
pub mod mf_video;

pub use bitstream::{
    build_av1c_record, build_avcc_record, build_hvcc_record, build_video_stream_header,
    parse_annex_b_access_unit, parse_av1_access_unit, split_annex_b_nalus, split_av1_obus,
    Av1Obu, ParsedAccessUnit,
};
pub use core_types::{EncodeError, MediaPacket, VideoSettings, VideoStreamHeader};
pub use mf_video::{
    detect_available_hardware_codecs, EncoderOutput, MfHardwareEncoder, MfVideoEncoderConfig,
};
