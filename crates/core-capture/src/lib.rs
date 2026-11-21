//! Zero-copy screen and window capture via `Windows.Graphics.Capture` (WGC) and
//! GPU color-space conversion / downscaling via `ID3D11VideoProcessor` (`BGRA/HDR -> NV12`).

#[cfg(windows)]
pub mod d3d_context;
#[cfg(windows)]
pub mod engine;
#[cfg(windows)]
pub mod targets;
#[cfg(windows)]
pub mod video_processor;

#[cfg(target_os = "linux")]
pub mod linux;

pub use core_types::{
    CaptureError, CaptureSettings, CaptureTarget, GpuFrameDescriptor, MonitorInfo, PixelFormat,
    WindowInfo,
};

#[cfg(windows)]
pub use d3d_context::D3d11SharedContext;
#[cfg(windows)]
pub use engine::{
    start_capture_thread, CaptureControlCommand, CaptureEngineConfig, CaptureOutputEvent,
    CaptureSessionHandle,
};
#[cfg(windows)]
pub use targets::{
    create_capture_item, enumerate_monitors, enumerate_windows, query_process_exe_name,
    query_process_exe_path,
};
#[cfg(windows)]
pub use video_processor::{GpuTextureFrame, GpuVideoProcessor};

#[cfg(target_os = "linux")]
pub use linux::{
    enumerate_monitors, enumerate_windows, query_process_exe_name, query_process_exe_path,
    start_capture_thread, CaptureControlCommand, CaptureEngineConfig, CaptureOutputEvent,
    CaptureSessionHandle, D3d11SharedContext, GpuTextureFrame, GpuVideoProcessor,
};
