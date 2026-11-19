use std::collections::VecDeque;

use nnnoiseless::DenoiseState;

use crate::resampler::{AAC_FRAME_SAMPLES, TARGET_SAMPLE_RATE_HZ};

/// Fixed analysis/synthesis frame size expected by RNNoise at 48 kHz (10 ms = 480 samples).
pub const RNNOISE_FRAME_SIZE: usize = nnnoiseless::FRAME_SIZE;

/// RNNoise operates on PCM samples scaled to signed 16-bit range `[-32768.0, 32767.0]`.
const RNNOISE_PCM_SCALE: f32 = 32_768.0;

/// Linear threshold above which `soft_clip_sample` engages smooth hyperbolic saturation (-1.94 dBFS).
const SOFT_CLIP_KNEE: f32 = 0.80;

/// Envelope detector attack time constant in seconds for noise gate level tracking.
const GATE_ENV_ATTACK_SECS: f32 = 0.002;
/// Envelope detector release time constant in seconds for noise gate level tracking.
const GATE_ENV_RELEASE_SECS: f32 = 0.060;
/// Hold time in samples before the noise gate begins closing (~80 ms at 48 kHz).
const GATE_HOLD_SAMPLES: u32 = 3_840;
/// Smooth gate gain opening ramp time in seconds (~5 ms to avoid onset clicks).
const GATE_GAIN_ATTACK_SECS: f32 = 0.005;
/// Smooth gate gain closing ramp time in seconds (~120 ms for natural syllable tails).
const GATE_GAIN_RELEASE_SECS: f32 = 0.120;
/// Soft limiter knee start (-3.0 dBFS): audio below this stays 1:1 linear.
const LIMITER_KNEE: f32 = 0.708;
/// Soft limiter ceiling (-0.5 dBFS) so loud voice transients never hit 0.0 dBFS clipping.
const LIMITER_CEILING: f32 = 0.944;
/// Limiter attack time constant (~0.5 ms).
const LIMITER_ATTACK_SECS: f32 = 0.000_5;
/// Limiter release time constant (~60 ms).
const LIMITER_RELEASE_SECS: f32 = 0.060;

/// Applies transparent soft clipping to a normalized `[-1.0, 1.0]` sample.
///
/// Signals inside `[-0.80, 0.80]` pass at exact unity gain (`0 dB` distortion), while
/// summed multi-source transients above `0.80` saturate smoothly via `tanh` toward `±1.0`.
#[must_use]
pub fn soft_clip_sample(sample: f32) -> f32 {
    if !sample.is_finite() {
        return 0.0;
    }
    let abs = sample.abs();
    if abs <= SOFT_CLIP_KNEE {
        return sample;
    }
    let headroom = 1.0 - SOFT_CLIP_KNEE;
    let compressed = SOFT_CLIP_KNEE + headroom * ((abs - SOFT_CLIP_KNEE) / headroom).tanh();
    compressed.copysign(sample).clamp(-1.0, 1.0)
}

/// Sums active `48 kHz` interleaved stereo float blocks with per-source volume and mute controls,
/// applying `soft_clip_sample` to the mixed output.
#[must_use]
pub fn mix_stereo_blocks_soft_clip(
    sources: &[(&[f32; AAC_FRAME_SAMPLES * 2], f32, bool)],
) -> [f32; AAC_FRAME_SAMPLES * 2] {
    let mut mixed = [0.0f32; AAC_FRAME_SAMPLES * 2];
    for &(block, volume, muted) in sources {
        if muted {
            continue;
        }
        let gain = if volume.is_finite() {
            volume.max(0.0)
        } else {
            0.0
        };
        if gain == 0.0 {
            continue;
        }
        for (dst, &src) in mixed.iter_mut().zip(block.iter()) {
            *dst += src * gain;
        }
    }

    for sample in &mut mixed {
        *sample = soft_clip_sample(*sample);
    }
    mixed
}

fn time_constant_alpha(seconds: f32) -> f32 {
    let denom = seconds.max(0.000_1) * (TARGET_SAMPLE_RATE_HZ as f32);
    1.0 - (-1.0 / denom).exp()
}

/// Real-time microphone processor combining pure-Rust RNNoise suppression (`nnnoiseless`)
/// with a smooth attack/release dBFS noise gate.
///
/// Game audio and secondary outputs never pass through this processor.
pub struct MicNoiseProcessor {
    enabled: bool,
    strength: f32,
    noise_gate_db: Option<f32>,
    left_state: Box<DenoiseState<'static>>,
    right_state: Box<DenoiseState<'static>>,
    in_left: VecDeque<f32>,
    in_right: VecDeque<f32>,
    out_left: VecDeque<f32>,
    out_right: VecDeque<f32>,
    // Noise gate follower state
    env_linear: f32,
    hold_remaining: u32,
    gate_gain: f32,
    env_attack_alpha: f32,
    env_release_alpha: f32,
    gain_attack_alpha: f32,
    gain_release_alpha: f32,
    // Soft peak limiter state
    soft_limiter_enabled: bool,
    limiter_gain: f32,
    limiter_attack_alpha: f32,
    limiter_release_alpha: f32,
}

impl MicNoiseProcessor {
    /// Creates a new `48 kHz` stereo microphone denoiser + noise gate + soft peak limiter.
    #[must_use]
    pub fn new(enabled: bool, strength: f32, noise_gate_db: Option<f32>) -> Self {
        let mut out_left = VecDeque::with_capacity(AAC_FRAME_SAMPLES + RNNOISE_FRAME_SIZE * 2);
        let mut out_right = VecDeque::with_capacity(AAC_FRAME_SAMPLES + RNNOISE_FRAME_SIZE * 2);
        // Pre-seed one 10 ms lookahead block so every 1024-sample push produces 1024 denoised samples
        // without changing packet cadence against Game Audio.
        out_left.resize(RNNOISE_FRAME_SIZE, 0.0);
        out_right.resize(RNNOISE_FRAME_SIZE, 0.0);

        Self {
            enabled,
            strength: strength.clamp(0.0, 1.0),
            noise_gate_db: noise_gate_db.map(|db| db.clamp(-90.0, -10.0)),
            left_state: DenoiseState::new(),
            right_state: DenoiseState::new(),
            in_left: VecDeque::with_capacity(AAC_FRAME_SAMPLES + RNNOISE_FRAME_SIZE),
            in_right: VecDeque::with_capacity(AAC_FRAME_SAMPLES + RNNOISE_FRAME_SIZE),
            out_left,
            out_right,
            env_linear: 0.0,
            hold_remaining: 0,
            gate_gain: 0.0,
            env_attack_alpha: time_constant_alpha(GATE_ENV_ATTACK_SECS),
            env_release_alpha: time_constant_alpha(GATE_ENV_RELEASE_SECS),
            gain_attack_alpha: time_constant_alpha(GATE_GAIN_ATTACK_SECS),
            gain_release_alpha: time_constant_alpha(GATE_GAIN_RELEASE_SECS),
            soft_limiter_enabled: true,
            limiter_gain: 1.0,
            limiter_attack_alpha: time_constant_alpha(LIMITER_ATTACK_SECS),
            limiter_release_alpha: time_constant_alpha(LIMITER_RELEASE_SECS),
        }
    }

    /// Enables or disables the microphone `-0.5 dBFS` soft peak limiter on the fly.
    pub fn set_soft_limiter(&mut self, enabled: bool) {
        self.soft_limiter_enabled = enabled;
        if !enabled {
            self.limiter_gain = 1.0;
        }
    }

    #[must_use]
    pub fn soft_limiter_enabled(&self) -> bool {
        self.soft_limiter_enabled
    }

    /// Updates RNNoise enable flag, wet/dry strength, and noise gate threshold on the fly.
    pub fn update_config(&mut self, enabled: bool, strength: f32, noise_gate_db: Option<f32>) {
        if enabled && !self.enabled {
            self.reset_rnnoise_state();
        }
        self.enabled = enabled;
        self.strength = strength.clamp(0.0, 1.0);
        self.noise_gate_db = noise_gate_db.map(|db| db.clamp(-90.0, -10.0));
    }

    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    #[must_use]
    pub fn strength(&self) -> f32 {
        self.strength
    }

    #[must_use]
    pub fn noise_gate_db(&self) -> Option<f32> {
        self.noise_gate_db
    }

    /// Resets internal RNNoise GRU states and staging buffers (e.g., on device hot-swap).
    pub fn reset(&mut self) {
        self.reset_rnnoise_state();
        self.env_linear = 0.0;
        self.hold_remaining = 0;
        self.gate_gain = 0.0;
        self.limiter_gain = 1.0;
    }

    fn reset_rnnoise_state(&mut self) {
        self.left_state = DenoiseState::new();
        self.right_state = DenoiseState::new();
        self.in_left.clear();
        self.in_right.clear();
        self.out_left.clear();
        self.out_right.clear();
        self.out_left.resize(RNNOISE_FRAME_SIZE, 0.0);
        self.out_right.resize(RNNOISE_FRAME_SIZE, 0.0);
    }

    /// Processes a `48 kHz` interleaved stereo `1024`-sample block in-place.
    pub fn process_stereo_1024(&mut self, interleaved: &mut [f32; AAC_FRAME_SAMPLES * 2]) {
        if self.enabled {
            for frame in 0..AAC_FRAME_SAMPLES {
                self.in_left.push_back(interleaved[frame * 2]);
                self.in_right.push_back(interleaved[frame * 2 + 1]);
            }
            self.pump_rnnoise_frames();

            for frame in 0..AAC_FRAME_SAMPLES {
                interleaved[frame * 2] = self.out_left.pop_front().unwrap_or(0.0);
                interleaved[frame * 2 + 1] = self.out_right.pop_front().unwrap_or(0.0);
            }
        }

        if let Some(gate_db) = self.noise_gate_db {
            self.apply_noise_gate(interleaved, gate_db);
        } else {
            // Keep envelope neutral when gate is bypassed.
            self.gate_gain = 1.0;
        }

        if self.soft_limiter_enabled {
            self.apply_soft_peak_limiter(interleaved);
        }
    }

    fn pump_rnnoise_frames(&mut self) {
        let mut dry_l = [0.0f32; RNNOISE_FRAME_SIZE];
        let mut dry_r = [0.0f32; RNNOISE_FRAME_SIZE];
        let mut in_scaled_l = [0.0f32; RNNOISE_FRAME_SIZE];
        let mut in_scaled_r = [0.0f32; RNNOISE_FRAME_SIZE];
        let mut out_scaled_l = [0.0f32; RNNOISE_FRAME_SIZE];
        let mut out_scaled_r = [0.0f32; RNNOISE_FRAME_SIZE];

        while self.in_left.len() >= RNNOISE_FRAME_SIZE && self.in_right.len() >= RNNOISE_FRAME_SIZE
        {
            for i in 0..RNNOISE_FRAME_SIZE {
                let l = self.in_left.pop_front().unwrap_or(0.0);
                let r = self.in_right.pop_front().unwrap_or(0.0);
                let safe_l = if l.is_finite() { l } else { 0.0 };
                let safe_r = if r.is_finite() { r } else { 0.0 };
                dry_l[i] = safe_l;
                dry_r[i] = safe_r;
                in_scaled_l[i] = safe_l * RNNOISE_PCM_SCALE;
                in_scaled_r[i] = safe_r * RNNOISE_PCM_SCALE;
            }

            let _vad_l = self.left_state.process_frame(&mut out_scaled_l, &in_scaled_l);
            let _vad_r = self
                .right_state
                .process_frame(&mut out_scaled_r, &in_scaled_r);

            let wet = self.strength;
            let dry = 1.0 - wet;

            for i in 0..RNNOISE_FRAME_SIZE {
                let wet_l = (out_scaled_l[i] / RNNOISE_PCM_SCALE).clamp(-1.0, 1.0);
                let wet_r = (out_scaled_r[i] / RNNOISE_PCM_SCALE).clamp(-1.0, 1.0);
                let blended_l = dry * dry_l[i] + wet * if wet_l.is_finite() { wet_l } else { 0.0 };
                let blended_r = dry * dry_r[i] + wet * if wet_r.is_finite() { wet_r } else { 0.0 };
                self.out_left.push_back(blended_l.clamp(-1.0, 1.0));
                self.out_right.push_back(blended_r.clamp(-1.0, 1.0));
            }
        }
    }

    fn apply_noise_gate(&mut self, interleaved: &mut [f32; AAC_FRAME_SAMPLES * 2], gate_db: f32) {
        let threshold_linear = 10.0_f32.powf(gate_db / 20.0);

        for frame in 0..AAC_FRAME_SAMPLES {
            let l = interleaved[frame * 2];
            let r = interleaved[frame * 2 + 1];
            let level = l.abs().max(r.abs());

            let env_alpha = if level > self.env_linear {
                self.env_attack_alpha
            } else {
                self.env_release_alpha
            };
            self.env_linear += env_alpha * (level - self.env_linear);

            let target_gain = if self.env_linear >= threshold_linear {
                self.hold_remaining = GATE_HOLD_SAMPLES;
                1.0
            } else if self.hold_remaining > 0 {
                self.hold_remaining -= 1;
                1.0
            } else {
                0.0
            };

            let gain_alpha = if target_gain > self.gate_gain {
                self.gain_attack_alpha
            } else {
                self.gain_release_alpha
            };
            self.gate_gain += gain_alpha * (target_gain - self.gate_gain);

            interleaved[frame * 2] = l * self.gate_gain;
            interleaved[frame * 2 + 1] = r * self.gate_gain;
        }
    }

    fn apply_soft_peak_limiter(&mut self, interleaved: &mut [f32; AAC_FRAME_SAMPLES * 2]) {
        for frame in 0..AAC_FRAME_SAMPLES {
            let l = interleaved[frame * 2];
            let r = interleaved[frame * 2 + 1];
            let abs_peak = if l.is_finite() && r.is_finite() {
                l.abs().max(r.abs())
            } else {
                0.0
            };

            let target_gain = if abs_peak > LIMITER_CEILING {
                LIMITER_CEILING / abs_peak
            } else {
                1.0
            };

            let alpha = if target_gain < self.limiter_gain {
                self.limiter_attack_alpha
            } else {
                self.limiter_release_alpha
            };
            self.limiter_gain += alpha * (target_gain - self.limiter_gain);

            interleaved[frame * 2] = soft_limit_ceiling(l * self.limiter_gain);
            interleaved[frame * 2 + 1] = soft_limit_ceiling(r * self.limiter_gain);
        }
    }
}

fn soft_limit_ceiling(sample: f32) -> f32 {
    if !sample.is_finite() {
        return 0.0;
    }
    let abs = sample.abs();
    if abs <= LIMITER_KNEE {
        return sample;
    }
    let headroom = LIMITER_CEILING - LIMITER_KNEE;
    let compressed = LIMITER_KNEE + headroom * ((abs - LIMITER_KNEE) / headroom).tanh();
    compressed
        .copysign(sample)
        .clamp(-LIMITER_CEILING, LIMITER_CEILING)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum_sq: f32 = samples.iter().map(|&s| s * s).sum();
        (sum_sq / (samples.len() as f32)).sqrt()
    }

    #[test]
    fn rnnoise_suppresses_stationary_white_noise_without_nans_or_clipping() {
        let mut proc = MicNoiseProcessor::new(true, 1.0, None);
        // Deterministic pseudo-random stationary background hiss/fan noise (~-34 dBFS).
        let mut state: u32 = 0x1234_5678;
        let mut lpf = 0.0f32;
        let mut next_noise = || -> f32 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let normalized = ((state >> 16) as f32 / 32_768.0) - 1.0;
            lpf = lpf * 0.7 + normalized * 0.3;
            lpf * 0.05
        };

        let mut raw_tail_rms = 0.0f32;
        let mut denoised_tail_rms = 0.0f32;

        // Run ~0.77s (36 blocks) so RNNoise converges and compare tail blocks.
        for block_idx in 0..36 {
            let mut block = [0.0f32; AAC_FRAME_SAMPLES * 2];
            for frame in 0..AAC_FRAME_SAMPLES {
                let s = next_noise();
                block[frame * 2] = s;
                block[frame * 2 + 1] = s;
            }
            let raw_rms = rms(&block);
            proc.process_stereo_1024(&mut block);

            assert!(block.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
            if block_idx >= 24 {
                raw_tail_rms += raw_rms;
                denoised_tail_rms += rms(&block);
            }
        }

        assert!(
            denoised_tail_rms < raw_tail_rms * 0.55,
            "expected RNNoise to strongly attenuate stationary noise: raw={raw_tail_rms}, denoised={denoised_tail_rms}"
        );
    }

    #[test]
    fn noise_gate_attenuates_quiet_floor_and_opens_smoothly_on_speech() {
        let mut proc = MicNoiseProcessor::new(false, 0.0, Some(-40.0));

        // 1. Feed quiet floor (-60 dBFS ~= 0.001 amplitude): gate should stay closed (~0 gain).
        let mut quiet = [0.001f32; AAC_FRAME_SAMPLES * 2];
        for _ in 0..6 {
            quiet.fill(0.001);
            proc.process_stereo_1024(&mut quiet);
        }
        assert!(
            rms(&quiet) < 0.000_1,
            "noise gate should attenuate -60 dB floor below -40 dB threshold: got {}",
            rms(&quiet)
        );

        // 2. Feed active speech-level tone (-12 dBFS ~= 0.25 amplitude): gate must open smoothly to ~1.0.
        let mut loud = [0.0f32; AAC_FRAME_SAMPLES * 2];
        for b in 0..6 {
            for (i, sample) in loud.iter_mut().enumerate() {
                let t = ((b * AAC_FRAME_SAMPLES + (i / 2)) as f32) / 48_000.0;
                *sample = 0.25 * (2.0 * std::f32::consts::PI * 220.0 * t).sin();
            }
            proc.process_stereo_1024(&mut loud);
        }
        let expected_sine_rms = 0.25 * std::f32::consts::FRAC_1_SQRT_2;
        assert!(
            rms(&loud) > expected_sine_rms * 0.85,
            "noise gate should open smoothly for -12 dB speech: got {}",
            rms(&loud)
        );
    }

    #[test]
    fn soft_clipper_preserves_unity_below_knee_and_bounds_peaks() {
        assert!((soft_clip_sample(0.5) - 0.5).abs() < 1e-6);
        assert!((soft_clip_sample(-0.75) - (-0.75)).abs() < 1e-6);

        let a = [0.95f32; AAC_FRAME_SAMPLES * 2];
        let b = [0.85f32; AAC_FRAME_SAMPLES * 2];
        let muted = [1.0f32; AAC_FRAME_SAMPLES * 2];
        let mixed = mix_stereo_blocks_soft_clip(&[(&a, 1.0, false), (&b, 1.0, false), (&muted, 1.0, true)]);
        assert!(mixed.iter().all(|&s| s > 0.9 && s <= 1.0));

        let all_muted = mix_stereo_blocks_soft_clip(&[(&a, 1.0, true)]);
        assert!(all_muted.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn mic_soft_limiter_caps_loud_peaks_below_zero_dbfs() {
        let mut proc = MicNoiseProcessor::new(false, 0.0, None);
        assert!(proc.soft_limiter_enabled());

        let mut hot_voice = [0.0f32; AAC_FRAME_SAMPLES * 2];
        for (i, dst) in hot_voice.iter_mut().enumerate() {
            let phase = (i / 2) as f32 * 0.05;
            *dst = 1.45 * phase.sin();
        }
        proc.process_stereo_1024(&mut hot_voice);
        let peak = hot_voice.iter().copied().map(f32::abs).fold(0.0f32, f32::max);
        assert!(
            peak <= LIMITER_CEILING + 1e-5 && peak > 0.70,
            "expected soft limiter peak inside (0.70, {LIMITER_CEILING}], got {peak}"
        );
    }
}
