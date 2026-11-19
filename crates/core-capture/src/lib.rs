//! Zero-copy screen and window capture via `Windows.Graphics.Capture` (WGC) and
//! GPU color-space conversion / downscaling via `ID3D11VideoProcessor` (`BGRA/HDR -> NV12`).

pub mod d3d_context;
pub mod engine;
pub mod targets;
pub mod video_processor;

pub use core_types::{
    CaptureError, CaptureSettings, CaptureTarget, GpuFrameDescriptor, MonitorInfo, PixelFormat,
    WindowInfo,
};
pub use d3d_context::D3d11SharedContext;
pub use engine::{
    start_capture_thread, CaptureControlCommand, CaptureEngineConfig, CaptureOutputEvent,
    CaptureSessionHandle,
};
pub use targets::{
    create_capture_item, enumerate_monitors, enumerate_windows, query_process_exe_name,
    query_process_exe_path,
};
pub use video_processor::{GpuTextureFrame, GpuVideoProcessor};
