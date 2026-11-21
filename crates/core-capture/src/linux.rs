//! Linux screen/window capture via `ashpd` (`xdg-desktop-portal` ScreenCast + PipeWire)
//! and GStreamer `pipewiresrc` outputting NV12 frames, with `x11rb` / `/proc` enumeration.

use ashpd::desktop::screencast::{CursorMode, Screencast, SourceType};
use ashpd::desktop::PersistMode;
use core_types::{
    CaptureError, CaptureTarget, GpuFrameDescriptor, HdrMode, MonitorInfo, MonotonicClock,
    PixelFormat, ResolutionPreset, WindowInfo, VIDEO_FRAME_CHANNEL_CAPACITY,
};
use crossbeam_channel::{bounded, Receiver, Sender, TryRecvError};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use std::fs;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tracing::{info, warn};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};

/// Shared Linux capture context (analogous to `D3d11SharedContext` on Windows).
#[derive(Clone, Default)]
pub struct D3d11SharedContext {
    pub device: (),
}

impl D3d11SharedContext {
    pub fn new() -> Result<Self, CaptureError> {
        gst::init().map_err(|e| CaptureError::InitFailed(format!("GStreamer init failed: {e}")))?;
        Ok(Self { device: () })
    }
}

/// CPU/DMA-BUF NV12 frame produced by `pipewiresrc` on Linux.
#[derive(Clone)]
pub struct GpuTextureFrame {
    pub nv12_bytes: Vec<u8>,
    pub stride: u32,
    pub descriptor: GpuFrameDescriptor,
    pub source_epoch: u64,
}

/// Placeholder for API parity with Windows `GpuVideoProcessor`.
pub struct GpuVideoProcessor;

/// Configuration for the Linux PipeWire/GStreamer capture worker thread.
#[derive(Debug, Clone)]
pub struct CaptureEngineConfig {
    pub initial_target: CaptureTarget,
    pub resolution: ResolutionPreset,
    pub fps: u32,
    pub capture_cursor: bool,
    pub capture_border: bool,
    pub hdr_mode: HdrMode,
    pub autogame_fallback_to_monitor: bool,
}

/// Control commands sent to the running capture thread.
#[derive(Debug, Clone)]
pub enum CaptureControlCommand {
    SwitchTarget(CaptureTarget),
    SetResolution(ResolutionPreset),
    Stop,
}

/// Messages emitted by the Linux capture thread.
pub enum CaptureOutputEvent {
    SourceChanged {
        epoch: u64,
        target: CaptureTarget,
        display_label: String,
        width: u32,
        height: u32,
    },
    DeviceRecreated(D3d11SharedContext),
    Nv12Frame(GpuTextureFrame),
    RecoverableWarning(String),
}

/// Handle controlling the active Linux capture worker thread.
pub struct CaptureSessionHandle {
    cmd_tx: Sender<CaptureControlCommand>,
    stop_flag: Arc<AtomicBool>,
    join_handle: Option<JoinHandle<()>>,
}

impl CaptureSessionHandle {
    pub fn switch_target(&self, target: CaptureTarget) -> Result<(), CaptureError> {
        self.cmd_tx
            .send(CaptureControlCommand::SwitchTarget(target))
            .map_err(|_| CaptureError::TargetClosed)
    }

    pub fn set_resolution(&self, preset: ResolutionPreset) -> Result<(), CaptureError> {
        self.cmd_tx
            .send(CaptureControlCommand::SetResolution(preset))
            .map_err(|_| CaptureError::TargetClosed)
    }

    pub fn stop(mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        let _ = self.cmd_tx.send(CaptureControlCommand::Stop);
        if let Some(h) = self.join_handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for CaptureSessionHandle {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        let _ = self.cmd_tx.send(CaptureControlCommand::Stop);
        if let Some(h) = self.join_handle.take() {
            let _ = h.join();
        }
    }
}

/// Enumerates monitors via X11/XWayland, falling back to a default Wayland portal monitor.
pub fn enumerate_monitors() -> Result<Vec<MonitorInfo>, CaptureError> {
    if let Ok((conn, screen_num)) = x11rb::connect(None) {
        let setup = conn.setup();
        if let Some(screen) = setup.roots.get(screen_num) {
            return Ok(vec![MonitorInfo {
                monitor_id: "DISPLAY-0".to_string(),
                name: format!(
                    "Primary Display ({}x{})",
                    screen.width_in_pixels, screen.height_in_pixels
                ),
                hmonitor: 0,
                width: u32::from(screen.width_in_pixels),
                height: u32::from(screen.height_in_pixels),
                x: 0,
                y: 0,
                is_primary: true,
            }]);
        }
    }

    Ok(vec![MonitorInfo {
        monitor_id: "wayland-0".to_string(),
        name: "Primary Display (Wayland Portal)".to_string(),
        hmonitor: 0,
        width: 1920,
        height: 1080,
        x: 0,
        y: 0,
        is_primary: true,
    }])
}

/// Enumerates visible top-level X11/XWayland windows with PID and process name.
pub fn enumerate_windows() -> Result<Vec<WindowInfo>, CaptureError> {
    let Ok((conn, screen_num)) = x11rb::connect(None) else {
        return Ok(Vec::new());
    };
    let Some(screen) = conn.setup().roots.get(screen_num) else {
        return Ok(Vec::new());
    };

    let Ok(atom_clients) = conn.intern_atom(false, b"_NET_CLIENT_LIST") else {
        return Ok(Vec::new());
    };
    let Ok(atom_name) = conn.intern_atom(false, b"_NET_WM_NAME") else {
        return Ok(Vec::new());
    };
    let Ok(atom_pid) = conn.intern_atom(false, b"_NET_WM_PID") else {
        return Ok(Vec::new());
    };
    let Ok(atom_utf8) = conn.intern_atom(false, b"UTF8_STRING") else {
        return Ok(Vec::new());
    };

    let Ok(atom_clients) = atom_clients.reply() else {
        return Ok(Vec::new());
    };
    let Ok(atom_name) = atom_name.reply() else {
        return Ok(Vec::new());
    };
    let Ok(atom_pid) = atom_pid.reply() else {
        return Ok(Vec::new());
    };
    let Ok(atom_utf8) = atom_utf8.reply() else {
        return Ok(Vec::new());
    };

    let Ok(prop_cookie) = conn.get_property(
        false,
        screen.root,
        atom_clients.atom,
        AtomEnum::WINDOW,
        0,
        1024,
    ) else {
        return Ok(Vec::new());
    };
    let Ok(prop) = prop_cookie.reply() else {
        return Ok(Vec::new());
    };

    let mut out = Vec::new();
    let Some(wins) = prop.value32() else {
        return Ok(out);
    };

    for win in wins {
        let title = conn
            .get_property(false, win, atom_name.atom, atom_utf8.atom, 0, 512)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| String::from_utf8(r.value).ok())
            .unwrap_or_default()
            .trim()
            .to_string();
        if title.is_empty() {
            continue;
        }
        let pid = conn
            .get_property(false, win, atom_pid.atom, AtomEnum::CARDINAL, 0, 1)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().and_then(|mut it| it.next()))
            .unwrap_or(0);
        let process_name = if pid > 0 {
            query_process_exe_name(pid).unwrap_or_else(|| "unknown".to_string())
        } else {
            "unknown".to_string()
        };
        let geom = conn
            .get_geometry(win)
            .ok()
            .and_then(|c| c.reply().ok());
        let (width, height) = geom.map_or((0, 0), |g| (u32::from(g.width), u32::from(g.height)));

        out.push(WindowInfo {
            hwnd: isize::try_from(win).unwrap_or(0),
            title,
            process_id: pid,
            process_name,
            width,
            height,
            is_minimized: false,
        });
    }
    Ok(out)
}

/// Queries the executable name of `pid` from `/proc/{pid}/comm`.
#[must_use]
pub fn query_process_exe_name(pid: u32) -> Option<String> {
    if let Some(p) = query_process_exe_path(pid) {
        if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Queries the full executable path of `pid` from `/proc/{pid}/exe`.
#[must_use]
pub fn query_process_exe_path(pid: u32) -> Option<PathBuf> {
    fs::read_link(format!("/proc/{pid}/exe")).ok()
}

/// Starts the Linux `ashpd` ScreenCast + GStreamer `pipewiresrc` capture worker thread.
pub fn start_capture_thread(
    shared: D3d11SharedContext,
    config: CaptureEngineConfig,
    clock: MonotonicClock,
) -> Result<(CaptureSessionHandle, Receiver<CaptureOutputEvent>), CaptureError> {
    let _ = shared;
    gst::init().map_err(|e| CaptureError::InitFailed(format!("GStreamer init failed: {e}")))?;
    let (cmd_tx, cmd_rx) = bounded::<CaptureControlCommand>(16);
    let (evt_tx, evt_rx) = bounded::<CaptureOutputEvent>(VIDEO_FRAME_CHANNEL_CAPACITY);
    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_c = Arc::clone(&stop_flag);

    let join_handle = thread::Builder::new()
        .name("ikoch-linux-capture".to_string())
        .spawn(move || {
            run_linux_capture_loop(config, clock, cmd_rx, evt_tx, stop_c);
        })
        .map_err(|e| CaptureError::InitFailed(format!("Spawn capture thread failed: {e}")))?;

    Ok((
        CaptureSessionHandle {
            cmd_tx,
            stop_flag,
            join_handle: Some(join_handle),
        },
        evt_rx,
    ))
}

struct PortalPipeWireStream {
    fd: std::os::fd::OwnedFd,
    node_id: u32,
    width: u32,
    height: u32,
}

fn open_portal_screencast(
    target: &CaptureTarget,
    capture_cursor: bool,
) -> Result<PortalPipeWireStream, CaptureError> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| CaptureError::InitFailed(format!("tokio runtime failed: {e}")))?;

    rt.block_on(async move {
        let proxy = Screencast::new()
            .await
            .map_err(|e| CaptureError::InitFailed(format!("ashpd Screencast::new failed: {e}")))?;
        let session = proxy
            .create_session()
            .await
            .map_err(|e| CaptureError::InitFailed(format!("create_session failed: {e}")))?;

        let source_types = match target {
            CaptureTarget::Monitor { .. } => SourceType::Monitor.into(),
            CaptureTarget::SpecificWindow { .. } | CaptureTarget::AutoGame => {
                SourceType::Window | SourceType::Monitor
            }
        };
        let cursor_mode = if capture_cursor {
            CursorMode::Embedded
        } else {
            CursorMode::Hidden
        };

        proxy
            .select_sources(
                &session,
                cursor_mode,
                source_types,
                false,
                None,
                PersistMode::DoNot,
            )
            .await
            .map_err(|e| CaptureError::InitFailed(format!("select_sources failed: {e}")))?;

        let response = proxy
            .start(&session, None)
            .await
            .map_err(|e| CaptureError::InitFailed(format!("screencast start failed: {e}")))?
            .response()
            .map_err(|e| CaptureError::InitFailed(format!("screencast response failed: {e}")))?;

        let stream = response
            .streams()
            .first()
            .ok_or_else(|| CaptureError::InitFailed("No PipeWire stream returned".to_string()))?;

        let node_id = stream.pipe_wire_node_id();
        let (width, height) = stream
            .size()
            .map_or((1920, 1080), |(w, h)| (w.max(1) as u32, h.max(1) as u32));

        let fd = proxy
            .open_pipe_wire_remote(&session)
            .await
            .map_err(|e| CaptureError::InitFailed(format!("open_pipe_wire_remote failed: {e}")))?;

        Ok(PortalPipeWireStream {
            fd,
            node_id,
            width,
            height,
        })
    })
}

fn run_linux_capture_loop(
    mut config: CaptureEngineConfig,
    clock: MonotonicClock,
    cmd_rx: Receiver<CaptureControlCommand>,
    evt_tx: Sender<CaptureOutputEvent>,
    stop_flag: Arc<AtomicBool>,
) {
    let mut epoch: u64 = 1;
    let mut current_target = config.initial_target.clone();

    while !stop_flag.load(Ordering::Relaxed) {
        let portal = match open_portal_screencast(&current_target, config.capture_cursor) {
            Ok(p) => p,
            Err(err) => {
                warn!("Linux ScreenCast portal unavailable: {err}");
                let _ = evt_tx.try_send(CaptureOutputEvent::RecoverableWarning(format!(
                    "Wayland/PipeWire ScreenCast unavailable: {err}"
                )));
                thread::sleep(Duration::from_secs(2));
                continue;
            }
        };

        let (out_w, out_h) = config.resolution.resolve(portal.width, portal.height);
        let pipeline_str = format!(
            "pipewiresrc fd={} path={} do-timestamp=true ! videoconvert ! videoscale ! video/x-raw,format=NV12,width={},height={},framerate={}/1 ! appsink name=sink emit-signals=false sync=false max-buffers=2 drop=true",
            portal.fd.as_raw_fd(),
            portal.node_id,
            out_w,
            out_h,
            config.fps.max(15)
        );

        let pipeline = match gst::parse::launch(&pipeline_str) {
            Ok(el) => match el.downcast::<gst::Pipeline>() {
                Ok(p) => p,
                Err(_) => {
                    warn!("GStreamer element is not a Pipeline");
                    thread::sleep(Duration::from_secs(1));
                    continue;
                }
            },
            Err(err) => {
                warn!("Failed to launch pipewiresrc pipeline: {err}");
                thread::sleep(Duration::from_secs(1));
                continue;
            }
        };

        let Some(sink_el) = pipeline.by_name("sink") else {
            warn!("Missing appsink in capture pipeline");
            continue;
        };
        let Ok(appsink) = sink_el.downcast::<gst_app::AppSink>() else {
            continue;
        };

        if pipeline.set_state(gst::State::Playing).is_err() {
            warn!("Failed to start GStreamer pipewiresrc pipeline");
            let _ = pipeline.set_state(gst::State::Null);
            thread::sleep(Duration::from_secs(1));
            continue;
        }

        let display_label = match &current_target {
            CaptureTarget::Monitor { name, .. } => name.clone(),
            CaptureTarget::SpecificWindow { title, .. } => title.clone(),
            CaptureTarget::AutoGame => "Auto Game".to_string(),
        };
        info!(
            "Linux PipeWire capture started [epoch={epoch}]: {} ({}x{})",
            display_label, out_w, out_h
        );
        let _ = evt_tx.send(CaptureOutputEvent::SourceChanged {
            epoch,
            target: current_target.clone(),
            display_label,
            width: out_w,
            height: out_h,
        });

        let mut frame_seq: u64 = 0;
        let mut restart_needed = false;

        while !stop_flag.load(Ordering::Relaxed) && !restart_needed {
            loop {
                match cmd_rx.try_recv() {
                    Ok(CaptureControlCommand::Stop) => {
                        let _ = pipeline.set_state(gst::State::Null);
                        return;
                    }
                    Ok(CaptureControlCommand::SwitchTarget(new_t)) => {
                        current_target = new_t;
                        epoch = epoch.saturating_add(1);
                        restart_needed = true;
                        break;
                    }
                    Ok(CaptureControlCommand::SetResolution(new_res)) => {
                        config.resolution = new_res;
                        epoch = epoch.saturating_add(1);
                        restart_needed = true;
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        let _ = pipeline.set_state(gst::State::Null);
                        return;
                    }
                }
            }
            if restart_needed {
                break;
            }

            if let Some(sample) = appsink.try_pull_sample(gst::ClockTime::from_mseconds(25)) {
                if let (Some(buffer), Some(caps)) = (sample.buffer(), sample.caps()) {
                    if let Ok(info) = gst_video::VideoInfo::from_caps(caps) {
                        if let Ok(map) = buffer.map_readable() {
                            let pts_us = clock.now_us();
                            frame_seq = frame_seq.saturating_add(1);
                            let w = info.width();
                            let h = info.height();
                            let stride = info.stride().first().copied().unwrap_or(w as i32).max(0) as u32;
                            let frame = GpuTextureFrame {
                                nv12_bytes: map.as_slice().to_vec(),
                                stride,
                                descriptor: GpuFrameDescriptor {
                                    pts_us,
                                    width: w,
                                    height: h,
                                    format: PixelFormat::Nv12,
                                    is_hdr: false,
                                    adapter_luid: 0,
                                    frame_number: frame_seq,
                                },
                                source_epoch: epoch,
                            };
                            let _ = evt_tx.try_send(CaptureOutputEvent::Nv12Frame(frame));
                        }
                    }
                }
            }
        }

        let _ = pipeline.set_state(gst::State::Null);
    }
}
