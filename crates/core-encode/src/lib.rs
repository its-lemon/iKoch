//! Hardware video encoding (H.264 / HEVC) via Windows Media Foundation
//! (`IMFTransform` + `IMFDXGIDeviceManager`) with vendor auto-selection (NVENC / AMF / QSV).

pub mod bitstream;
#[cfg(windows)]
pub mod mf_video;
#[cfg(target_os = "linux")]
pub mod linux;

pub use bitstream::{
    build_avcc_record, build_hvcc_record, build_video_stream_header, parse_annex_b_access_unit,
    split_annex_b_nalus, ParsedAccessUnit,
};
pub use core_types::{EncodeError, MediaPacket, VideoSettings, VideoStreamHeader};
#[cfg(windows)]
pub use mf_video::{EncoderOutput, MfHardwareEncoder, MfVideoEncoderConfig};
#[cfg(target_os = "linux")]
pub use linux::{EncoderOutput, MfHardwareEncoder, MfVideoEncoderConfig};
