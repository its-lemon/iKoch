//! PCM sample format decoding, multi-channel to stereo downmix, volume/mute scaling,
//! and synchronous FFT resampling (`rubato`) to 48 kHz Stereo (`i16` 1024-frame blocks).

use core_types::AudioError;
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
use std::collections::VecDeque;
use wasapi::SampleType;

/// Fixed output sample rate required by the AAC pipeline.
pub const TARGET_SAMPLE_RATE_HZ: u32 = 48_000;

/// Fixed stereo channel count required by the AAC pipeline.
pub const TARGET_CHANNELS: usize = 2;

/// Number of stereo frames consumed by one AAC-LC frame.
pub const AAC_FRAME_SAMPLES: usize = 1024;

/// Converts arbitrary WASAPI PCM/Float audio streams into 48 kHz Stereo `i16` 1024-frame blocks.
pub struct Stereo48kResampler {
    in_rate: u32,
    in_channels: usize,
    sample_type: SampleType,
    bits_per_sample: u16,
    valid_bits_per_sample: u16,
    block_align: usize,
    volume: f32,
    muted: bool,
    max_abs_sample: f32,
    in_stereo_queue: VecDeque<f32>,
    resampler: Option<Fft<f32>>,
    in_scratch: Vec<f32>,
    out_scratch: Vec<f32>,
}

impl Stereo48kResampler {
    /// Creates a resampler for the given WASAPI device format targeting `48_000 Hz` Stereo.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        in_rate: u32,
        in_channels: u16,
        sample_type: SampleType,
        bits_per_sample: u16,
        valid_bits_per_sample: u16,
        block_align: usize,
        volume: f32,
        muted: bool,
    ) -> Result<Self, AudioError> {
        let channels = usize::from(in_channels.max(1));
        let expected_align = channels * usize::from((bits_per_sample / 8).max(1));
        let safe_align = block_align.max(expected_align);

        let resampler = if in_rate != TARGET_SAMPLE_RATE_HZ {
            let fft = Fft::<f32>::new(
                in_rate as usize,
                TARGET_SAMPLE_RATE_HZ as usize,
                AAC_FRAME_SAMPLES,
                TARGET_CHANNELS,
                FixedSync::Output,
            )
            .map_err(|err| {
                AudioError::ResampleFailed(format!(
                    "Failed to create rubato FFT resampler ({in_rate} -> 48000): {err}"
                ))
            })?;
            Some(fft)
        } else {
            None
        };

        let max_in = resampler
            .as_ref()
            .map_or(AAC_FRAME_SAMPLES, |r| r.input_frames_max().max(AAC_FRAME_SAMPLES));

        Ok(Self {
            in_rate,
            in_channels: channels,
            sample_type,
            bits_per_sample,
            valid_bits_per_sample,
            block_align: safe_align,
            volume: volume.clamp(0.0, 2.0),
            muted,
            max_abs_sample: 0.0,
            in_stereo_queue: VecDeque::with_capacity(max_in * TARGET_CHANNELS * 4),
            resampler,
            in_scratch: vec![0.0; max_in * TARGET_CHANNELS],
            out_scratch: vec![0.0; AAC_FRAME_SAMPLES * TARGET_CHANNELS],
        })
    }

    /// Updates gain/mute state dynamically without rebuilding the resampler.
    pub fn set_volume_and_mute(&mut self, volume: f32, muted: bool) {
        self.volume = volume.clamp(0.0, 2.0);
        self.muted = muted;
    }

    /// Returns the input sample rate in Hz.
    #[must_use]
    pub const fn input_sample_rate(&self) -> u32 {
        self.in_rate
    }

    /// Pushes raw WASAPI packet bytes, decoding samples and downmixing/upmixing to stereo `f32`.
    pub fn push_raw_bytes(&mut self, bytes: &[u8]) {
        if self.block_align == 0 || bytes.is_empty() {
            return;
        }
        let frames = bytes.len() / self.block_align;
        let gain = if self.muted { 0.0 } else { self.volume };

        for frame_idx in 0..frames {
            let frame_start = frame_idx * self.block_align;
            let frame_bytes = &bytes[frame_start..frame_start + self.block_align];
            let (l, r) = decode_frame_to_stereo(
                frame_bytes,
                self.in_channels,
                self.sample_type,
                self.bits_per_sample,
                self.valid_bits_per_sample,
            );
            let sl = (l * gain).clamp(-1.0, 1.0);
            let sr = (r * gain).clamp(-1.0, 1.0);
            self.max_abs_sample = self.max_abs_sample.max(sl.abs()).max(sr.abs());
            self.in_stereo_queue.push_back(sl);
            self.in_stereo_queue.push_back(sr);
        }
    }

    /// Returns and resets the peak linear amplitude (`0.0..=1.0`) observed in pushed samples.
    pub fn take_peak_linear(&mut self) -> f32 {
        let peak = self.max_abs_sample;
        self.max_abs_sample = 0.0;
        peak
    }

    /// Injects `input_frames` of stereo silence at the input sample rate so `rubato`
    /// remains phase-continuous when WASAPI loopback pauses during silence.
    pub fn push_silence_input_frames(&mut self, input_frames: usize) {
        let samples = input_frames.saturating_mul(TARGET_CHANNELS);
        self.in_stereo_queue.extend(std::iter::repeat_n(0.0f32, samples));
    }

    /// Returns how many input frames are still buffered waiting to form a 48 kHz output frame.
    #[must_use]
    pub fn queued_input_frames(&self) -> usize {
        self.in_stereo_queue.len() / TARGET_CHANNELS
    }

    /// Discards queued PCM frames and resets peak tracking after initial stream warmup drain.
    pub fn clear(&mut self) {
        self.in_stereo_queue.clear();
        self.max_abs_sample = 0.0;
    }

    /// Produces one 1024-frame `48 kHz Stereo i16` block (`2048` samples) when enough input
    /// frames have accumulated, or `Ok(None)` if more input is needed.
    pub fn pop_48k_aac_frame(&mut self) -> Result<Option<[i16; AAC_FRAME_SAMPLES * 2]>, AudioError> {
        Ok(self.pop_48k_float_frame()?.map(|f| float_frame_to_i16(&f)))
    }

    /// Produces one 1024-frame `48 kHz Stereo f32` block (`2048` interleaved samples) when
    /// enough input frames have accumulated, or `Ok(None)` if more input is needed.
    pub fn pop_48k_float_frame(
        &mut self,
    ) -> Result<Option<[f32; AAC_FRAME_SAMPLES * 2]>, AudioError> {
        let mut out_f32 = [0.0f32; AAC_FRAME_SAMPLES * TARGET_CHANNELS];

        match self.resampler.as_mut() {
            None => {
                let needed_samples = AAC_FRAME_SAMPLES * TARGET_CHANNELS;
                if self.in_stereo_queue.len() < needed_samples {
                    return Ok(None);
                }
                for sample_dst in &mut out_f32 {
                    let s = self.in_stereo_queue.pop_front().unwrap_or(0.0);
                    *sample_dst = if s.is_finite() { s } else { 0.0 };
                }
                Ok(Some(out_f32))
            }
            Some(fft) => {
                let needed_frames = fft.input_frames_next();
                let needed_samples = needed_frames * TARGET_CHANNELS;
                if self.in_stereo_queue.len() < needed_samples {
                    return Ok(None);
                }

                if self.in_scratch.len() < needed_samples {
                    self.in_scratch.resize(needed_samples, 0.0);
                }
                for dst in &mut self.in_scratch[..needed_samples] {
                    *dst = self.in_stereo_queue.pop_front().unwrap_or(0.0);
                }

                let in_adapter = InterleavedSlice::new(
                    &self.in_scratch[..needed_samples],
                    TARGET_CHANNELS,
                    needed_frames,
                )
                .map_err(|e| AudioError::ResampleFailed(format!("Input adapter error: {e}")))?;

                let out_frames_expected = fft.output_frames_next();
                let out_samples_expected = out_frames_expected * TARGET_CHANNELS;
                if self.out_scratch.len() < out_samples_expected {
                    self.out_scratch.resize(out_samples_expected, 0.0);
                }
                let mut out_adapter = InterleavedSlice::new_mut(
                    &mut self.out_scratch[..out_samples_expected],
                    TARGET_CHANNELS,
                    out_frames_expected,
                )
                .map_err(|e| AudioError::ResampleFailed(format!("Output adapter error: {e}")))?;

                let (_, written_frames) = fft
                    .process_into_buffer(&in_adapter, &mut out_adapter, None)
                    .map_err(|e| AudioError::ResampleFailed(format!("rubato process failed: {e}")))?;

                let copy_samples = (written_frames * TARGET_CHANNELS).min(out_f32.len());
                for (dst, &src) in out_f32[..copy_samples]
                    .iter_mut()
                    .zip(&self.out_scratch[..copy_samples])
                {
                    *dst = if src.is_finite() { src } else { 0.0 };
                }

                Ok(Some(out_f32))
            }
        }
    }
}

/// Quantizes a normalized `48 kHz Stereo f32` frame into signed 16-bit PCM for AAC encoding.
#[must_use]
pub fn float_frame_to_i16(
    src: &[f32; AAC_FRAME_SAMPLES * TARGET_CHANNELS],
) -> [i16; AAC_FRAME_SAMPLES * TARGET_CHANNELS] {
    let mut dst = [0i16; AAC_FRAME_SAMPLES * TARGET_CHANNELS];
    for (out, &sample) in dst.iter_mut().zip(src.iter()) {
        *out = float_to_i16(sample);
    }
    dst
}

/// Converts a linear peak amplitude (`0.0..=1.0`) into dBFS (clamped at `-100.0 dBFS`).
#[must_use]
pub fn linear_to_dbfs(linear: f32) -> f32 {
    if linear <= 1e-5 {
        -100.0
    } else {
        20.0 * linear.log10()
    }
}

#[inline]
fn float_to_i16(sample: f32) -> i16 {
    let clamped = sample.clamp(-1.0, 1.0);
    (clamped * 32767.0).round() as i16
}

fn decode_frame_to_stereo(
    frame_bytes: &[u8],
    channels: usize,
    sample_type: SampleType,
    bits_per_sample: u16,
    valid_bits: u16,
) -> (f32, f32) {
    let bytes_per_sample = usize::from((bits_per_sample / 8).max(1));
    let mut ch_vals = [0.0f32; 8];
    let ch_count = channels.min(ch_vals.len());

    for (ch, slot) in ch_vals.iter_mut().enumerate().take(ch_count) {
        let start = ch * bytes_per_sample;
        let end = start + bytes_per_sample;
        if end > frame_bytes.len() {
            break;
        }
        *slot = decode_sample(&frame_bytes[start..end], sample_type, bits_per_sample, valid_bits);
    }

    match channels {
        0 => (0.0, 0.0),
        1 => (ch_vals[0], ch_vals[0]),
        2 => (ch_vals[0], ch_vals[1]),
        _ => {
            // Standard 5.1 / 7.1 -> Stereo downmix (FL, FR, FC, LFE, BL, BR, SL, SR)
            let fc = ch_vals[2] * std::f32::consts::FRAC_1_SQRT_2;
            let surround_l = (ch_vals[4] + ch_vals[6]) * std::f32::consts::FRAC_1_SQRT_2;
            let surround_r = (ch_vals[5] + ch_vals[7]) * std::f32::consts::FRAC_1_SQRT_2;
            let left = (ch_vals[0] + fc + surround_l).clamp(-1.0, 1.0);
            let right = (ch_vals[1] + fc + surround_r).clamp(-1.0, 1.0);
            (left, right)
        }
    }
}

#[inline]
fn decode_sample(bytes: &[u8], sample_type: SampleType, bits: u16, valid_bits: u16) -> f32 {
    match (sample_type, bits) {
        (SampleType::Float, 32) if bytes.len() >= 4 => {
            let v = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            if v.is_finite() {
                v.clamp(-1.0, 1.0)
            } else {
                0.0
            }
        }
        (SampleType::Int, 16) if bytes.len() >= 2 => {
            let v = i16::from_le_bytes([bytes[0], bytes[1]]);
            f32::from(v) / 32768.0
        }
        (SampleType::Int, 24) if bytes.len() >= 3 => {
            let sign_ext = if (bytes[2] & 0x80) != 0 { 0xFF } else { 0x00 };
            let v = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], sign_ext]);
            (v as f32) / 8_388_608.0
        }
        (SampleType::Int, 32) if bytes.len() >= 4 => {
            let v = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            if valid_bits == 24 {
                ((v >> 8) as f32) / 8_388_608.0
            } else {
                (v as f32) / 2_147_483_648.0
            }
        }
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resamples_44100_mono_to_48000_stereo_1024_blocks() -> Result<(), AudioError> {
        let mut rs = Stereo48kResampler::new(
            44_100,
            1,
            SampleType::Float,
            32,
            32,
            4,
            0.5,
            false,
        )?;

        // Generate 4410 mono f32 samples (100 ms of 440 Hz sine wave)
        let mut raw = Vec::with_capacity(4410 * 4);
        for i in 0..4410 {
            let sample = (2.0 * std::f32::consts::PI * 440.0 * (i as f32) / 44_100.0).sin();
            raw.extend_from_slice(&sample.to_le_bytes());
        }
        rs.push_raw_bytes(&raw);

        let mut produced_frames = 0usize;
        let mut peak = 0i16;
        while let Some(block) = rs.pop_48k_aac_frame()? {
            produced_frames += 1;
            for &s in &block {
                peak = peak.max(s.saturating_abs());
            }
        }

        // 100 ms at 48 kHz = 4800 samples => 4 blocks of 1024 (after FFT warmup delay)
        assert!(produced_frames >= 3, "expected >= 3 blocks, got {produced_frames}");
        // Volume 0.5 => peak around 16383
        assert!(peak > 8_000 && peak <= 17_000, "unexpected scaled peak {peak}");
        Ok(())
    }

    #[test]
    fn mute_zeroes_output_samples() -> Result<(), AudioError> {
        let mut rs =
            Stereo48kResampler::new(48_000, 2, SampleType::Int, 16, 16, 4, 1.0, true)?;
        let mut raw = Vec::with_capacity(1024 * 4);
        for _ in 0..1024 {
            raw.extend_from_slice(&12_000i16.to_le_bytes());
            raw.extend_from_slice(&(-12_000i16).to_le_bytes());
        }
        rs.push_raw_bytes(&raw);
        let block = rs.pop_48k_aac_frame()?.ok_or_else(|| {
            AudioError::ResampleFailed("expected 1 block".to_string())
        })?;
        assert!(block.iter().all(|&s| s == 0));
        Ok(())
    }

    #[test]
    fn direct_48k_downmix_bypasses_fft_and_clear_resets() -> Result<(), AudioError> {
        let mut rs =
            Stereo48kResampler::new(48_000, 2, SampleType::Int, 16, 16, 4, 1.0, false)?;
        assert!(rs.resampler.is_none());

        let mut raw = Vec::with_capacity(1024 * 4);
        for _ in 0..1024 {
            raw.extend_from_slice(&10_000i16.to_le_bytes());
            raw.extend_from_slice(&(-10_000i16).to_le_bytes());
        }
        rs.push_raw_bytes(&raw);
        assert_eq!(rs.queued_input_frames(), 1024);
        assert!(rs.pop_48k_float_frame()?.is_some());

        rs.push_raw_bytes(&raw);
        rs.clear();
        assert_eq!(rs.queued_input_frames(), 0);
        assert!(rs.pop_48k_float_frame()?.is_none());
        Ok(())
    }

    #[test]
    fn downmixes_8ch_7_1_float32_to_stereo_48k_and_recovers_after_silence() -> Result<(), AudioError> {
        // 8-channel 7.1 float32 @ 48 kHz (typical wireless gaming headset like MCHOSE V9 PRO)
        let mut rs =
            Stereo48kResampler::new(48_000, 8, SampleType::Float, 32, 32, 32, 1.0, false)?;

        // 1) Push 1024 frames of silence
        rs.push_silence_input_frames(1024);
        let silent_block = rs
            .pop_48k_float_frame()?
            .ok_or_else(|| AudioError::ResampleFailed("expected silent 7.1 block".to_string()))?;
        assert!(silent_block.iter().all(|&s| s == 0.0));

        // 2) Resume real 8-channel 7.1 audio (signal in FL, FR, FC, SL, SR)
        let mut raw = Vec::with_capacity(1024 * 32);
        for i in 0..1024 {
            let tone = 0.25 * (2.0 * std::f32::consts::PI * 440.0 * (i as f32) / 48_000.0).sin();
            let chs = [tone, tone, tone * 0.5, 0.0, 0.0, 0.0, tone * 0.5, tone * 0.5];
            for ch in chs {
                raw.extend_from_slice(&ch.to_le_bytes());
            }
        }
        rs.push_raw_bytes(&raw);
        let audio_block = rs
            .pop_48k_float_frame()?
            .ok_or_else(|| AudioError::ResampleFailed("expected active 7.1 block".to_string()))?;
        let peak = audio_block.iter().fold(0.0f32, |acc, &v| acc.max(v.abs()));
        assert!(
            peak > 0.20,
            "expected audible 7.1 -> stereo downmix after silence, got peak={peak}"
        );
        Ok(())
    }
}
