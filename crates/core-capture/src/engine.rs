//! WGC capture loop supporting dynamic `CaptureTarget` switching, window resize/minimize
//! handling, `DXGI_ERROR_DEVICE_REMOVED` recovery, and bounded channel backpressure.

use crate::d3d_context::D3d11SharedContext;
use crate::targets::{create_capture_item, ResolvedCaptureItem};
use crate::video_processor::{GpuTextureFrame, GpuVideoProcessor};
use core_types::{
    CaptureError, CaptureTarget, HdrMode, MonotonicClock, ResolutionPreset,
    VIDEO_FRAME_CHANNEL_CAPACITY,
};
use crossbeam_channel::{bounded, Receiver, Sender, TryRecvError};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{HWND, RPC_E_CHANGED_MODE};
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::System::WinRT::Direct3D11::IDirect3DDxgiInterfaceAccess;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindow};

/// Interval at which the most recent GPU NV12 texture is re-emitted without copies when WGC
/// produces no new frame (e.g. a static desktop or paused game), preserving fixed GOP cadence.
const STATIC_FRAME_REPEAT_INTERVAL: Duration = Duration::from_millis(100);

/// Configuration for the capture worker thread.
#[derive(Debug, Clone)]
pub struct CaptureEngineConfig {
    pub initial_target: CaptureTarget,
    pub resolution: ResolutionPreset,
    pub fps: u32,
    pub capture_cursor: bool,
    pub capture_border: bool,
    pub hdr_mode: HdrMode,
    /// When `true` (e.g. CLI test bench), `AutoGame` falls back to primary monitor if no foreground app is selected.
    pub autogame_fallback_to_monitor: bool,
}

/// Control commands sent to the running WGC capture thread for zero-restart switching.
#[derive(Debug, Clone)]
pub enum CaptureControlCommand {
    SwitchTarget(CaptureTarget),
    SetResolution(ResolutionPreset),
    Stop,
}

/// Messages emitted by the capture thread over a bounded channel.
pub enum CaptureOutputEvent {
    /// Emitted at startup and whenever `CaptureTarget` or output dimensions change so
    /// downstream encoder and ring buffer reset cleanly without mixing sources.
    SourceChanged {
        epoch: u64,
        target: CaptureTarget,
        display_label: String,
        width: u32,
        height: u32,
    },
    /// Emitted if the D3D11 device was lost (`DXGI_ERROR_DEVICE_REMOVED`) and recreated.
    DeviceRecreated(D3d11SharedContext),
    /// Zero-copy NV12 VRAM frame ready for hardware encoding.
    Nv12Frame(GpuTextureFrame),
    /// Non-fatal diagnostic notice (e.g., window minimized or transient device recovery).
    RecoverableWarning(String),
}

/// Handle controlling the active capture worker thread.
pub struct CaptureSessionHandle {
    cmd_tx: Sender<CaptureControlCommand>,
    stop_flag: Arc<AtomicBool>,
    join_handle: Option<JoinHandle<()>>,
}

impl CaptureSessionHandle {
    /// Requests a live switch to a new `CaptureTarget` without restarting the process.
    pub fn switch_target(&self, target: CaptureTarget) -> Result<(), CaptureError> {
        self.cmd_tx
            .send(CaptureControlCommand::SwitchTarget(target))
            .map_err(|_| CaptureError::TargetClosed)
    }

    /// Requests a live output resolution change (applied on GPU by `ID3D11VideoProcessor`).
    pub fn set_resolution(&self, preset: ResolutionPreset) -> Result<(), CaptureError> {
        self.cmd_tx
            .send(CaptureControlCommand::SetResolution(preset))
            .map_err(|_| CaptureError::TargetClosed)
    }

    /// Signals the capture thread to stop and waits for it to join cleanly.
    pub fn stop(mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        let _ = self.cmd_tx.send(CaptureControlCommand::Stop);
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for CaptureSessionHandle {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        let _ = self.cmd_tx.send(CaptureControlCommand::Stop);
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }
}

/// Spawns the WGC + D3D11 VideoProcessor capture thread with bounded output channel.
pub fn start_capture_thread(
    shared_d3d: D3d11SharedContext,
    config: CaptureEngineConfig,
    clock: MonotonicClock,
) -> Result<(CaptureSessionHandle, Receiver<CaptureOutputEvent>), CaptureError> {
    let (frame_tx, frame_rx) = bounded(VIDEO_FRAME_CHANNEL_CAPACITY);
    let (cmd_tx, cmd_rx) = bounded(16);
    let stop_flag = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop_flag);

    let join_handle = thread::Builder::new()
        .name("rustclip-wgc-capture".to_string())
        .spawn(move || {
            run_capture_loop(shared_d3d, config, clock, cmd_rx, frame_tx, thread_stop);
        })
        .map_err(|err| {
            CaptureError::InitFailed(format!("Failed to spawn capture thread: {err}"))
        })?;

    Ok((
        CaptureSessionHandle {
            cmd_tx,
            stop_flag,
            join_handle: Some(join_handle),
        },
        frame_rx,
    ))
}

struct ActiveWgcSession {
    item: GraphicsCaptureItem,
    frame_pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    pool_size: SizeInt32,
    window_hwnd: Option<HWND>,
    display_label: String,
}

impl ActiveWgcSession {
    fn close(self) {
        let _ = self.session.Close();
        let _ = self.frame_pool.Close();
        drop(self.item);
    }
}

fn run_capture_loop(
    shared: D3d11SharedContext,
    mut config: CaptureEngineConfig,
    clock: MonotonicClock,
    cmd_rx: Receiver<CaptureControlCommand>,
    out_tx: Sender<CaptureOutputEvent>,
    stop_flag: Arc<AtomicBool>,
) {
    // Initialize WinRT Multi-Threaded Apartment on the capture thread.
    if let Err(err) = unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
        if err.code() != RPC_E_CHANGED_MODE {
            warn!("RoInitialize failed on capture thread: {err}");
        }
    }

    let mut winrt_device = match shared.create_winrt_device() {
        Ok(dev) => dev,
        Err(err) => {
            let _ = out_tx.try_send(CaptureOutputEvent::RecoverableWarning(format!(
                "Initial IDirect3DDevice creation failed: {err}"
            )));
            return;
        }
    };

    let mut processor = match GpuVideoProcessor::new(&shared) {
        Ok(proc) => proc,
        Err(err) => {
            let _ = out_tx.try_send(CaptureOutputEvent::RecoverableWarning(format!(
                "Initial GpuVideoProcessor creation failed: {err}"
            )));
            return;
        }
    };

    let mut current_target = config.initial_target.clone();
    let mut source_epoch: u64 = 1;
    let mut frame_number: u64 = 0;
    let mut active: Option<ActiveWgcSession> = None;
    let mut current_out_size: (u32, u32) = (0, 0);
    let mut last_nv12_frame: Option<GpuTextureFrame> = None;
    let mut last_emitted_at = Instant::now();
    let target_frame_interval =
        Duration::from_nanos(1_000_000_000 / u64::from(config.fps.clamp(15, 240)));
    let mut next_frame_deadline = Instant::now();
    let mut last_minimized_notice = Instant::now() - Duration::from_secs(10);

    while !stop_flag.load(Ordering::Relaxed) {
        // 1. Drain any pending control commands for hot target / resolution switching.
        loop {
            match cmd_rx.try_recv() {
                Ok(CaptureControlCommand::Stop) => {
                    if let Some(sess) = active.take() {
                        sess.close();
                    }
                    return;
                }
                Ok(CaptureControlCommand::SwitchTarget(new_target)) => {
                    info!("Hot-switching CaptureTarget to {:?}", new_target);
                    last_nv12_frame = None;
                    if let Some(sess) = active.take() {
                        sess.close();
                    }
                    current_target = new_target;
                    source_epoch = source_epoch.saturating_add(1);
                    current_out_size = (0, 0);
                }
                Ok(CaptureControlCommand::SetResolution(preset)) => {
                    info!("Updating output resolution preset to {:?}", preset);
                    last_nv12_frame = None;
                    config.resolution = preset;
                    source_epoch = source_epoch.saturating_add(1);
                    current_out_size = (0, 0);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if let Some(sess) = active.take() {
                        sess.close();
                    }
                    return;
                }
            }
        }

        // 2. Ensure WGC session exists for `current_target`.
        if active.is_none() {
            match open_wgc_session(&winrt_device, &current_target, &config) {
                Ok(sess) => {
                    let (out_w, out_h) = config.resolution.resolve(
                        sess.pool_size.Width.max(2) as u32,
                        sess.pool_size.Height.max(2) as u32,
                    );
                    current_out_size = (out_w, out_h);
                    let _ = out_tx.send(CaptureOutputEvent::SourceChanged {
                        epoch: source_epoch,
                        target: current_target.clone(),
                        display_label: sess.display_label.clone(),
                        width: out_w,
                        height: out_h,
                    });
                    active = Some(sess);
                    next_frame_deadline = Instant::now();
                }
                Err(err) => {
                    if err.is_recoverable() {
                        debug!("Waiting for capture target availability: {err}");
                    } else {
                        let _ = out_tx.try_send(CaptureOutputEvent::RecoverableWarning(format!(
                            "Capture init warning: {err}"
                        )));
                    }
                    thread::sleep(Duration::from_millis(200));
                    continue;
                }
            }
        }

        // 3. Verify SpecificWindow health (handle minimize without crashing; handle close cleanly).
        if let Some(sess) = active.as_ref() {
            if let Some(hwnd) = sess.window_hwnd {
                if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
                    let _ = out_tx.try_send(CaptureOutputEvent::RecoverableWarning(
                        "Captured window closed; waiting for target or switch".to_string(),
                    ));
                    last_nv12_frame = None;
                    if let Some(old) = active.take() {
                        old.close();
                    }
                    thread::sleep(Duration::from_millis(150));
                    continue;
                }
                if unsafe { IsIconic(hwnd) }.as_bool() {
                    last_nv12_frame = None;
                    if last_minimized_notice.elapsed() > Duration::from_secs(5) {
                        let _ = out_tx.try_send(CaptureOutputEvent::RecoverableWarning(format!(
                            "Window '{}' is minimized; pausing frame acquisition until restored",
                            sess.display_label
                        )));
                        last_minimized_notice = Instant::now();
                    }
                    thread::sleep(Duration::from_millis(100));
                    continue;
                }
            }
        }

        // 4. Pace frame acquisition to target FPS without spinning CPU.
        let now = Instant::now();
        if now < next_frame_deadline {
            thread::sleep((next_frame_deadline - now).min(Duration::from_millis(4)));
            continue;
        }
        next_frame_deadline = (next_frame_deadline + target_frame_interval).max(now);

        let Some(sess) = active.as_mut() else {
            continue;
        };

        // Pull the latest available frame from the free-threaded WGC frame pool so we keep
        // latency minimal and drain any queued frames if the game renders faster than target FPS.
        let mut latest_frame = None;
        while let Ok(frame) = sess.frame_pool.TryGetNextFrame() {
            latest_frame = Some(frame);
        }

        let Some(frame) = latest_frame else {
            if let Some(prev) = last_nv12_frame.as_ref() {
                if prev.source_epoch == source_epoch
                    && last_emitted_at.elapsed() >= STATIC_FRAME_REPEAT_INTERVAL
                {
                    let mut repeat = prev.clone();
                    frame_number = frame_number.saturating_add(1);
                    repeat.descriptor.pts_us = clock.now_us();
                    repeat.descriptor.frame_number = frame_number;
                    let _ = out_tx.try_send(CaptureOutputEvent::Nv12Frame(repeat));
                    last_emitted_at = Instant::now();
                    continue;
                }
            }
            thread::sleep(Duration::from_millis(1));
            continue;
        };

        let content_size = match frame.ContentSize() {
            Ok(sz) => sz,
            Err(_) => continue,
        };
        if content_size.Width < 2 || content_size.Height < 2 {
            // Minimized or zero-area surface: ignore cleanly.
            continue;
        }

        // Handle live window/display resize by recreating the WGC frame pool on the fly.
        if content_size.Width != sess.pool_size.Width || content_size.Height != sess.pool_size.Height
        {
            last_nv12_frame = None;
            let px_format = choose_wgc_pixel_format(config.hdr_mode);
            if let Err(err) = sess
                .frame_pool
                .Recreate(&winrt_device, px_format, 2, content_size)
            {
                warn!("FramePool::Recreate failed: {err}");
                if let Some(old) = active.take() {
                    old.close();
                }
                continue;
            }
            sess.pool_size = content_size;

            let (new_out_w, new_out_h) = config
                .resolution
                .resolve(content_size.Width as u32, content_size.Height as u32);
            if (new_out_w, new_out_h) != current_out_size {
                current_out_size = (new_out_w, new_out_h);
                source_epoch = source_epoch.saturating_add(1);
                let _ = out_tx.send(CaptureOutputEvent::SourceChanged {
                    epoch: source_epoch,
                    target: current_target.clone(),
                    display_label: sess.display_label.clone(),
                    width: new_out_w,
                    height: new_out_h,
                });
            }
            // Wait for the next frame produced from the recreated pool.
            continue;
        }

        let surface = match frame.Surface() {
            Ok(s) => s,
            Err(_) => continue,
        };
        let dxgi_access: IDirect3DDxgiInterfaceAccess =
            match windows::core::Interface::cast(&surface) {
                Ok(a) => a,
                Err(_) => continue,
            };
        let input_tex: ID3D11Texture2D = match unsafe { dxgi_access.GetInterface() } {
            Ok(t) => t,
            Err(_) => continue,
        };

        let pts_us = clock.now_us();
        frame_number = frame_number.saturating_add(1);
        let (out_w, out_h) = if current_out_size.0 >= 2 && current_out_size.1 >= 2 {
            current_out_size
        } else {
            config
                .resolution
                .resolve(content_size.Width as u32, content_size.Height as u32)
        };

        match processor.convert_to_nv12(
            &input_tex,
            out_w,
            out_h,
            config.fps,
            pts_us,
            frame_number,
            source_epoch,
        ) {
            Ok(gpu_frame) => {
                last_nv12_frame = Some(gpu_frame.clone());
                last_emitted_at = Instant::now();
                // Bounded channel backpressure: if encoder queue is full, drop newest captured
                // frame rather than stalling WGC or growing memory.
                if out_tx.try_send(CaptureOutputEvent::Nv12Frame(gpu_frame)).is_err() {
                    debug!("Video frame channel full; dropping frame {frame_number}");
                }
            }
            Err(err @ CaptureError::DeviceRemoved { .. }) => {
                warn!("GPU device lost ({err}); attempting full D3D11 + WGC recovery");
                last_nv12_frame = None;
                if let Some(old) = active.take() {
                    old.close();
                }
                match recover_d3d_pipeline(&out_tx) {
                    Ok((new_winrt, new_proc)) => {
                        winrt_device = new_winrt;
                        processor = new_proc;
                        source_epoch = source_epoch.saturating_add(1);
                    }
                    Err(rec_err) => {
                        let _ = out_tx.try_send(CaptureOutputEvent::RecoverableWarning(format!(
                            "GPU recovery delayed: {rec_err}"
                        )));
                        thread::sleep(Duration::from_millis(500));
                    }
                }
            }
            Err(err) => {
                let _ = out_tx.try_send(CaptureOutputEvent::RecoverableWarning(format!(
                    "VideoProcessor error: {err}"
                )));
            }
        }
    }

    if let Some(sess) = active.take() {
        sess.close();
    }
}

fn choose_wgc_pixel_format(hdr_mode: HdrMode) -> DirectXPixelFormat {
    match hdr_mode {
        HdrMode::PreserveHdr10 => DirectXPixelFormat::R16G16B16A16Float,
        HdrMode::AutoToneMapToSdr | HdrMode::ForceSdrClamp => {
            DirectXPixelFormat::B8G8R8A8UIntNormalized
        }
    }
}

fn open_wgc_session(
    winrt_device: &IDirect3DDevice,
    target: &CaptureTarget,
    config: &CaptureEngineConfig,
) -> Result<ActiveWgcSession, CaptureError> {
    if !GraphicsCaptureSession::IsSupported().unwrap_or(false) {
        return Err(CaptureError::UnsupportedPlatform);
    }

    let ResolvedCaptureItem {
        item,
        display_label,
        window_hwnd,
    } = create_capture_item(target, config.autogame_fallback_to_monitor)?;

    let size = item
        .Size()
        .map_err(|err| CaptureError::InitFailed(format!("GraphicsCaptureItem::Size failed: {err}")))?;
    let safe_size = SizeInt32 {
        Width: size.Width.max(2),
        Height: size.Height.max(2),
    };

    let px_format = choose_wgc_pixel_format(config.hdr_mode);
    let frame_pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        winrt_device,
        px_format,
        2,
        safe_size,
    )
    .map_err(|err| {
        CaptureError::InitFailed(format!(
            "Direct3D11CaptureFramePool::CreateFreeThreaded failed: {err}"
        ))
    })?;

    let session = frame_pool.CreateCaptureSession(&item).map_err(|err| {
        CaptureError::InitFailed(format!("CreateCaptureSession failed: {err}"))
    })?;

    let _ = session.SetIsCursorCaptureEnabled(config.capture_cursor);
    // Border toggle requires Windows 11 / manifest capability; ignore error on unsupported systems.
    let _ = session.SetIsBorderRequired(config.capture_border);

    session
        .StartCapture()
        .map_err(|err| CaptureError::InitFailed(format!("StartCapture failed: {err}")))?;

    Ok(ActiveWgcSession {
        item,
        frame_pool,
        session,
        pool_size: safe_size,
        window_hwnd,
        display_label,
    })
}

fn recover_d3d_pipeline(
    out_tx: &Sender<CaptureOutputEvent>,
) -> Result<(IDirect3DDevice, GpuVideoProcessor), CaptureError> {
    let new_shared = D3d11SharedContext::new()?;
    let new_winrt = new_shared.create_winrt_device()?;
    let new_proc = GpuVideoProcessor::new(&new_shared)?;
    let _ = out_tx.send(CaptureOutputEvent::DeviceRecreated(new_shared));
    Ok((new_winrt, new_proc))
}
