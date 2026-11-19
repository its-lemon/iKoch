//! Native Windows Media Foundation (`MFCreateSourceReaderFromURL`) verification helper.
//!
//! Confirms that Windows' own MP4 demuxer and AAC decoder (used by Windows «Кино и ТВ» /
//! Movies & TV and File Explorer previews) can open a recorded `.mp4` and decode audio samples.

use crate::resampler::linear_to_dbfs;
use core_types::AudioError;
use std::path::Path;
use windows::core::HSTRING;
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::Media::MediaFoundation::{
    IMFMediaType, IMFSample, MFCreateMediaType, MFCreateSourceReaderFromURL, MFMediaType_Audio,
    MFShutdown, MFStartup, MFAudioFormat_PCM, MFSTARTUP_NOSOCKET, MF_MT_AUDIO_BITS_PER_SAMPLE,
    MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE,
    MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READER_FIRST_AUDIO_STREAM, MF_VERSION,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

/// Decoded audio diagnostics reported by native Windows Media Foundation Source Reader.
#[derive(Debug, Clone)]
pub struct MfAudioVerifyReport {
    pub sample_rate_hz: u32,
    pub channels: u32,
    pub decoded_frames: u64,
    pub decoded_packets: u64,
    pub duration_sec: f64,
    pub peak_linear: f32,
    pub peak_dbfs: f32,
}

struct MfSessionGuard;

impl Drop for MfSessionGuard {
    fn drop(&mut self) {
        let _ = unsafe { MFShutdown() };
    }
}

/// Opens `mp4_path` with `MFCreateSourceReaderFromURL`, configures the first active audio stream
/// to decode into 16-bit PCM, and drains decoded audio samples to verify playback compatibility
/// with Windows «Кино и ТВ» (Movies & TV) and Explorer.
pub fn verify_mp4_audio_with_mf_source_reader(
    mp4_path: &Path,
) -> Result<MfAudioVerifyReport, AudioError> {
    let com_hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if com_hr.is_err() && com_hr != RPC_E_CHANGED_MODE {
        return Err(AudioError::WasapiInitFailed(format!(
            "CoInitializeEx failed in MF verifier: {com_hr:?}"
        )));
    }

    unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) }.map_err(|err| {
        AudioError::WasapiInitFailed(format!("MFStartup failed in MF verifier: {err}"))
    })?;
    let _mf_guard = MfSessionGuard;

    let abs_path = if mp4_path.is_absolute() {
        mp4_path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_default()
            .join(mp4_path)
    };
    let url = HSTRING::from(abs_path.as_os_str());
    let reader = unsafe { MFCreateSourceReaderFromURL(&url, None) }.map_err(|err| {
        AudioError::AacEncodeFailed(format!(
            "MFCreateSourceReaderFromURL('{}') failed: {err}",
            abs_path.display()
        ))
    })?;

    let stream_idx = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
    let target_pcm: IMFMediaType = unsafe { MFCreateMediaType() }.map_err(|err| {
        AudioError::AacEncodeFailed(format!("MFCreateMediaType failed: {err}"))
    })?;
    unsafe {
        target_pcm
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
            .map_err(|e| AudioError::AacEncodeFailed(e.to_string()))?;
        target_pcm
            .SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)
            .map_err(|e| AudioError::AacEncodeFailed(e.to_string()))?;
        target_pcm
            .SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
            .map_err(|e| AudioError::AacEncodeFailed(e.to_string()))?;
        reader
            .SetCurrentMediaType(stream_idx, None, &target_pcm)
            .map_err(|err| {
                AudioError::AacEncodeFailed(format!(
                    "MF SourceReader SetCurrentMediaType(PCM) failed for '{}': {err}",
                    abs_path.display()
                ))
            })?;
    }

    let current_mt = unsafe { reader.GetCurrentMediaType(stream_idx) }.map_err(|err| {
        AudioError::AacEncodeFailed(format!("GetCurrentMediaType failed: {err}"))
    })?;
    let sample_rate_hz = unsafe { current_mt.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND) }
        .unwrap_or(48_000);
    let channels = unsafe { current_mt.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS) }.unwrap_or(2);
    let bytes_per_frame = (channels.max(1) as usize) * size_of::<i16>();

    let mut decoded_frames: u64 = 0;
    let mut decoded_packets: u64 = 0;
    let mut max_abs: i32 = 0;

    loop {
        let mut actual_idx: u32 = 0;
        let mut flags: u32 = 0;
        let mut timestamp_hns: i64 = 0;
        let mut sample_opt: Option<IMFSample> = None;

        unsafe {
            reader.ReadSample(
                stream_idx,
                0,
                Some(&mut actual_idx),
                Some(&mut flags),
                Some(&mut timestamp_hns),
                Some(&mut sample_opt),
            )
        }
        .map_err(|err| {
            AudioError::AacEncodeFailed(format!("MF SourceReader::ReadSample failed: {err}"))
        })?;

        if let Some(sample) = sample_opt {
            if let Ok(buf) = unsafe { sample.ConvertToContiguousBuffer() } {
                let mut ptr: *mut u8 = std::ptr::null_mut();
                let mut cur_len: u32 = 0;
                if unsafe { buf.Lock(&mut ptr, None, Some(&mut cur_len)) }.is_ok()
                    && !ptr.is_null()
                    && cur_len > 0
                {
                    let byte_slice = unsafe { std::slice::from_raw_parts(ptr, cur_len as usize) };
                    let sample_count = byte_slice.len() / 2;
                    for chunk in byte_slice[..sample_count * 2].chunks_exact(2) {
                        let s = i16::from_le_bytes([chunk[0], chunk[1]]);
                        let abs = i32::from(s).abs();
                        if abs > max_abs {
                            max_abs = abs;
                        }
                    }
                    decoded_frames = decoded_frames
                        .saturating_add((byte_slice.len() / bytes_per_frame.max(2)) as u64);
                    decoded_packets = decoded_packets.saturating_add(1);
                    let _ = unsafe { buf.Unlock() };
                }
            }
        }

        if (flags & (MF_SOURCE_READERF_ENDOFSTREAM.0 as u32)) != 0 {
            break;
        }
    }

    if decoded_frames == 0 || decoded_packets == 0 {
        return Err(AudioError::AacEncodeFailed(format!(
            "MFCreateSourceReaderFromURL decoded 0 audio frames from '{}'",
            abs_path.display()
        )));
    }

    let peak_linear = ((max_abs as f32) / 32_767.0).min(1.0);
    let duration_sec = (decoded_frames as f64) / f64::from(sample_rate_hz.max(1));

    Ok(MfAudioVerifyReport {
        sample_rate_hz,
        channels,
        decoded_frames,
        decoded_packets,
        duration_sec,
        peak_linear,
        peak_dbfs: linear_to_dbfs(peak_linear),
    })
}
