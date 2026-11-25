//! Multi-stream WASAPI audio capture (Game Loopback + Microphone + Optional Secondary Output),
//! pure-Rust RNNoise mic denoising (`nnnoiseless`), synchronized Mixdown + clean stem tracks,
//! `48 kHz` Stereo resampling (`rubato`), QPC clock alignment, and Media Foundation AAC encoding.

pub mod aac_encoder;
pub mod capture;
pub mod denoise;
pub mod devices;
pub mod mf_verify;
pub mod process_loopback;
pub mod resampler;

pub use aac_encoder::{elapsed_48k_frames, sample_index_to_pts_us, MfAacEncoder};
pub use capture::{
    is_ptt_key_down, start_audio_pipeline, target_aac_bitrate_kbps, AudioControlCommand,
    AudioOutputEvent, AudioPipelineHandle, AudioPipelineTelemetry, AudioTrackTelemetry,
};
pub use core_types::{
    ActiveAudioAppInfo, AppAudioSlotConfig, AudioDeviceInfo, AudioError, AudioOutputMode,
    AudioSettings, AudioStreamHeader, MediaPacket, MicActivationMode, MicDeviceConfig, TrackKind,
};
pub use denoise::{
    mix_stereo_blocks_soft_clip, soft_clip_sample, MicNoiseProcessor, RNNOISE_FRAME_SIZE,
};
pub use devices::{
    enumerate_capture_devices, enumerate_render_devices, enumerate_render_devices_with_activity,
};
pub use mf_verify::{verify_mp4_audio_with_mf_source_reader, MfAudioVerifyReport};
pub use process_loopback::{
    enumerate_audio_sessions, find_best_pid_for_processes, is_pid_running, list_active_audio_apps,
    AudioSessionInfo, ProcessLoopbackStream,
};
pub use resampler::{
    float_frame_to_i16, linear_to_dbfs, Stereo48kResampler, AAC_FRAME_SAMPLES, TARGET_CHANNELS,
    TARGET_SAMPLE_RATE_HZ,
};
