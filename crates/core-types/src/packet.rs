//! Encoded media packet contracts, stream initialization headers, and channel bounds.

use crate::settings::VideoCodec;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Bounded capacity between WGC frame acquisition and GPU VideoProcessor + Encoder.
/// Keeps latency low and VRAM surface pools bounded under GPU backpressure.
pub const VIDEO_FRAME_CHANNEL_CAPACITY: usize = 4;

/// Bounded capacity between WASAPI capture/resampler and AAC MFT encoder.
pub const AUDIO_CHUNK_CHANNEL_CAPACITY: usize = 64;

/// Bounded capacity between hardware encoders (video + audio) and PacketRouter.
pub const ENCODED_PACKET_CHANNEL_CAPACITY: usize = 256;

/// Bounded capacity between PacketRouter and the disk fMP4 session writer.
/// If disk I/O stalls, PacketRouter drops frames cleanly instead of leaking RAM.
pub const DISK_MUX_CHANNEL_CAPACITY: usize = 512;

/// Logical elementary stream identifier inside the pipeline and MP4 container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    /// Hardware-encoded H.264 or HEVC elementary video stream (MP4 Track 1).
    Video,
    /// Real-time synchronized mix of enabled audio sources (`tkhd flags = 0x03`, MP4 Track 2).
    AudioMixed,
    /// Clean system/game WASAPI loopback encoded to 48 kHz Stereo AAC (`tkhd flags = 0x02`, MP4 Track 3).
    AudioGame,
    /// Clean microphone WASAPI capture (post-denoise) encoded to 48 kHz Stereo AAC (`tkhd flags = 0x02`, MP4 Track 4).
    AudioMic,
    /// Optional secondary WASAPI output loopback (e.g., Discord/Voicemod) (`tkhd flags = 0x02`, MP4 Track 5).
    AudioSecondary,
    /// Dynamic per-application WASAPI process loopback stem (`tkhd flags = 0x02`, MP4 Track 6+).
    AudioApp(u8),
}

impl TrackKind {
    /// Returns the 1-based MP4 track identifier.
    #[must_use]
    pub const fn mp4_track_id(self) -> u32 {
        match self {
            Self::Video => 1,
            Self::AudioMixed => 2,
            Self::AudioGame => 3,
            Self::AudioMic => 4,
            Self::AudioSecondary => 5,
            Self::AudioApp(idx) => 6 + (idx as u32),
        }
    }

    /// Returns `true` if this track carries an audio stream.
    #[must_use]
    pub const fn is_audio(self) -> bool {
        matches!(
            self,
            Self::AudioMixed
                | Self::AudioGame
                | Self::AudioMic
                | Self::AudioSecondary
                | Self::AudioApp(_)
        )
    }
}

/// GPU surface pixel format along the zero-copy D3D11 video pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PixelFormat {
    Bgra8Unorm,
    Rgba16FloatHdr,
    Nv12,
}

/// Lightweight GPU texture descriptor passed alongside the D3D11 texture handle (never CPU pixels).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuFrameDescriptor {
    /// Monotonic capture timestamp in microseconds (`MonotonicClock`).
    pub pts_us: i64,
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub is_hdr: bool,
    /// DXGI adapter LUID packed into `u64` to guarantee capture and encoder share the same GPU.
    pub adapter_luid: u64,
    pub frame_number: u64,
}

/// Codec-specific initialization headers needed to start writing an MP4 or rebase a GOP slice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoStreamHeader {
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    /// Sequence Parameter Set NAL unit (Annex-B stripped).
    pub sps: Vec<u8>,
    /// Picture Parameter Set NAL unit (Annex-B stripped).
    pub pps: Vec<u8>,
    /// Video Parameter Set NAL unit (for HEVC only).
    pub vps: Option<Vec<u8>>,
    /// Pre-serialized `avcC` (H.264) or `hvcC` (HEVC) configuration box payload.
    pub avcc_or_hvcc: Vec<u8>,
}

/// AAC stream configuration header (`AudioSpecificConfig` for MP4 `esds` atom).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioStreamHeader {
    pub track: TrackKind,
    pub sample_rate: u32,
    pub channels: u16,
    pub bitrate_bps: u32,
    pub audio_specific_config: Vec<u8>,
}

/// Combined elementary stream headers cached in both `GopRingBuffer` and `Fmp4DiskWriter`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamHeaders {
    pub video: VideoStreamHeader,
    pub audio_mixed: Option<AudioStreamHeader>,
    pub audio_game: Option<AudioStreamHeader>,
    pub audio_mic: Option<AudioStreamHeader>,
    pub audio_secondary: Option<AudioStreamHeader>,
    /// Dynamic per-application audio track headers `(slot_idx, display_label, header)`.
    #[serde(default)]
    pub app_audio: Vec<(u8, String, AudioStreamHeader)>,
}

/// Compressed encoded packet fanned out by `PacketRouter` via cheap `Arc<[u8]>` reference counting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaPacket {
    pub track: TrackKind,
    /// Presentation timestamp in microseconds on the shared monotonic clock.
    pub pts_us: i64,
    /// Decoding timestamp in microseconds on the shared monotonic clock.
    pub dts_us: i64,
    /// Packet duration in microseconds.
    pub duration_us: u32,
    /// `true` for IDR video keyframes (and always `true` for AAC audio frames).
    pub is_keyframe: bool,
    /// Monotonically increasing per-track packet sequence number.
    pub sequence: u64,
    /// Compressed bitstream bytes (AVCC/HVCC NALUs for video, raw AAC frame for audio).
    pub payload: Arc<[u8]>,
}

impl MediaPacket {
    /// Creates a new encoded packet from an owned byte vector converted once into `Arc<[u8]>`.
    #[must_use]
    pub fn new(
        track: TrackKind,
        pts_us: i64,
        dts_us: i64,
        duration_us: u32,
        is_keyframe: bool,
        sequence: u64,
        payload: Vec<u8>,
    ) -> Self {
        Self {
            track,
            pts_us,
            dts_us,
            duration_us,
            is_keyframe,
            sequence,
            payload: Arc::from(payload),
        }
    }

    /// Returns a cloned packet handle with `pts_us` and `dts_us` shifted so `base_pts_us` becomes `t = 0`.
    /// Clones only the `Arc<[u8]>` pointer without copying payload bytes.
    #[must_use]
    pub fn with_rebased_timestamps(&self, base_pts_us: i64) -> Self {
        Self {
            track: self.track,
            pts_us: self.pts_us.saturating_sub(base_pts_us),
            dts_us: self.dts_us.saturating_sub(base_pts_us),
            duration_us: self.duration_us,
            is_keyframe: self.is_keyframe,
            sequence: self.sequence,
            payload: Arc::clone(&self.payload),
        }
    }

    /// Returns the compressed payload size in bytes.
    #[must_use]
    pub fn size_bytes(&self) -> usize {
        self.payload.len()
    }
}
