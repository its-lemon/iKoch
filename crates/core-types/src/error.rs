//! Unified error hierarchy with explicit recoverable vs fatal classification.

use thiserror::Error;

/// Result alias using [`CoreError`].
pub type CoreResult<T> = Result<T, CoreError>;

/// Top-level error contract shared across all RustClip crates.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("capture error: {0}")]
    Capture(#[from] CaptureError),

    #[error("encode error: {0}")]
    Encode(#[from] EncodeError),

    #[error("audio error: {0}")]
    Audio(#[from] AudioError),

    #[error("mux error: {0}")]
    Mux(#[from] MuxError),

    #[error("storage error: {0}")]
    Storage(#[from] StorageError),

    #[error("sidecar ffmpeg error: {0}")]
    Sidecar(#[from] SidecarError),

    #[error("configuration error: {0}")]
    Config(String),
}

impl CoreError {
    /// Returns `true` when the pipeline should perform graceful recovery (re-initialization)
    /// instead of terminating the capture/audio thread.
    #[must_use]
    pub fn is_recoverable(&self) -> bool {
        match self {
            Self::Capture(err) => err.is_recoverable(),
            Self::Encode(err) => err.is_recoverable(),
            Self::Audio(err) => err.is_recoverable(),
            Self::Mux(err) => err.is_recoverable(),
            Self::Storage(_) | Self::Sidecar(_) | Self::Config(_) => false,
        }
    }
}

/// Errors emitted by `core-capture` (WGC, D3D11 device, `ID3D11VideoProcessor`).
#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("DXGI device removed or reset (HRESULT {hresult:#010x})")]
    DeviceRemoved { hresult: u32 },

    #[error("capture target resolution or DPI changed ({width}x{height})")]
    SurfaceResized { width: u32, height: u32 },

    #[error("capture target window closed or became unavailable")]
    TargetClosed,

    #[error("Windows.Graphics.Capture is not supported on this system")]
    UnsupportedPlatform,

    #[error("D3D11/WGC initialization failed: {0}")]
    InitFailed(String),

    #[error("video processor conversion failed: {0}")]
    VideoProcessorFailed(String),
}

impl CaptureError {
    #[must_use]
    pub const fn is_recoverable(&self) -> bool {
        matches!(
            self,
            Self::DeviceRemoved { .. } | Self::SurfaceResized { .. } | Self::TargetClosed
        )
    }
}

/// Errors emitted by `core-encode` (Media Foundation hardware MFT).
#[derive(Debug, Error)]
pub enum EncodeError {
    #[error("no compatible hardware video encoder MFT found for {codec}")]
    HardwareEncoderUnavailable { codec: String },

    #[error("Media Foundation device manager reset required (HRESULT {hresult:#010x})")]
    DeviceResetRequired { hresult: u32 },

    #[error("encoder stream format rejected: {0}")]
    InvalidMediaType(String),

    #[error("encoder runtime failure: {0}")]
    MftFailure(String),
}

impl EncodeError {
    #[must_use]
    pub const fn is_recoverable(&self) -> bool {
        matches!(self, Self::DeviceResetRequired { .. })
    }
}

/// Errors emitted by `core-audio` (WASAPI capture, silence injection, resampler, AAC MFT).
#[derive(Debug, Error)]
pub enum AudioError {
    #[error("audio endpoint disconnected or invalidated: {device}")]
    DeviceDisconnected { device: String },

    #[error("audio endpoint format changed and requires re-initialization")]
    FormatChanged,

    #[error("WASAPI initialization failed: {0}")]
    WasapiInitFailed(String),

    #[error("resampler error: {0}")]
    ResampleFailed(String),

    #[error("AAC Media Foundation encoder failed: {0}")]
    AacEncodeFailed(String),
}

impl AudioError {
    #[must_use]
    pub const fn is_recoverable(&self) -> bool {
        matches!(
            self,
            Self::DeviceDisconnected { .. } | Self::FormatChanged
        )
    }
}

/// Errors emitted by `core-mux` (PacketRouter, GOP Ring Buffer, fMP4 writer).
#[derive(Debug, Error)]
pub enum MuxError {
    #[error("ring buffer contains no complete GOP starting with an IDR keyframe")]
    NoKeyframeInBuffer,

    #[error("stream headers (SPS/PPS/ASC) are not yet initialized")]
    MissingStreamHeaders,

    #[error("bounded mux channel congested; frame dropped cleanly")]
    BackpressureDrop,

    #[error("MP4/fMP4 container I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("invalid container state: {0}")]
    InvalidState(String),
}

impl MuxError {
    #[must_use]
    pub const fn is_recoverable(&self) -> bool {
        matches!(self, Self::BackpressureDrop)
    }
}

/// Errors related to disk storage, JSON sidecars, and quota checks.
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("storage I/O failure: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON serialization failure: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("recording item not found: {0}")]
    NotFound(String),
}

/// Errors emitted when running the offline `ffmpeg.exe` sidecar process.
#[derive(Debug, Error)]
pub enum SidecarError {
    #[error("ffmpeg sidecar binary not found at {0}")]
    BinaryMissing(String),

    #[error("ffmpeg exited with status {code:?}: {stderr}")]
    ProcessFailed { code: Option<i32>, stderr: String },

    #[error("invalid edit parameters: {0}")]
    InvalidRequest(String),

    #[error("sidecar spawn failed: {0}")]
    SpawnFailed(#[from] std::io::Error),
}
