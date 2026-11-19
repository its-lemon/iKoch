//! Windows Media Foundation AAC Encoder MFT (`48 kHz Stereo` raw AAC frames for MP4).

use crate::resampler::{TARGET_CHANNELS, TARGET_SAMPLE_RATE_HZ};
use core_types::{hns_to_us, us_to_hns, AudioError, AudioStreamHeader, MediaPacket, TrackKind};
use std::mem::ManuallyDrop;
use tracing::{debug, warn};
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFMediaBuffer, IMFTransform, MFCreateMediaType, MFCreateMemoryBuffer,
    MFCreateSample, MFMediaType_Audio, MFShutdown, MFStartup, MFTEnumEx, MFAudioFormat_AAC,
    MFAudioFormat_PCM, MFT_CATEGORY_AUDIO_ENCODER, MFT_ENUM_FLAG, MFT_ENUM_FLAG_ASYNCMFT,
    MFT_ENUM_FLAG_SORTANDFILTER, MFT_ENUM_FLAG_SYNCMFT, MFT_MESSAGE_COMMAND_DRAIN,
    MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, MFT_MESSAGE_NOTIFY_END_OF_STREAM,
    MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_OUTPUT_DATA_BUFFER, MFT_OUTPUT_STREAM_INFO,
    MFT_OUTPUT_STREAM_PROVIDES_SAMPLES, MFT_REGISTER_TYPE_INFO, MF_E_TRANSFORM_NEED_MORE_INPUT,
    MF_MT_AAC_PAYLOAD_TYPE, MF_MT_AUDIO_AVG_BYTES_PER_SECOND, MF_MT_AUDIO_BITS_PER_SAMPLE,
    MF_MT_AUDIO_BLOCK_ALIGNMENT, MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND,
    MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_MT_USER_DATA, MF_VERSION, MFSTARTUP_NOSOCKET,
};
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};

/// Duration of one 1024-sample AAC frame at 48 kHz in microseconds (`1024 / 48000 = 21333.33 us`).
pub const AAC_FRAME_DURATION_US: u32 = 21_333;

/// Encodes `48 kHz Stereo i16` PCM blocks into compressed AAC elementary stream packets.
pub struct MfAacEncoder {
    track: TrackKind,
    transform: IMFTransform,
    header: AudioStreamHeader,
    mft_provides_samples: bool,
    output_buf_size: u32,
    sequence: u64,
    mf_started: bool,
}

impl MfAacEncoder {
    /// Initializes Media Foundation and creates an AAC Encoder MFT for `track`.
    pub fn new(track: TrackKind, bitrate_kbps: u32) -> Result<Self, AudioError> {
        let com_hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if com_hr.is_err() && com_hr != RPC_E_CHANGED_MODE {
            warn!("CoInitializeEx warning in MfAacEncoder: {com_hr:?}");
        }

        unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) }
            .map_err(|err| AudioError::AacEncodeFailed(format!("MFStartup failed: {err}")))?;

        match Self::init_inner(track, bitrate_kbps) {
            Ok(enc) => Ok(enc),
            Err(err) => {
                let _ = unsafe { MFShutdown() };
                Err(err)
            }
        }
    }

    fn init_inner(track: TrackKind, bitrate_kbps: u32) -> Result<Self, AudioError> {
        // Windows AAC Encoder MFT supports discrete average byte rates:
        // 12000 (96 kbps), 16000 (128 kbps), 20000 (160 kbps), 24000 (192 kbps).
        let snapped_bytes_per_sec: u32 = if bitrate_kbps <= 112 {
            12_000
        } else if bitrate_kbps <= 144 {
            16_000
        } else if bitrate_kbps <= 176 {
            20_000
        } else {
            24_000
        };

        let in_info = MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Audio,
            guidSubtype: MFAudioFormat_PCM,
        };
        let out_info = MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Audio,
            guidSubtype: MFAudioFormat_AAC,
        };

        let mut activates_ptr: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count: u32 = 0;

        unsafe {
            MFTEnumEx(
                MFT_CATEGORY_AUDIO_ENCODER,
                MFT_ENUM_FLAG(
                    MFT_ENUM_FLAG_SYNCMFT.0
                        | MFT_ENUM_FLAG_ASYNCMFT.0
                        | MFT_ENUM_FLAG_SORTANDFILTER.0,
                ),
                Some(&in_info),
                Some(&out_info),
                &mut activates_ptr,
                &mut count,
            )
        }
        .map_err(|err| AudioError::AacEncodeFailed(format!("MFTEnumEx(AAC) failed: {err}")))?;

        if activates_ptr.is_null() || count == 0 {
            if !activates_ptr.is_null() {
                unsafe { CoTaskMemFree(Some(activates_ptr.cast())) };
            }
            return Err(AudioError::AacEncodeFailed(
                "No Media Foundation AAC Encoder MFT found".to_string(),
            ));
        }

        let mut transform: Option<IMFTransform> = None;
        unsafe {
            let slice = std::slice::from_raw_parts_mut(activates_ptr, count as usize);
            for slot in slice {
                if let Some(act) = slot.take() {
                    if transform.is_none() {
                        if let Ok(mft) = act.ActivateObject::<IMFTransform>() {
                            transform = Some(mft);
                        }
                    }
                }
            }
            CoTaskMemFree(Some(activates_ptr.cast()));
        }

        let transform = transform.ok_or_else(|| {
            AudioError::AacEncodeFailed("Failed to activate AAC Encoder MFT".to_string())
        })?;

        let input_type = build_pcm_input_type()?;
        let output_type = build_aac_output_type(snapped_bytes_per_sec)?;

        // Microsoft AAC Encoder MFT expects Input Type first, then Output Type.
        unsafe { transform.SetInputType(0, &input_type, 0) }.map_err(|err| {
            AudioError::AacEncodeFailed(format!("AAC SetInputType failed: {err}"))
        })?;
        unsafe { transform.SetOutputType(0, &output_type, 0) }.map_err(|err| {
            AudioError::AacEncodeFailed(format!("AAC SetOutputType failed: {err}"))
        })?;

        let stream_info: MFT_OUTPUT_STREAM_INFO =
            unsafe { transform.GetOutputStreamInfo(0) }.map_err(|err| {
                AudioError::AacEncodeFailed(format!("AAC GetOutputStreamInfo failed: {err}"))
            })?;
        let mft_provides_samples =
            (stream_info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32) != 0;
        let output_buf_size = stream_info.cbSize.max(8192);

        let asc = extract_audio_specific_config(&transform);
        debug!(
            "Initialized AAC Encoder MFT for {:?} ({} bps, ASC={:02x?})",
            track,
            snapped_bytes_per_sec * 8,
            asc
        );

        unsafe {
            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .map_err(|err| {
                    AudioError::AacEncodeFailed(format!("AAC BEGIN_STREAMING failed: {err}"))
                })?;
            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .map_err(|err| {
                    AudioError::AacEncodeFailed(format!("AAC START_OF_STREAM failed: {err}"))
                })?;
        }

        let header = AudioStreamHeader {
            track,
            sample_rate: TARGET_SAMPLE_RATE_HZ,
            channels: TARGET_CHANNELS as u16,
            bitrate_bps: snapped_bytes_per_sec * 8,
            audio_specific_config: asc,
        };

        Ok(Self {
            track,
            transform,
            header,
            mft_provides_samples,
            output_buf_size,
            sequence: 0,
            mf_started: true,
        })
    }

    /// Returns the `AudioStreamHeader` containing `AudioSpecificConfig` for MP4 `esds`.
    #[must_use]
    pub fn stream_header(&self) -> &AudioStreamHeader {
        &self.header
    }

    /// Encodes one `1024`-frame `48 kHz Stereo i16` PCM block (`2048` samples) at `pts_us`.
    pub fn encode_pcm_block(
        &mut self,
        samples_i16_stereo: &[i16],
        pts_us: i64,
    ) -> Result<Vec<MediaPacket>, AudioError> {
        let byte_len = std::mem::size_of_val(samples_i16_stereo);
        let buffer = unsafe { MFCreateMemoryBuffer(byte_len as u32) }.map_err(|err| {
            AudioError::AacEncodeFailed(format!("MFCreateMemoryBuffer failed: {err}"))
        })?;

        let mut dst_ptr: *mut u8 = std::ptr::null_mut();
        unsafe { buffer.Lock(&mut dst_ptr, None, None) }.map_err(|err| {
            AudioError::AacEncodeFailed(format!("IMFMediaBuffer::Lock failed: {err}"))
        })?;
        if !dst_ptr.is_null() && byte_len > 0 {
            let src_bytes = unsafe {
                std::slice::from_raw_parts(samples_i16_stereo.as_ptr().cast::<u8>(), byte_len)
            };
            let dst_bytes = unsafe { std::slice::from_raw_parts_mut(dst_ptr, byte_len) };
            dst_bytes.copy_from_slice(src_bytes);
        }
        let _ = unsafe { buffer.Unlock() };
        unsafe { buffer.SetCurrentLength(byte_len as u32) }.map_err(|err| {
            AudioError::AacEncodeFailed(format!("SetCurrentLength failed: {err}"))
        })?;

        let sample = unsafe { MFCreateSample() }
            .map_err(|err| AudioError::AacEncodeFailed(format!("MFCreateSample failed: {err}")))?;
        unsafe {
            sample
                .AddBuffer(&buffer)
                .map_err(|err| AudioError::AacEncodeFailed(format!("AddBuffer failed: {err}")))?;
            sample
                .SetSampleTime(us_to_hns(pts_us))
                .map_err(|err| AudioError::AacEncodeFailed(format!("SetSampleTime failed: {err}")))?;
            sample
                .SetSampleDuration(us_to_hns(i64::from(AAC_FRAME_DURATION_US)))
                .map_err(|err| {
                    AudioError::AacEncodeFailed(format!("SetSampleDuration failed: {err}"))
                })?;
        }

        unsafe { self.transform.ProcessInput(0, &sample, 0) }.map_err(|err| {
            AudioError::AacEncodeFailed(format!("AAC ProcessInput failed: {err}"))
        })?;

        let mut packets = Vec::new();
        self.drain_outputs(pts_us, &mut packets)?;
        Ok(packets)
    }

    /// Flushes any remaining buffered AAC frame from the MFT.
    pub fn flush(&mut self, fallback_pts_us: i64) -> Result<Vec<MediaPacket>, AudioError> {
        let _ = unsafe {
            self.transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0)
        };
        let _ = unsafe { self.transform.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0) };
        let mut packets = Vec::new();
        self.drain_outputs(fallback_pts_us, &mut packets)?;
        Ok(packets)
    }

    fn drain_outputs(
        &mut self,
        fallback_pts_us: i64,
        packets: &mut Vec<MediaPacket>,
    ) -> Result<(), AudioError> {
        loop {
            let provided_sample = if self.mft_provides_samples {
                None
            } else {
                let s = unsafe { MFCreateSample() }.map_err(|err| {
                    AudioError::AacEncodeFailed(format!("MFCreateSample(out) failed: {err}"))
                })?;
                let b = unsafe { MFCreateMemoryBuffer(self.output_buf_size) }.map_err(|err| {
                    AudioError::AacEncodeFailed(format!("MFCreateMemoryBuffer(out) failed: {err}"))
                })?;
                unsafe {
                    s.AddBuffer(&b).map_err(|err| {
                        AudioError::AacEncodeFailed(format!("AddBuffer(out) failed: {err}"))
                    })?;
                }
                Some(s)
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
            let _ = ManuallyDrop::into_inner(out_buf.pEvents);

            match res {
                Ok(()) => {}
                Err(err) if err.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => break,
                Err(err) => {
                    return Err(AudioError::AacEncodeFailed(format!(
                        "AAC ProcessOutput failed: {err}"
                    )))
                }
            }

            let Some(sample) = returned_sample else {
                break;
            };

            let pts_us = unsafe { sample.GetSampleTime() }
                .map(hns_to_us)
                .unwrap_or(fallback_pts_us);
            let dur_us = unsafe { sample.GetSampleDuration() }
                .map(|d| u32::try_from(hns_to_us(d).max(1)).unwrap_or(AAC_FRAME_DURATION_US))
                .unwrap_or(AAC_FRAME_DURATION_US);

            let buffer: IMFMediaBuffer = unsafe { sample.GetBufferByIndex(0) }.map_err(|err| {
                AudioError::AacEncodeFailed(format!("GetBufferByIndex failed: {err}"))
            })?;

            let mut raw_ptr: *mut u8 = std::ptr::null_mut();
            let mut cur_len: u32 = 0;
            unsafe { buffer.Lock(&mut raw_ptr, None, Some(&mut cur_len)) }.map_err(|err| {
                AudioError::AacEncodeFailed(format!("Output buffer lock failed: {err}"))
            })?;

            let payload = if !raw_ptr.is_null() && cur_len > 0 {
                unsafe { std::slice::from_raw_parts(raw_ptr, cur_len as usize) }.to_vec()
            } else {
                Vec::new()
            };
            let _ = unsafe { buffer.Unlock() };

            if !payload.is_empty() {
                self.sequence = self.sequence.saturating_add(1);
                packets.push(MediaPacket::new(
                    self.track,
                    pts_us,
                    pts_us,
                    dur_us,
                    true,
                    self.sequence,
                    payload,
                ));
            }
        }
        Ok(())
    }
}

impl Drop for MfAacEncoder {
    fn drop(&mut self) {
        if self.mf_started {
            let _ = unsafe { MFShutdown() };
        }
    }
}

fn build_pcm_input_type() -> Result<windows::Win32::Media::MediaFoundation::IMFMediaType, AudioError>
{
    let mt = unsafe { MFCreateMediaType() }.map_err(|err| {
        AudioError::AacEncodeFailed(format!("MFCreateMediaType(in) failed: {err}"))
    })?;
    let block_align = u32::from(TARGET_CHANNELS as u16) * 2; // 4 bytes per stereo 16-bit frame
    let avg_bytes = TARGET_SAMPLE_RATE_HZ * block_align;
    unsafe {
        mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
            .map_err(|e| AudioError::AacEncodeFailed(e.to_string()))?;
        mt.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)
            .map_err(|e| AudioError::AacEncodeFailed(e.to_string()))?;
        mt.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, TARGET_CHANNELS as u32)
            .map_err(|e| AudioError::AacEncodeFailed(e.to_string()))?;
        mt.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, TARGET_SAMPLE_RATE_HZ)
            .map_err(|e| AudioError::AacEncodeFailed(e.to_string()))?;
        mt.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
            .map_err(|e| AudioError::AacEncodeFailed(e.to_string()))?;
        mt.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, block_align)
            .map_err(|e| AudioError::AacEncodeFailed(e.to_string()))?;
        mt.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, avg_bytes)
            .map_err(|e| AudioError::AacEncodeFailed(e.to_string()))?;
    }
    Ok(mt)
}

fn build_aac_output_type(
    avg_bytes_per_sec: u32,
) -> Result<windows::Win32::Media::MediaFoundation::IMFMediaType, AudioError> {
    let mt = unsafe { MFCreateMediaType() }.map_err(|err| {
        AudioError::AacEncodeFailed(format!("MFCreateMediaType(out) failed: {err}"))
    })?;
    unsafe {
        mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
            .map_err(|e| AudioError::AacEncodeFailed(e.to_string()))?;
        mt.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)
            .map_err(|e| AudioError::AacEncodeFailed(e.to_string()))?;
        mt.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, TARGET_CHANNELS as u32)
            .map_err(|e| AudioError::AacEncodeFailed(e.to_string()))?;
        mt.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, TARGET_SAMPLE_RATE_HZ)
            .map_err(|e| AudioError::AacEncodeFailed(e.to_string()))?;
        mt.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
            .map_err(|e| AudioError::AacEncodeFailed(e.to_string()))?;
        mt.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0)
            .map_err(|e| AudioError::AacEncodeFailed(e.to_string()))?;
        mt.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, avg_bytes_per_sec)
            .map_err(|e| AudioError::AacEncodeFailed(e.to_string()))?;
    }
    Ok(mt)
}

fn extract_audio_specific_config(transform: &IMFTransform) -> Vec<u8> {
    if let Ok(out_type) = unsafe { transform.GetOutputCurrentType(0) } {
        let mut ptr: *mut u8 = std::ptr::null_mut();
        let mut len: u32 = 0;
        if unsafe { out_type.GetAllocatedBlob(&MF_MT_USER_DATA, &mut ptr, &mut len) }.is_ok()
            && !ptr.is_null()
        {
            let slice = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
            // Strip 12-byte HEAACWAVEINFO header (`MF_MT_USER_DATA`) and keep strictly the
            // 2-byte AudioSpecificConfig (`0x11 0x90` for 48 kHz Stereo AAC-LC) for MP4 `esds`.
            let asc = if slice.len() >= 14 {
                slice[12..14].to_vec()
            } else if slice.len() == 2 {
                slice.to_vec()
            } else {
                Vec::new()
            };
            unsafe { CoTaskMemFree(Some(ptr.cast())) };
            if asc.len() == 2 && asc != [0, 0] {
                return asc;
            }
        }
    }
    // Standard 48 kHz Stereo AAC-LC AudioSpecificConfig fallback (0x11, 0x90)
    vec![0x11, 0x90]
}

/// Computes exact presentation timestamp in microseconds for `frame_index` at 48 kHz.
#[must_use]
pub const fn sample_index_to_pts_us(base_pts_us: i64, emitted_frames_48k: u64) -> i64 {
    let delta = ((emitted_frames_48k as i128) * 1_000_000) / (TARGET_SAMPLE_RATE_HZ as i128);
    base_pts_us.saturating_add(delta as i64)
}

/// Computes how many 48 kHz frames should have elapsed by `now_us` since `base_pts_us`.
#[must_use]
pub const fn elapsed_48k_frames(base_pts_us: i64, now_us: i64) -> u64 {
    let elapsed_us = now_us.saturating_sub(base_pts_us);
    if elapsed_us <= 0 {
        return 0;
    }
    (((elapsed_us as u128) * (TARGET_SAMPLE_RATE_HZ as u128)) / 1_000_000) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resampler::AAC_FRAME_SAMPLES;

    #[test]
    fn mf_aac_encoder_produces_valid_header_and_packets() -> Result<(), AudioError> {
        let mut enc = MfAacEncoder::new(TrackKind::AudioGame, 192)?;
        assert_eq!(enc.stream_header().sample_rate, 48_000);
        assert_eq!(enc.stream_header().audio_specific_config.len(), 2);
        assert_eq!(enc.stream_header().audio_specific_config, vec![0x11, 0x90]);
        assert_eq!(enc.stream_header().channels, 2);
        assert!(!enc.stream_header().audio_specific_config.is_empty());

        let mut total_packets = Vec::new();
        let pcm = [0i16; AAC_FRAME_SAMPLES * TARGET_CHANNELS];
        for i in 0..10 {
            let pts_us = sample_index_to_pts_us(0, i * (AAC_FRAME_SAMPLES as u64));
            total_packets.extend(enc.encode_pcm_block(&pcm, pts_us)?);
        }
        total_packets.extend(enc.flush(200_000)?);
        assert!(
            total_packets.len() >= 8,
            "expected >= 8 AAC packets, got {}",
            total_packets.len()
        );
        assert!(total_packets
            .iter()
            .all(|p| p.track == TrackKind::AudioGame && !p.payload.is_empty()));
        Ok(())
    }
}
