//! Hardware H.264 / HEVC encoding via Windows Media Foundation (`IMFTransform` +
//! `IMFDXGIDeviceManager`) using zero-copy NV12 `ID3D11Texture2D` input samples.

use crate::bitstream::{
    build_video_stream_header, parse_annex_b_access_unit, split_annex_b_nalus,
};
use core_capture::GpuTextureFrame;
use core_types::{
    hns_to_us, us_to_hns, EncodeError, GpuVendorPreference, MediaPacket, RateControlMode,
    TrackKind, VideoCodec, VideoStreamHeader,
};
use std::ffi::OsString;
use std::mem::ManuallyDrop;
use std::os::windows::ffi::OsStringExt;
use tracing::{debug, info, warn};
use windows::core::{Interface, GUID, PWSTR};
use windows::Win32::Foundation::{RPC_E_CHANGED_MODE, VARIANT_TRUE};
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::{DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET};
use windows::Win32::Media::MediaFoundation::{
    eAVEncCommonRateControlMode_CBR, eAVEncCommonRateControlMode_PeakConstrainedVBR,
    eAVEncCommonRateControlMode_Quality, eAVEncH264VProfile_High, IMFActivate, IMFDXGIDeviceManager,
    IMFMediaBuffer, IMFMediaEventGenerator, IMFSample, IMFTransform, MFCreateDXGIDeviceManager,
    MFCreateDXGISurfaceBuffer, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
    MFMediaType_Video, MFShutdown, MFStartup, MFTEnumEx, MFVideoFormat_H264, MFVideoFormat_HEVC,
    MFVideoFormat_NV12, MFVideoInterlace_Progressive, CODECAPI_AVEncCommonBufferSize,
    CODECAPI_AVEncCommonLowLatency, CODECAPI_AVEncCommonMaxBitRate,
    CODECAPI_AVEncCommonMeanBitRate, CODECAPI_AVEncCommonRateControlMode,
    CODECAPI_AVEncCommonRealTime, CODECAPI_AVEncH264CABACEnable,
    CODECAPI_AVEncMPVDefaultBPictureCount, CODECAPI_AVEncMPVGOPSize,
    CODECAPI_AVEncVideoForceKeyFrame, ICodecAPI, METransformDrainComplete, METransformHaveOutput,
    METransformNeedInput, MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG, MFT_ENUM_FLAG_ASYNCMFT,
    MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_FLAG_SORTANDFILTER, MFT_ENUM_FLAG_SYNCMFT,
    MFT_FRIENDLY_NAME_Attribute, MFT_MESSAGE_COMMAND_DRAIN, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING,
    MFT_MESSAGE_NOTIFY_END_OF_STREAM, MFT_MESSAGE_NOTIFY_START_OF_STREAM,
    MFT_MESSAGE_SET_D3D_MANAGER, MFT_OUTPUT_DATA_BUFFER, MFT_OUTPUT_STREAM_INFO,
    MFT_OUTPUT_STREAM_PROVIDES_SAMPLES, MFT_REGISTER_TYPE_INFO, MF_EVENT_FLAG_NONE,
    MF_EVENT_FLAG_NO_WAIT, MF_E_NOTACCEPTING, MF_E_NO_EVENTS_AVAILABLE,
    MF_E_TRANSFORM_NEED_MORE_INPUT, MF_E_TRANSFORM_STREAM_CHANGE, MF_MT_AVG_BITRATE,
    MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE,
    MF_MT_MAX_KEYFRAME_SPACING, MF_MT_MPEG2_PROFILE, MF_MT_MPEG_SEQUENCE_HEADER,
    MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_TRANSFORM_ASYNC, MF_TRANSFORM_ASYNC_UNLOCK,
    MF_VERSION, MFSTARTUP_NOSOCKET,
};
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};
use windows::Win32::System::Variant::{VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_BOOL, VT_UI4};

/// Configuration for creating or reconfiguring [`MfHardwareEncoder`].
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

/// Zero-copy Windows Media Foundation hardware video encoder (NVENC / AMF / QSV).
pub struct MfHardwareEncoder {
    config: MfVideoEncoderConfig,
    transform: IMFTransform,
    event_gen: Option<IMFMediaEventGenerator>,
    codec_api: Option<ICodecAPI>,
    _dxgi_manager: IMFDXGIDeviceManager,
    _reset_token: u32,
    is_async: bool,
    pending_need_input: u32,
    mft_provides_samples: bool,
    output_buf_size: u32,
    cached_vps: Option<Vec<u8>>,
    cached_sps: Option<Vec<u8>>,
    cached_pps: Option<Vec<u8>>,
    header_emitted: bool,
    sequence: u64,
    last_keyframe_pts_us: Option<i64>,
    last_forced_keyframe_pts_us: Option<i64>,
    frame_duration_us: u32,
    friendly_name: String,
    mf_started: bool,
}

impl MfHardwareEncoder {
    /// Initializes Media Foundation, binds `d3d_device`, selects a hardware MFT, and starts streaming.
    pub fn new(d3d_device: &ID3D11Device, config: MfVideoEncoderConfig) -> Result<Self, EncodeError> {
        let com_hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if com_hr.is_err() && com_hr != RPC_E_CHANGED_MODE {
            warn!("CoInitializeEx warning on encoder thread: {com_hr:?}");
        }

        unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) }
            .map_err(|err| EncodeError::MftFailure(format!("MFStartup failed: {err}")))?;

        match Self::init_inner(d3d_device, config) {
            Ok(enc) => Ok(enc),
            Err(err) => {
                let _ = unsafe { MFShutdown() };
                Err(err)
            }
        }
    }

    fn init_inner(
        d3d_device: &ID3D11Device,
        mut config: MfVideoEncoderConfig,
    ) -> Result<Self, EncodeError> {
        config.width = (config.width.max(2)) & !1;
        config.height = (config.height.max(2)) & !1;
        config.fps = config.fps.clamp(15, 240);
        config.gop_seconds = config.gop_seconds.clamp(1, 2);

        let (transform, friendly_name) = activate_encoder_mft(config.codec, config.vendor)?;
        info!(
            "Activated Media Foundation video encoder '{}' ({:?}, {}x{} @ {} fps, {} kbps)",
            friendly_name,
            config.codec,
            config.width,
            config.height,
            config.fps,
            config.bitrate_kbps
        );

        // Unlock async MFTs (NVENC / AMF / QSV are usually asynchronous).
        let attrs = unsafe { transform.GetAttributes() }.ok();
        let is_async = attrs
            .as_ref()
            .and_then(|a| unsafe { a.GetUINT32(&MF_TRANSFORM_ASYNC) }.ok())
            .unwrap_or(0)
            != 0;
        if is_async {
            if let Some(a) = attrs.as_ref() {
                unsafe { a.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1) }.map_err(|err| {
                    EncodeError::MftFailure(format!("MF_TRANSFORM_ASYNC_UNLOCK failed: {err}"))
                })?;
            }
        }

        let event_gen = if is_async {
            Some(transform.cast::<IMFMediaEventGenerator>().map_err(|err| {
                EncodeError::MftFailure(format!("Query IMFMediaEventGenerator failed: {err}"))
            })?)
        } else {
            None
        };

        // Bind shared D3D11 device so MFT accepts DXGI surface buffers directly in VRAM.
        let mut reset_token: u32 = 0;
        let mut dev_mgr: Option<IMFDXGIDeviceManager> = None;
        unsafe { MFCreateDXGIDeviceManager(&mut reset_token, &mut dev_mgr) }.map_err(|err| {
            EncodeError::MftFailure(format!("MFCreateDXGIDeviceManager failed: {err}"))
        })?;
        let dxgi_manager = dev_mgr.ok_or_else(|| {
            EncodeError::MftFailure("MFCreateDXGIDeviceManager returned null".to_string())
        })?;
        unsafe { dxgi_manager.ResetDevice(d3d_device, reset_token) }
            .map_err(|err| EncodeError::MftFailure(format!("ResetDevice failed: {err}")))?;

        unsafe {
            transform.ProcessMessage(
                MFT_MESSAGE_SET_D3D_MANAGER,
                dxgi_manager.as_raw() as usize,
            )
        }
        .map_err(|err| {
            EncodeError::MftFailure(format!("MFT_MESSAGE_SET_D3D_MANAGER failed: {err}"))
        })?;

        // Configure ICodecAPI BEFORE SetOutputType (NVIDIA NVENC MFT locks static rate-control
        // settings during SetOutputType), then verify/re-apply after SetOutputType and SetInputType.
        let codec_api = match transform.cast::<ICodecAPI>() {
            Ok(api) => {
                configure_codec_api(&api, &config, "pre-SetOutputType");
                Some(api)
            }
            Err(err) => {
                warn!("Encoder MFT '{}' does not expose ICodecAPI: {err}", friendly_name);
                None
            }
        };

        // Configure Output Media Type first, then Input Media Type (required by MFT contract).
        let output_type = build_output_media_type(&config)?;
        unsafe { transform.SetOutputType(0, &output_type, 0) }.map_err(|err| {
            EncodeError::InvalidMediaType(format!("SetOutputType failed: {err}"))
        })?;

        if let Some(api) = codec_api.as_ref() {
            verify_or_reapply_codec_api(api, &config, "post-SetOutputType");
        }

        let input_type = build_input_media_type(&config)?;
        unsafe { transform.SetInputType(0, &input_type, 0) }.map_err(|err| {
            EncodeError::InvalidMediaType(format!("SetInputType failed: {err}"))
        })?;

        if let Some(api) = codec_api.as_ref() {
            verify_or_reapply_codec_api(api, &config, "post-SetInputType");
        }

        let stream_info: MFT_OUTPUT_STREAM_INFO =
            unsafe { transform.GetOutputStreamInfo(0) }.map_err(|err| {
                EncodeError::MftFailure(format!("GetOutputStreamInfo failed: {err}"))
            })?;
        let mft_provides_samples =
            (stream_info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32) != 0;
        let output_buf_size = stream_info.cbSize.max(1024 * 1024);

        unsafe {
            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .map_err(|err| {
                    EncodeError::MftFailure(format!("NOTIFY_BEGIN_STREAMING failed: {err}"))
                })?;
            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .map_err(|err| {
                    EncodeError::MftFailure(format!("NOTIFY_START_OF_STREAM failed: {err}"))
                })?;
        }

        let mut enc = Self {
            frame_duration_us: (1_000_000 / config.fps.max(1)),
            config,
            transform,
            event_gen,
            codec_api,
            _dxgi_manager: dxgi_manager,
            _reset_token: reset_token,
            is_async,
            pending_need_input: 0,
            mft_provides_samples,
            output_buf_size,
            cached_vps: None,
            cached_sps: None,
            cached_pps: None,
            header_emitted: false,
            sequence: 0,
            last_keyframe_pts_us: None,
            last_forced_keyframe_pts_us: None,
            friendly_name,
            mf_started: true,
        };

        enc.extract_sequence_header_from_mft();
        enc.force_keyframe();
        Ok(enc)
    }

    /// Returns the friendly name of the selected hardware encoder MFT.
    #[must_use]
    pub fn friendly_name(&self) -> &str {
        &self.friendly_name
    }

    /// Requests the encoder to emit an IDR keyframe on the very next frame.
    pub fn force_keyframe(&self) {
        if let Some(api) = self.codec_api.as_ref() {
            let val = variant_u32(1);
            if let Err(err) = unsafe { api.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &val) } {
                warn!("ICodecAPI::SetValue(CODECAPI_AVEncVideoForceKeyFrame) failed: {err}");
            }
        }
    }

    /// Encodes one VRAM NV12 [`GpuTextureFrame`] strictly via DXGI surface buffer (zero CPU readback).
    pub fn encode_gpu_frame(
        &mut self,
        frame: &GpuTextureFrame,
    ) -> Result<Vec<EncoderOutput>, EncodeError> {
        let mut outputs = Vec::new();

        if self.is_async {
            self.pump_async_events(&mut outputs, false)?;
            while self.pending_need_input == 0 {
                self.pump_async_events(&mut outputs, true)?;
            }
            self.pending_need_input = self.pending_need_input.saturating_sub(1);
        }

        let gop_interval_us = i64::from(self.config.gop_seconds.max(1)) * 1_000_000;
        let should_force_idr = match self.last_keyframe_pts_us {
            None => self.last_forced_keyframe_pts_us.is_none(),
            Some(last_idr_pts) => {
                let since_idr = frame.descriptor.pts_us.saturating_sub(last_idr_pts);
                let since_force = self
                    .last_forced_keyframe_pts_us
                    .map_or(i64::MAX, |f| frame.descriptor.pts_us.saturating_sub(f));
                since_idr >= gop_interval_us && since_force >= gop_interval_us / 2
            }
        };
        if should_force_idr {
            self.force_keyframe();
            self.last_forced_keyframe_pts_us = Some(frame.descriptor.pts_us);
        }

        let buffer = unsafe {
            MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, &frame.texture, 0, false)
        }
        .map_err(|err| map_mf_err("MFCreateDXGISurfaceBuffer", err))?;

        let sample = unsafe { MFCreateSample() }
            .map_err(|err| map_mf_err("MFCreateSample", err))?;
        unsafe {
            sample
                .AddBuffer(&buffer)
                .map_err(|err| map_mf_err("IMFSample::AddBuffer", err))?;
            sample
                .SetSampleTime(us_to_hns(frame.descriptor.pts_us))
                .map_err(|err| map_mf_err("SetSampleTime", err))?;
            sample
                .SetSampleDuration(us_to_hns(i64::from(self.frame_duration_us)))
                .map_err(|err| map_mf_err("SetSampleDuration", err))?;
        }

        match unsafe { self.transform.ProcessInput(0, &sample, 0) } {
            Ok(()) => {}
            Err(err) if err.code() == MF_E_NOTACCEPTING => {
                // Drain output first and retry once if sync MFT backpressured.
                self.drain_sync_outputs(&mut outputs)?;
                unsafe { self.transform.ProcessInput(0, &sample, 0) }
                    .map_err(|e| map_mf_err("ProcessInput(retry)", e))?;
            }
            Err(err) => return Err(map_mf_err("ProcessInput", err)),
        }

        if self.is_async {
            self.pump_async_events(&mut outputs, false)?;
        } else {
            self.drain_sync_outputs(&mut outputs)?;
        }

        Ok(outputs)
    }

    /// Drains any remaining encoded frames from the MFT at end-of-stream or before target reset.
    pub fn flush(&mut self) -> Result<Vec<EncoderOutput>, EncodeError> {
        let mut outputs = Vec::new();
        let _ = unsafe {
            self.transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0)
        };
        let _ = unsafe { self.transform.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0) };

        if self.is_async {
            if let Some(gen) = self.event_gen.clone() {
                for _ in 0..64 {
                    match unsafe { gen.GetEvent(MF_EVENT_FLAG_NONE) } {
                        Ok(evt) => {
                            let ev_type = unsafe { evt.GetType() }.unwrap_or(0);
                            if ev_type == METransformHaveOutput.0 as u32 {
                                self.pull_single_output(&mut outputs)?;
                            } else if ev_type == METransformDrainComplete.0 as u32 {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
        } else {
            let _ = self.drain_sync_outputs(&mut outputs);
        }

        Ok(outputs)
    }

    fn pump_async_events(
        &mut self,
        outputs: &mut Vec<EncoderOutput>,
        wait_for_one: bool,
    ) -> Result<(), EncodeError> {
        let Some(gen) = self.event_gen.clone() else {
            return Ok(());
        };

        let mut flags = if wait_for_one {
            MF_EVENT_FLAG_NONE
        } else {
            MF_EVENT_FLAG_NO_WAIT
        };

        loop {
            match unsafe { gen.GetEvent(flags) } {
                Ok(evt) => {
                    flags = MF_EVENT_FLAG_NO_WAIT;
                    let ev_type = unsafe { evt.GetType() }.unwrap_or(0);
                    if ev_type == METransformNeedInput.0 as u32 {
                        self.pending_need_input = self.pending_need_input.saturating_add(1);
                    } else if ev_type == METransformHaveOutput.0 as u32 {
                        self.pull_single_output(outputs)?;
                    } else if ev_type == METransformDrainComplete.0 as u32 {
                        break;
                    }
                    if wait_for_one && self.pending_need_input > 0 {
                        // Also drain any immediately ready output events before returning.
                        continue;
                    }
                }
                Err(err) if err.code() == MF_E_NO_EVENTS_AVAILABLE => break,
                Err(err) => return Err(map_mf_err("IMFMediaEventGenerator::GetEvent", err)),
            }
        }
        Ok(())
    }

    fn drain_sync_outputs(&mut self, outputs: &mut Vec<EncoderOutput>) -> Result<(), EncodeError> {
        loop {
            match self.pull_single_output(outputs) {
                Ok(true) => continue,
                Ok(false) => break,
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }

    fn pull_single_output(&mut self, outputs: &mut Vec<EncoderOutput>) -> Result<bool, EncodeError> {
        let provided_sample = if self.mft_provides_samples {
            None
        } else {
            let sample = unsafe { MFCreateSample() }
                .map_err(|err| map_mf_err("MFCreateSample(output)", err))?;
            let buf = unsafe { MFCreateMemoryBuffer(self.output_buf_size) }
                .map_err(|err| map_mf_err("MFCreateMemoryBuffer", err))?;
            unsafe {
                sample
                    .AddBuffer(&buf)
                    .map_err(|err| map_mf_err("AddBuffer(output)", err))?;
            }
            Some(sample)
        };

        let mut out_buf = MFT_OUTPUT_DATA_BUFFER {
            dwStreamID: 0,
            pSample: ManuallyDrop::new(provided_sample),
            dwStatus: 0,
            pEvents: ManuallyDrop::new(None),
        };
        let mut status: u32 = 0;

        let res = unsafe {
            self.transform
                .ProcessOutput(0, std::slice::from_mut(&mut out_buf), &mut status)
        };

        let returned_sample = ManuallyDrop::into_inner(out_buf.pSample);
        let _returned_events = ManuallyDrop::into_inner(out_buf.pEvents);

        match res {
            Ok(()) => {}
            Err(err) if err.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(false),
            Err(err) if err.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                debug!("MFT signaled MF_E_TRANSFORM_STREAM_CHANGE; refreshing output type");
                if let Ok(new_type) = unsafe { self.transform.GetOutputCurrentType(0) } {
                    let target_bps = self.config.bitrate_kbps.saturating_mul(1000);
                    if let Err(e) = unsafe { new_type.SetUINT32(&MF_MT_AVG_BITRATE, target_bps) } {
                        warn!("SetUINT32(MF_MT_AVG_BITRATE on stream change) failed: {e}");
                    }
                    if let Err(e) = unsafe { self.transform.SetOutputType(0, &new_type, 0) } {
                        warn!("SetOutputType(on stream change) failed: {e}");
                    }
                }
                if let Some(api) = self.codec_api.as_ref() {
                    configure_codec_api(api, &self.config, "stream-change");
                }
                self.extract_sequence_header_from_mft();
                return Ok(true);
            }
            Err(err) => return Err(map_mf_err("ProcessOutput", err)),
        }

        let Some(sample) = returned_sample else {
            return Ok(false);
        };

        self.consume_encoded_sample(&sample, outputs)?;
        Ok(true)
    }

    fn consume_encoded_sample(
        &mut self,
        sample: &IMFSample,
        outputs: &mut Vec<EncoderOutput>,
    ) -> Result<(), EncodeError> {
        let pts_hns = unsafe { sample.GetSampleTime() }.unwrap_or(0);
        let dur_hns = unsafe { sample.GetSampleDuration() }
            .unwrap_or_else(|_| us_to_hns(i64::from(self.frame_duration_us)));

        let buffer: IMFMediaBuffer = unsafe { sample.GetBufferByIndex(0) }
            .map_err(|err| map_mf_err("IMFSample::GetBufferByIndex", err))?;

        let mut raw_ptr: *mut u8 = std::ptr::null_mut();
        let mut cur_len: u32 = 0;
        unsafe { buffer.Lock(&mut raw_ptr, None, Some(&mut cur_len)) }
            .map_err(|err| map_mf_err("IMFMediaBuffer::Lock", err))?;

        let annex_b_bytes = if !raw_ptr.is_null() && cur_len > 0 {
            unsafe { std::slice::from_raw_parts(raw_ptr, cur_len as usize) }.to_vec()
        } else {
            Vec::new()
        };
        let _ = unsafe { buffer.Unlock() };

        if annex_b_bytes.is_empty() {
            return Ok(());
        }

        let parsed = parse_annex_b_access_unit(self.config.codec, &annex_b_bytes);
        if let Some(vps) = parsed.vps {
            self.cached_vps = Some(vps);
        }
        if let Some(sps) = parsed.sps {
            if self.cached_sps.as_ref() != Some(&sps) {
                self.cached_sps = Some(sps);
                self.header_emitted = false;
            }
        }
        if let Some(pps) = parsed.pps {
            if self.cached_pps.as_ref() != Some(&pps) {
                self.cached_pps = Some(pps);
                self.header_emitted = false;
            }
        }

        if !self.header_emitted {
            if let (Some(sps), Some(pps)) = (self.cached_sps.clone(), self.cached_pps.clone()) {
                let header = build_video_stream_header(
                    self.config.codec,
                    self.config.width,
                    self.config.height,
                    self.config.fps,
                    self.cached_vps.clone(),
                    sps,
                    pps,
                );
                outputs.push(EncoderOutput::StreamHeader(header));
                self.header_emitted = true;
            }
        }

        if !parsed.length_prefixed_payload.is_empty() {
            let pts_us = hns_to_us(pts_hns);
            if parsed.is_keyframe {
                self.last_keyframe_pts_us = Some(pts_us);
            }
            let dur_us = u32::try_from(hns_to_us(dur_hns).max(1)).unwrap_or(self.frame_duration_us);
            self.sequence = self.sequence.saturating_add(1);
            let pkt = MediaPacket::new(
                TrackKind::Video,
                pts_us,
                pts_us, // Low-latency 0 B-frames pipeline: DTS == PTS
                dur_us,
                parsed.is_keyframe,
                self.sequence,
                parsed.length_prefixed_payload,
            );
            outputs.push(EncoderOutput::Packet(pkt));
        }

        Ok(())
    }

    fn extract_sequence_header_from_mft(&mut self) {
        let Ok(out_type) = (unsafe { self.transform.GetOutputCurrentType(0) }) else {
            return;
        };
        let mut ptr: *mut u8 = std::ptr::null_mut();
        let mut len: u32 = 0;
        if unsafe { out_type.GetAllocatedBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut ptr, &mut len) }
            .is_err()
        {
            return;
        }
        if !ptr.is_null() && len > 0 {
            let blob = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
            for nalu in split_annex_b_nalus(blob) {
                if nalu.is_empty() {
                    continue;
                }
                match self.config.codec {
                    VideoCodec::H264 => match nalu[0] & 0x1F {
                        7 => self.cached_sps = Some(nalu.to_vec()),
                        8 => self.cached_pps = Some(nalu.to_vec()),
                        _ => {}
                    },
                    VideoCodec::Hevc => match (nalu[0] >> 1) & 0x3F {
                        32 => self.cached_vps = Some(nalu.to_vec()),
                        33 => self.cached_sps = Some(nalu.to_vec()),
                        34 => self.cached_pps = Some(nalu.to_vec()),
                        _ => {}
                    },
                    VideoCodec::Av1 => {}
                }
            }
        }
        if !ptr.is_null() {
            unsafe { CoTaskMemFree(Some(ptr.cast())) };
        }
    }
}

impl Drop for MfHardwareEncoder {
    fn drop(&mut self) {
        if self.mf_started {
            let _ = unsafe { MFShutdown() };
        }
    }
}

fn activate_encoder_mft(
    codec: VideoCodec,
    vendor: GpuVendorPreference,
) -> Result<(IMFTransform, String), EncodeError> {
    let out_subtype = match codec {
        VideoCodec::H264 => MFVideoFormat_H264,
        VideoCodec::Hevc => MFVideoFormat_HEVC,
        VideoCodec::Av1 => {
            return Err(EncodeError::HardwareEncoderUnavailable {
                codec: "AV1 (GPU encoder support scheduled for Stage 5-6)".to_string(),
            })
        }
    };

    let input_info = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_NV12,
    };
    let output_info = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: out_subtype,
    };

    let flags: MFT_ENUM_FLAG = if vendor == GpuVendorPreference::SoftwareFallback {
        MFT_ENUM_FLAG(
            MFT_ENUM_FLAG_HARDWARE.0
                | MFT_ENUM_FLAG_ASYNCMFT.0
                | MFT_ENUM_FLAG_SYNCMFT.0
                | MFT_ENUM_FLAG_SORTANDFILTER.0,
        )
    } else {
        MFT_ENUM_FLAG(MFT_ENUM_FLAG_HARDWARE.0 | MFT_ENUM_FLAG_SORTANDFILTER.0)
    };

    let mut activates_ptr: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count: u32 = 0;

    unsafe {
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            flags,
            Some(&input_info),
            Some(&output_info),
            &mut activates_ptr,
            &mut count,
        )
    }
    .map_err(|err| EncodeError::MftFailure(format!("MFTEnumEx failed: {err}")))?;

    if activates_ptr.is_null() || count == 0 {
        if !activates_ptr.is_null() {
            unsafe { CoTaskMemFree(Some(activates_ptr.cast())) };
        }
        return Err(EncodeError::HardwareEncoderUnavailable {
            codec: format!("{codec:?}"),
        });
    }

    let mut candidates: Vec<(IMFActivate, String)> = Vec::with_capacity(count as usize);
    unsafe {
        let slice = std::slice::from_raw_parts_mut(activates_ptr, count as usize);
        for item in slice {
            if let Some(act) = item.take() {
                let name = read_mft_friendly_name(&act);
                candidates.push((act, name));
            }
        }
        CoTaskMemFree(Some(activates_ptr.cast()));
    }

    // Order candidates according to user GPU vendor preference.
    candidates.sort_by_key(|(_, name)| vendor_priority(name, vendor));

    for (act, name) in candidates {
        match unsafe { act.ActivateObject::<IMFTransform>() } {
            Ok(transform) => return Ok((transform, name)),
            Err(err) => {
                warn!("Failed to activate encoder MFT '{name}': {err}");
            }
        }
    }

    Err(EncodeError::HardwareEncoderUnavailable {
        codec: format!("{codec:?}"),
    })
}

fn read_mft_friendly_name(act: &IMFActivate) -> String {
    let mut pwstr = PWSTR::null();
    let mut len: u32 = 0;
    if unsafe { act.GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &mut pwstr, &mut len) }.is_ok()
        && !pwstr.is_null()
    {
        let slice = unsafe { std::slice::from_raw_parts(pwstr.0, len as usize) };
        let name = OsString::from_wide(slice).to_string_lossy().into_owned();
        unsafe { CoTaskMemFree(Some(pwstr.0.cast())) };
        name
    } else {
        "Unknown Hardware Encoder MFT".to_string()
    }
}

fn vendor_priority(name: &str, pref: GpuVendorPreference) -> u8 {
    let lower = name.to_ascii_lowercase();
    let is_nv = lower.contains("nvidia") || lower.contains("nvenc");
    let is_amd = lower.contains("amd") || lower.contains("amf") || lower.contains("radeon");
    let is_intel = lower.contains("intel") || lower.contains("qsv");

    match pref {
        GpuVendorPreference::Auto => {
            if is_nv {
                0
            } else if is_amd {
                1
            } else if is_intel {
                2
            } else {
                3
            }
        }
        GpuVendorPreference::NvidiaNvenc => {
            if is_nv {
                0
            } else {
                1
            }
        }
        GpuVendorPreference::AmdAmf => {
            if is_amd {
                0
            } else {
                1
            }
        }
        GpuVendorPreference::IntelQsv => {
            if is_intel {
                0
            } else {
                1
            }
        }
        GpuVendorPreference::SoftwareFallback => 0,
    }
}

fn build_output_media_type(
    config: &MfVideoEncoderConfig,
) -> Result<windows::Win32::Media::MediaFoundation::IMFMediaType, EncodeError> {
    let mt = unsafe { MFCreateMediaType() }
        .map_err(|err| EncodeError::InvalidMediaType(format!("MFCreateMediaType failed: {err}")))?;
    let subtype = match config.codec {
        VideoCodec::H264 => MFVideoFormat_H264,
        VideoCodec::Hevc => MFVideoFormat_HEVC,
        VideoCodec::Av1 => {
            return Err(EncodeError::HardwareEncoderUnavailable {
                codec: "AV1 (GPU encoder support scheduled for Stage 5-6)".to_string(),
            })
        }
    };

    unsafe {
        mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(|e| EncodeError::InvalidMediaType(e.to_string()))?;
        mt.SetGUID(&MF_MT_SUBTYPE, &subtype)
            .map_err(|e| EncodeError::InvalidMediaType(e.to_string()))?;
        mt.SetUINT32(&MF_MT_AVG_BITRATE, config.bitrate_kbps.saturating_mul(1000))
            .map_err(|e| EncodeError::InvalidMediaType(e.to_string()))?;
        mt.SetUINT64(&MF_MT_FRAME_SIZE, pack_u32_pair(config.width, config.height))
            .map_err(|e| EncodeError::InvalidMediaType(e.to_string()))?;
        mt.SetUINT64(&MF_MT_FRAME_RATE, pack_u32_pair(config.fps, 1))
            .map_err(|e| EncodeError::InvalidMediaType(e.to_string()))?;
        mt.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack_u32_pair(1, 1))
            .map_err(|e| EncodeError::InvalidMediaType(e.to_string()))?;
        mt.SetUINT32(
            &MF_MT_INTERLACE_MODE,
            MFVideoInterlace_Progressive.0 as u32,
        )
        .map_err(|e| EncodeError::InvalidMediaType(e.to_string()))?;
        mt.SetUINT32(
            &MF_MT_MAX_KEYFRAME_SPACING,
            config.fps.saturating_mul(config.gop_seconds),
        )
        .map_err(|e| EncodeError::InvalidMediaType(e.to_string()))?;

        let profile = match config.codec {
            VideoCodec::H264 => eAVEncH264VProfile_High.0 as u32,
            VideoCodec::Hevc | VideoCodec::Av1 => 1u32, // Main profile
        };
        if let Err(err) = mt.SetUINT32(&MF_MT_MPEG2_PROFILE, profile) {
            warn!("SetUINT32(MF_MT_MPEG2_PROFILE={profile}) failed: {err}");
        }
    }

    Ok(mt)
}

fn build_input_media_type(
    config: &MfVideoEncoderConfig,
) -> Result<windows::Win32::Media::MediaFoundation::IMFMediaType, EncodeError> {
    let mt = unsafe { MFCreateMediaType() }
        .map_err(|err| EncodeError::InvalidMediaType(format!("MFCreateMediaType failed: {err}")))?;

    unsafe {
        mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(|e| EncodeError::InvalidMediaType(e.to_string()))?;
        mt.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
            .map_err(|e| EncodeError::InvalidMediaType(e.to_string()))?;
        mt.SetUINT64(&MF_MT_FRAME_SIZE, pack_u32_pair(config.width, config.height))
            .map_err(|e| EncodeError::InvalidMediaType(e.to_string()))?;
        mt.SetUINT64(&MF_MT_FRAME_RATE, pack_u32_pair(config.fps, 1))
            .map_err(|e| EncodeError::InvalidMediaType(e.to_string()))?;
        mt.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack_u32_pair(1, 1))
            .map_err(|e| EncodeError::InvalidMediaType(e.to_string()))?;
        mt.SetUINT32(
            &MF_MT_INTERLACE_MODE,
            MFVideoInterlace_Progressive.0 as u32,
        )
        .map_err(|e| EncodeError::InvalidMediaType(e.to_string()))?;
    }

    Ok(mt)
}

fn set_codec_value(api: &ICodecAPI, guid: &GUID, name: &str, val: &VARIANT) {
    if let Err(err) = unsafe { api.SetValue(guid, val) } {
        warn!("ICodecAPI::SetValue({name}) failed: {err}");
    } else {
        debug!("ICodecAPI::SetValue({name}) applied");
    }
}

fn set_optional_codec_prop(
    api: &ICodecAPI,
    guid: &GUID,
    name: &str,
    val: &VARIANT,
    retry_u32: Option<u32>,
) {
    if unsafe { api.IsSupported(guid) }.is_err() {
        debug!("ICodecAPI property {name} not supported by MFT; skipping");
        return;
    }
    match unsafe { api.SetValue(guid, val) } {
        Ok(()) => debug!("ICodecAPI::SetValue({name}) applied"),
        Err(first_err) => {
            if let Some(u32_fallback) = retry_u32 {
                debug!(
                    "ICodecAPI::SetValue({name}) failed with primary VARIANT ({first_err}); retrying as VT_UI4={u32_fallback}"
                );
                let fallback_var = variant_u32(u32_fallback);
                if let Err(retry_err) = unsafe { api.SetValue(guid, &fallback_var) } {
                    warn!("ICodecAPI::SetValue({name}, VT_UI4={u32_fallback}) failed: {retry_err}");
                } else {
                    debug!("ICodecAPI::SetValue({name}, VT_UI4={u32_fallback}) applied");
                }
            } else {
                warn!("ICodecAPI::SetValue({name}) failed: {first_err}");
            }
        }
    }
}

fn get_codec_u32(api: &ICodecAPI, guid: &GUID) -> Option<u32> {
    let var = unsafe { api.GetValue(guid) }.ok()?;
    unsafe {
        let inner = &*var.Anonymous.Anonymous;
        if inner.vt == VT_UI4 {
            Some(inner.Anonymous.ulVal)
        } else {
            None
        }
    }
}

fn get_codec_bool(api: &ICodecAPI, guid: &GUID) -> Option<bool> {
    let var = unsafe { api.GetValue(guid) }.ok()?;
    unsafe {
        let inner = &*var.Anonymous.Anonymous;
        if inner.vt == VT_BOOL {
            Some(inner.Anonymous.boolVal == VARIANT_TRUE)
        } else {
            None
        }
    }
}

fn encoder_rate_params(config: &MfVideoEncoderConfig) -> (u32, u32, u32, u32, u32) {
    let rc_mode = match config.rate_control {
        RateControlMode::Cbr => eAVEncCommonRateControlMode_CBR.0 as u32,
        RateControlMode::Vbr => eAVEncCommonRateControlMode_PeakConstrainedVBR.0 as u32,
        RateControlMode::Cqp => eAVEncCommonRateControlMode_Quality.0 as u32,
    };
    let target_bps = config.bitrate_kbps.saturating_mul(1000);
    let max_bps = match config.rate_control {
        RateControlMode::Cbr => target_bps,
        RateControlMode::Vbr | RateControlMode::Cqp => {
            ((f64::from(target_bps) * 1.35).round() as u32).max(target_bps)
        }
    };
    // 1-second HRD/VBV buffer in bytes constrains short-term spikes without starving keyframes.
    let vbv_bytes = (target_bps / 8).max(65_536);
    let gop_frames = config.fps.saturating_mul(config.gop_seconds);
    (rc_mode, target_bps, max_bps, vbv_bytes, gop_frames)
}

fn configure_codec_api(api: &ICodecAPI, config: &MfVideoEncoderConfig, stage: &str) {
    let (rc_mode, target_bps, max_bps, vbv_bytes, gop_frames) = encoder_rate_params(config);

    // Apply structural/hint properties first so latency hints cannot override rate control.
    // Disable B-frames so PTS == DTS and replay ring buffer cuts cleanly at any IDR.
    set_optional_codec_prop(
        api,
        &CODECAPI_AVEncMPVDefaultBPictureCount,
        "CODECAPI_AVEncMPVDefaultBPictureCount",
        &variant_u32(0),
        None,
    );
    set_codec_value(
        api,
        &CODECAPI_AVEncMPVGOPSize,
        "CODECAPI_AVEncMPVGOPSize",
        &variant_u32(gop_frames),
    );
    if config.codec == VideoCodec::H264 {
        set_codec_value(
            api,
            &CODECAPI_AVEncH264CABACEnable,
            "CODECAPI_AVEncH264CABACEnable",
            &variant_bool(true),
        );
    }
    set_optional_codec_prop(
        api,
        &CODECAPI_AVEncCommonLowLatency,
        "CODECAPI_AVEncCommonLowLatency",
        &variant_bool(true),
        Some(1),
    );
    set_optional_codec_prop(
        api,
        &CODECAPI_AVEncCommonRealTime,
        "CODECAPI_AVEncCommonRealTime",
        &variant_bool(true),
        Some(1),
    );

    // Apply rate control mode and bitrates last in strict order.
    set_codec_value(
        api,
        &CODECAPI_AVEncCommonRateControlMode,
        "CODECAPI_AVEncCommonRateControlMode",
        &variant_u32(rc_mode),
    );

    match config.rate_control {
        RateControlMode::Vbr | RateControlMode::Cbr => {
            set_codec_value(
                api,
                &CODECAPI_AVEncCommonMeanBitRate,
                "CODECAPI_AVEncCommonMeanBitRate",
                &variant_u32(target_bps),
            );
            set_codec_value(
                api,
                &CODECAPI_AVEncCommonMaxBitRate,
                "CODECAPI_AVEncCommonMaxBitRate",
                &variant_u32(max_bps),
            );
            set_codec_value(
                api,
                &CODECAPI_AVEncCommonBufferSize,
                "CODECAPI_AVEncCommonBufferSize",
                &variant_u32(vbv_bytes),
            );
        }
        RateControlMode::Cqp => {
            set_codec_value(
                api,
                &CODECAPI_AVEncCommonMeanBitRate,
                "CODECAPI_AVEncCommonMeanBitRate",
                &variant_u32(target_bps),
            );
        }
    }

    info!(
        "Configured MFT ICodecAPI [{stage}] ({:?}): requested=[mode={}, mean={} bps, max={} bps, vbv={} B, gop={}f ({}s)], readback=[mode={:?}, mean={:?}, max={:?}, vbv={:?}, gop={:?}, cabac={:?}]",
        config.rate_control,
        rc_mode,
        target_bps,
        max_bps,
        vbv_bytes,
        gop_frames,
        config.gop_seconds,
        get_codec_u32(api, &CODECAPI_AVEncCommonRateControlMode),
        get_codec_u32(api, &CODECAPI_AVEncCommonMeanBitRate),
        get_codec_u32(api, &CODECAPI_AVEncCommonMaxBitRate),
        get_codec_u32(api, &CODECAPI_AVEncCommonBufferSize),
        get_codec_u32(api, &CODECAPI_AVEncMPVGOPSize),
        get_codec_bool(api, &CODECAPI_AVEncH264CABACEnable),
    );
}

fn verify_or_reapply_codec_api(api: &ICodecAPI, config: &MfVideoEncoderConfig, stage: &str) {
    let (rc_mode, target_bps, max_bps, vbv_bytes, gop_frames) = encoder_rate_params(config);

    let rb_rc = get_codec_u32(api, &CODECAPI_AVEncCommonRateControlMode);
    let rb_mean = get_codec_u32(api, &CODECAPI_AVEncCommonMeanBitRate);
    let rb_max = get_codec_u32(api, &CODECAPI_AVEncCommonMaxBitRate);
    let rb_vbv = get_codec_u32(api, &CODECAPI_AVEncCommonBufferSize);
    let rb_gop = get_codec_u32(api, &CODECAPI_AVEncMPVGOPSize);
    let rb_cabac = get_codec_bool(api, &CODECAPI_AVEncH264CABACEnable);

    info!(
        "MFT ICodecAPI [{stage}] readback: [mode={:?}, mean={:?}, max={:?}, vbv={:?}, gop={:?}, cabac={:?}]",
        rb_rc, rb_mean, rb_max, rb_vbv, rb_gop, rb_cabac
    );

    let is_constrained_rc = matches!(config.rate_control, RateControlMode::Vbr | RateControlMode::Cbr);
    let rc_drifted = rb_rc != Some(rc_mode);
    let mean_drifted = rb_mean != Some(target_bps);
    let max_drifted = is_constrained_rc && rb_max != Some(max_bps);
    let vbv_drifted = is_constrained_rc && rb_vbv != Some(vbv_bytes);
    let gop_drifted = rb_gop != Some(gop_frames);
    let cabac_drifted = config.codec == VideoCodec::H264 && rb_cabac != Some(true);

    if rc_drifted || mean_drifted || max_drifted || vbv_drifted || gop_drifted || cabac_drifted {
        if gop_drifted {
            set_codec_value(
                api,
                &CODECAPI_AVEncMPVGOPSize,
                "CODECAPI_AVEncMPVGOPSize(reapply)",
                &variant_u32(gop_frames),
            );
        }
        if cabac_drifted {
            set_codec_value(
                api,
                &CODECAPI_AVEncH264CABACEnable,
                "CODECAPI_AVEncH264CABACEnable(reapply)",
                &variant_bool(true),
            );
        }
        if rc_drifted {
            set_codec_value(
                api,
                &CODECAPI_AVEncCommonRateControlMode,
                "CODECAPI_AVEncCommonRateControlMode(reapply)",
                &variant_u32(rc_mode),
            );
        }
        if mean_drifted {
            set_codec_value(
                api,
                &CODECAPI_AVEncCommonMeanBitRate,
                "CODECAPI_AVEncCommonMeanBitRate(reapply)",
                &variant_u32(target_bps),
            );
        }
        if max_drifted {
            set_codec_value(
                api,
                &CODECAPI_AVEncCommonMaxBitRate,
                "CODECAPI_AVEncCommonMaxBitRate(reapply)",
                &variant_u32(max_bps),
            );
        }
        if vbv_drifted {
            set_codec_value(
                api,
                &CODECAPI_AVEncCommonBufferSize,
                "CODECAPI_AVEncCommonBufferSize(reapply)",
                &variant_u32(vbv_bytes),
            );
        }

        info!(
            "MFT ICodecAPI [{stage}] post-reapply readback: [mode={:?}, mean={:?}, max={:?}, vbv={:?}, gop={:?}, cabac={:?}]",
            get_codec_u32(api, &CODECAPI_AVEncCommonRateControlMode),
            get_codec_u32(api, &CODECAPI_AVEncCommonMeanBitRate),
            get_codec_u32(api, &CODECAPI_AVEncCommonMaxBitRate),
            get_codec_u32(api, &CODECAPI_AVEncCommonBufferSize),
            get_codec_u32(api, &CODECAPI_AVEncMPVGOPSize),
            get_codec_bool(api, &CODECAPI_AVEncH264CABACEnable),
        );
    }
}

const fn pack_u32_pair(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | (lo as u64)
}

fn variant_u32(val: u32) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_UI4,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 { ulVal: val },
            }),
        },
    }
}

fn variant_bool(val: bool) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_BOOL,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 {
                    boolVal: if val {
                        VARIANT_TRUE
                    } else {
                        Default::default()
                    },
                },
            }),
        },
    }
}

fn map_mf_err(op: &str, err: windows::core::Error) -> EncodeError {
    let hr = err.code();
    if hr == DXGI_ERROR_DEVICE_REMOVED || hr == DXGI_ERROR_DEVICE_RESET {
        EncodeError::DeviceResetRequired {
            hresult: hr.0 as u32,
        }
    } else {
        EncodeError::MftFailure(format!("{op} failed: {err}"))
    }
}
