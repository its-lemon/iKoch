//! Linux hardware/software video encoding (H.264 / HEVC) via GStreamer:
//! `appsrc (NV12) -> videoconvert -> nvh264enc / vaapih264enc / x264enc -> h264parse (Annex-B) -> appsink`.

use crate::bitstream::{build_video_stream_header, parse_annex_b_access_unit};
use core_capture::GpuTextureFrame;
use core_types::{
    EncodeError, GpuVendorPreference, MediaPacket, RateControlMode, TrackKind, VideoCodec,
    VideoStreamHeader,
};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use tracing::{error, info, warn};

/// Configuration for creating or reconfiguring [`MfHardwareEncoder`] on Linux.
#[derive(Debug, Clone)]
pub struct MfVideoEncoderConfig {
    pub codec: VideoCodec,
    pub vendor: GpuVendorPreference,
    pub rate_control: RateControlMode,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub gop_seconds: u32,
}

/// Items emitted by [`MfHardwareEncoder`] during encoding or draining.
#[derive(Debug, Clone)]
pub enum EncoderOutput {
    StreamHeader(VideoStreamHeader),
    Packet(MediaPacket),
}

/// Linux GStreamer video encoder (`nvh264enc` -> `vaapih264enc` -> `x264enc` fallback).
pub struct MfHardwareEncoder {
    config: MfVideoEncoderConfig,
    pipeline: gst::Pipeline,
    appsrc: gst_app::AppSrc,
    appsink: gst_app::AppSink,
    cached_sps: Option<Vec<u8>>,
    cached_pps: Option<Vec<u8>>,
    cached_vps: Option<Vec<u8>>,
    header_emitted: bool,
    sequence: u64,
    first_pts_us: Option<i64>,
}

fn has_gst_element(name: &str) -> bool {
    gst::ElementFactory::find(name).is_some()
}

fn select_encoder_candidates(
    cfg: &MfVideoEncoderConfig,
) -> Result<Vec<(&'static str, String)>, EncodeError> {
    let gop_frames = (cfg.fps.max(1) * cfg.gop_seconds.clamp(1, 5)).max(15);
    let kbps = cfg.bitrate_kbps.clamp(500, 120_000);
    let mut candidates = Vec::new();

    match cfg.codec {
        VideoCodec::H264 => {
            if has_gst_element("nvh264enc") {
                candidates.push((
                    "nvh264enc",
                    format!(
                        "nvh264enc bitrate={kbps} gop-size={gop_frames} rc-mode=vbr preset=low-latency-hq zerolatency=true ! h264parse config-interval=1 ! video/x-h264,stream-format=byte-stream,alignment=au"
                    ),
                ));
            }
            if has_gst_element("vaapih264enc") {
                candidates.push((
                    "vaapih264enc",
                    format!(
                        "vaapih264enc bitrate={kbps} keyframe-period={gop_frames} rate-control=vbr ! h264parse config-interval=1 ! video/x-h264,stream-format=byte-stream,alignment=au"
                    ),
                ));
            }
            if has_gst_element("x264enc") {
                candidates.push((
                    "x264enc",
                    format!(
                        "x264enc bitrate={kbps} key-int-max={gop_frames} tune=zerolatency speed-preset=veryfast ! h264parse config-interval=1 ! video/x-h264,stream-format=byte-stream,alignment=au"
                    ),
                ));
            }
        }
        VideoCodec::Hevc => {
            if has_gst_element("nvh265enc") {
                candidates.push((
                    "nvh265enc",
                    format!(
                        "nvh265enc bitrate={kbps} gop-size={gop_frames} rc-mode=vbr preset=low-latency-hq zerolatency=true ! h265parse config-interval=1 ! video/x-h265,stream-format=byte-stream,alignment=au"
                    ),
                ));
            }
            if has_gst_element("vaapih265enc") {
                candidates.push((
                    "vaapih265enc",
                    format!(
                        "vaapih265enc bitrate={kbps} keyframe-period={gop_frames} rate-control=vbr ! h265parse config-interval=1 ! video/x-h265,stream-format=byte-stream,alignment=au"
                    ),
                ));
            }
            if has_gst_element("x265enc") {
                candidates.push((
                    "x265enc",
                    format!(
                        "x265enc bitrate={kbps} key-int-max={gop_frames} tune=zerolatency speed-preset=veryfast ! h265parse config-interval=1 ! video/x-h265,stream-format=byte-stream,alignment=au"
                    ),
                ));
            }
        }
        VideoCodec::Av1 => {
            return Err(EncodeError::HardwareEncoderUnavailable {
                codec: "AV1".to_string(),
            });
        }
    }

    if candidates.is_empty() {
        return Err(EncodeError::HardwareEncoderUnavailable {
            codec: format!("{:?}", cfg.codec),
        });
    }
    Ok(candidates)
}

fn drain_encoder_bus(pipeline: &gst::Pipeline) {
    let Some(bus) = pipeline.bus() else {
        return;
    };
    while let Some(msg) = bus.pop() {
        match msg.view() {
            gst::MessageView::Error(err) => {
                error!(
                    "GStreamer encoder error from {:?}: {} ({:?})",
                    err.src().map(|s| s.path_string()),
                    err.error(),
                    err.debug()
                );
            }
            gst::MessageView::Warning(w) => {
                warn!(
                    "GStreamer encoder warning from {:?}: {} ({:?})",
                    w.src().map(|s| s.path_string()),
                    w.error(),
                    w.debug()
                );
            }
            _ => {}
        }
    }
}

impl MfHardwareEncoder {
    /// Initializes a GStreamer hardware/software video encoder matching `config`.
    pub fn new(_device: &(), config: MfVideoEncoderConfig) -> Result<Self, EncodeError> {
        gst::init().map_err(|e| EncodeError::MftFailure(format!("GStreamer init: {e}")))?;

        let candidates = select_encoder_candidates(&config)?;
        let vinfo = gst_video::VideoInfo::builder(
            gst_video::VideoFormat::Nv12,
            config.width.max(2),
            config.height.max(2),
        )
        .fps(gst::Fraction::new(config.fps.max(1) as i32, 1))
        .build()
        .map_err(|e| EncodeError::InvalidMediaType(format!("VideoInfo failed: {e}")))?;
        let caps = vinfo
            .to_caps()
            .map_err(|e| EncodeError::InvalidMediaType(format!("VideoInfo caps failed: {e}")))?;

        let mut last_err = String::from("No GStreamer video encoder candidate succeeded");
        for (enc_name, enc_stage) in candidates {
            let pipeline_str = format!(
                "appsrc name=src is-live=true format=time block=false ! videoconvert ! {enc_stage} ! appsink name=sink emit-signals=false sync=false max-buffers=8 drop=false"
            );
            let Ok(el) = gst::parse::launch(&pipeline_str) else {
                warn!("GStreamer parse failed for encoder '{enc_name}'");
                continue;
            };
            let Ok(pipeline) = el.downcast::<gst::Pipeline>() else {
                continue;
            };
            let Some(appsrc) = pipeline
                .by_name("src")
                .and_then(|e| e.downcast::<gst_app::AppSrc>().ok())
            else {
                continue;
            };
            let Some(appsink) = pipeline
                .by_name("sink")
                .and_then(|e| e.downcast::<gst_app::AppSink>().ok())
            else {
                continue;
            };
            appsrc.set_caps(Some(&caps));

            match pipeline.set_state(gst::State::Playing) {
                Ok(_) => {
                    info!(
                        "Linux GStreamer encoder '{}' started ({:?}, {}x{} @ {} fps, {} kbps)",
                        enc_name, config.codec, config.width, config.height, config.fps, config.bitrate_kbps
                    );
                    return Ok(Self {
                        config,
                        pipeline,
                        appsrc,
                        appsink,
                        cached_sps: None,
                        cached_pps: None,
                        cached_vps: None,
                        header_emitted: false,
                        sequence: 0,
                        first_pts_us: None,
                    });
                }
                Err(e) => {
                    warn!("GStreamer encoder '{enc_name}' failed to start: {e}; trying next fallback");
                    drain_encoder_bus(&pipeline);
                    let _ = pipeline.set_state(gst::State::Null);
                    last_err = format!("{enc_name} start failed: {e}");
                }
            }
        }

        Err(EncodeError::MftFailure(last_err))
    }

    pub fn encode_gpu_frame(
        &mut self,
        frame: &GpuTextureFrame,
    ) -> Result<Vec<EncoderOutput>, EncodeError> {
        let pts_us = frame.descriptor.pts_us;
        let base_pts = *self.first_pts_us.get_or_insert(pts_us);
        let rel_us = pts_us.saturating_sub(base_pts).max(0) as u64;

        let mut gst_buf = gst::Buffer::from_mut_slice(frame.nv12_bytes.clone());
        if let Some(buf_ref) = gst_buf.get_mut() {
            buf_ref.set_pts(gst::ClockTime::from_useconds(rel_us));
            let dur_us = 1_000_000u64 / u64::from(self.config.fps.max(1));
            buf_ref.set_duration(gst::ClockTime::from_useconds(dur_us));
        }

        if let Err(err) = self.appsrc.push_buffer(gst_buf) {
            warn!("GStreamer appsrc push_buffer warning: {err:?}");
        }

        let mut out = Vec::new();
        self.drain_sink(&mut out);
        Ok(out)
    }

    pub fn flush(&mut self) -> Result<Vec<EncoderOutput>, EncodeError> {
        let _ = self.appsrc.end_of_stream();
        let mut out = Vec::new();
        for _ in 0..20 {
            if self.appsink.try_pull_sample(gst::ClockTime::from_mseconds(10)).is_none() {
                break;
            }
        }
        self.drain_sink(&mut out);
        let _ = self.pipeline.set_state(gst::State::Null);
        Ok(out)
    }

    fn drain_sink(&mut self, out: &mut Vec<EncoderOutput>) {
        drain_encoder_bus(&self.pipeline);
        while let Some(sample) = self.appsink.try_pull_sample(gst::ClockTime::ZERO) {
            let Some(buffer) = sample.buffer() else {
                continue;
            };
            let Ok(map) = buffer.map_readable() else {
                continue;
            };
            let annex_b = map.as_slice();
            if annex_b.is_empty() {
                continue;
            }

            let parsed = parse_annex_b_access_unit(self.config.codec, annex_b);
            if let Some(vps) = parsed.vps {
                self.cached_vps = Some(vps);
            }
            if let Some(sps) = parsed.sps {
                self.cached_sps = Some(sps);
            }
            if let Some(pps) = parsed.pps {
                self.cached_pps = Some(pps);
            }

            if !self.header_emitted {
                if let (Some(sps), Some(pps)) = (&self.cached_sps, &self.cached_pps) {
                    let hdr = build_video_stream_header(
                        self.config.codec,
                        self.config.width,
                        self.config.height,
                        self.config.fps,
                        self.cached_vps.clone(),
                        sps.clone(),
                        pps.clone(),
                    );
                    self.header_emitted = true;
                    out.push(EncoderOutput::StreamHeader(hdr));
                }
            }

            if !self.header_emitted || parsed.length_prefixed_payload.is_empty() {
                continue;
            }

            let base_pts = self.first_pts_us.unwrap_or(0);
            let pts_us = buffer
                .pts()
                .map_or(base_pts, |p| base_pts.saturating_add((p.nseconds() / 1000) as i64));
            let duration_us = buffer
                .duration()
                .map_or(1_000_000 / self.config.fps.max(1), |d| {
                    (d.nseconds() / 1000) as u32
                });
            self.sequence = self.sequence.saturating_add(1);

            out.push(EncoderOutput::Packet(MediaPacket::new(
                TrackKind::Video,
                pts_us,
                pts_us,
                duration_us,
                parsed.is_keyframe,
                self.sequence,
                parsed.length_prefixed_payload,
            )));
        }
    }
}

impl Drop for MfHardwareEncoder {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}
