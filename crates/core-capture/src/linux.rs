//! Linux screen/window capture via `ashpd` (`xdg-desktop-portal` ScreenCast + PipeWire)
//! and GStreamer `pipewiresrc` outputting NV12 frames, with `x11rb` / `/proc` enumeration.

use ashpd::desktop::screencast::{CursorMode, Screencast, SourceType};
use ashpd::desktop::PersistMode;
use core_types::{
    CaptureError, CaptureTarget, GpuFrameDescriptor, HdrMode, MonitorInfo, MonotonicClock,
    PixelFormat, ResolutionPreset, WindowInfo, VIDEO_FRAME_CHANNEL_CAPACITY,
};
use crossbeam_channel::{bounded, Receiver, Sender};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use std::fs;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tracing::{error, info, warn};
use x11rb::connection::Connection;
use x11rb::protocol::randr;
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

/// Enumerates all connected monitors via XRandR (X11 / XWayland), falling back to
/// Linux DRM sysfs (`/sys/class/drm/card*-*/`) on pure Wayland, and finally a portal placeholder.
pub fn enumerate_monitors() -> Result<Vec<MonitorInfo>, CaptureError> {
    if let Some(xrandr_monitors) = enumerate_monitors_xrandr() {
        if !xrandr_monitors.is_empty() {
            return Ok(xrandr_monitors);
        }
    }

    let drm_monitors = enumerate_monitors_drm_sysfs();
    if !drm_monitors.is_empty() {
        return Ok(drm_monitors);
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

fn enumerate_monitors_xrandr() -> Option<Vec<MonitorInfo>> {
    let (conn, screen_num) = x11rb::connect(None).ok()?;
    let screen = conn.setup().roots.get(screen_num)?;
    let res = randr::get_screen_resources_current(&conn, screen.root)
        .ok()?
        .reply()
        .ok()?;
    let primary_output = randr::get_output_primary(&conn, screen.root)
        .ok()
        .and_then(|c| c.reply().ok())
        .map(|r| r.output)
        .unwrap_or(0);

    let mut out = Vec::new();
    for &output in &res.outputs {
        let Ok(info_cookie) = randr::get_output_info(&conn, output, res.config_timestamp) else {
            continue;
        };
        let Ok(info) = info_cookie.reply() else {
            continue;
        };
        if info.connection != randr::Connection::CONNECTED || info.crtc == 0 {
            continue;
        }
        let Ok(crtc_cookie) = randr::get_crtc_info(&conn, info.crtc, res.config_timestamp) else {
            continue;
        };
        let Ok(crtc) = crtc_cookie.reply() else {
            continue;
        };
        if crtc.width == 0 || crtc.height == 0 {
            continue;
        }
        let connector = String::from_utf8_lossy(&info.name).trim().to_string();
        let label = if connector.is_empty() {
            format!("Monitor {}", out.len() + 1)
        } else {
            connector.clone()
        };
        let is_primary = output == primary_output || (primary_output == 0 && out.is_empty());
        out.push(MonitorInfo {
            monitor_id: label.clone(),
            name: format!("{label} ({}x{})", crtc.width, crtc.height),
            hmonitor: out.len() as isize,
            width: u32::from(crtc.width),
            height: u32::from(crtc.height),
            x: i32::from(crtc.x),
            y: i32::from(crtc.y),
            is_primary,
        });
    }

    if out.is_empty() {
        out.push(MonitorInfo {
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
        });
    } else if !out.iter().any(|m| m.is_primary) {
        if let Some(first) = out.first_mut() {
            first.is_primary = true;
        }
    }

    Some(out)
}

fn enumerate_monitors_drm_sysfs() -> Vec<MonitorInfo> {
    let Ok(entries) = fs::read_dir("/sys/class/drm") else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| name.starts_with("card") && name.contains('-'))
        })
        .collect();
    paths.sort();

    let mut out = Vec::new();
    let mut x_offset = 0i32;
    for path in paths {
        let Ok(status) = fs::read_to_string(path.join("status")) else {
            continue;
        };
        if status.trim() != "connected" {
            continue;
        }
        let (width, height) = fs::read_to_string(path.join("modes"))
            .ok()
            .and_then(|modes| {
                modes.lines().find_map(|line| {
                    let (w_str, h_str) = line.trim().split_once('x')?;
                    let w = w_str.parse::<u32>().ok()?;
                    let h = h_str
                        .trim_end_matches(|c: char| !c.is_ascii_digit())
                        .parse::<u32>()
                        .ok()?;
                    (w > 0 && h > 0).then_some((w, h))
                })
            })
            .unwrap_or((1920, 1080));

        let raw_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Display");
        let connector = raw_name
            .split_once('-')
            .map_or(raw_name, |(_, rest)| rest)
            .to_string();

        out.push(MonitorInfo {
            monitor_id: connector.clone(),
            name: format!("{connector} ({width}x{height})"),
            hmonitor: out.len() as isize,
            width,
            height,
            x: x_offset,
            y: 0,
            is_primary: out.is_empty(),
        });
        x_offset = x_offset.saturating_add(width as i32);
    }
    out
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

fn restore_token_path() -> PathBuf {
    core_types::ikoch_logs_dir()
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
        .join("screencast_restore_token")
}

fn load_restore_token() -> Option<String> {
    fs::read_to_string(restore_token_path())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn save_restore_token(token: &str) {
    let path = restore_token_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, token);
}

fn clear_restore_token() {
    let _ = fs::remove_file(restore_token_path());
}

fn open_portal_screencast(
    target: &CaptureTarget,
    capture_cursor: bool,
) -> Result<PortalPipeWireStream, CaptureError> {
    let cached = load_restore_token();
    match open_portal_screencast_once(target, capture_cursor, cached.as_deref()) {
        Ok(stream) => Ok(stream),
        Err(err) if cached.is_some() => {
            warn!("Cached Wayland ScreenCast restore_token failed ({err}); clearing token and retrying");
            clear_restore_token();
            open_portal_screencast_once(target, capture_cursor, None)
        }
        Err(err) => Err(err),
    }
}

fn open_portal_screencast_once(
    target: &CaptureTarget,
    capture_cursor: bool,
    restore_token: Option<&str>,
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
                restore_token,
                PersistMode::ExplicitlyRevoked,
            )
            .await
            .map_err(|e| CaptureError::InitFailed(format!("select_sources failed: {e}")))?;

        let response = proxy
            .start(&session, None)
            .await
            .map_err(|e| CaptureError::InitFailed(format!("screencast start failed: {e}")))?
            .response()
            .map_err(|e| CaptureError::InitFailed(format!("screencast response failed: {e}")))?;

        if let Some(new_token) = response.restore_token() {
            save_restore_token(new_token);
        }

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

fn drain_gst_bus_errors(pipeline: &gst::Pipeline) -> bool {
    let Some(bus) = pipeline.bus() else {
        return false;
    };
    let mut fatal = false;
    while let Some(msg) = bus.pop() {
        match msg.view() {
            gst::MessageView::Error(err) => {
                error!(
                    "GStreamer capture error from {:?}: {} ({:?})",
                    err.src().map(|s| s.path_string()),
                    err.error(),
                    err.debug()
                );
                fatal = true;
            }
            gst::MessageView::Warning(w) => {
                warn!(
                    "GStreamer capture warning from {:?}: {} ({:?})",
                    w.src().map(|s| s.path_string()),
                    w.error(),
                    w.debug()
                );
            }
            gst::MessageView::Eos(_) => {
                warn!("GStreamer capture stream ended (EOS)");
                fatal = true;
            }
            _ => {}
        }
    }
    fatal
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
        let caps_str = if config.resolution == ResolutionPreset::Native {
            "video/x-raw,format=NV12".to_string()
        } else {
            format!("video/x-raw,format=NV12,width={out_w},height={out_h}")
        };
        let pipeline_str = format!(
            "pipewiresrc fd={} path={} do-timestamp=true ! videoconvert ! videoscale ! {} ! appsink name=sink emit-signals=false sync=false max-buffers=2 drop=true",
            portal.fd.as_raw_fd(),
            portal.node_id,
            caps_str
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
                error!("Failed to launch pipewiresrc pipeline ({pipeline_str}): {err}");
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
            error!("Failed to start GStreamer pipewiresrc pipeline: {pipeline_str}");
            let _ = drain_gst_bus_errors(&pipeline);
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
            "Linux PipeWire capture started [epoch={epoch}]: {} (portal={}x{}, out={}x{})",
            display_label, portal.width, portal.height, out_w, out_h
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
            if let Ok(cmd) = cmd_rx.try_recv() {
                match cmd {
                    CaptureControlCommand::Stop => {
                        let _ = pipeline.set_state(gst::State::Null);
                        return;
                    }
                    CaptureControlCommand::SwitchTarget(new_t) => {
                        clear_restore_token();
                        current_target = new_t;
                        epoch = epoch.saturating_add(1);
                        restart_needed = true;
                    }
                    CaptureControlCommand::SetResolution(new_res) => {
                        config.resolution = new_res;
                        epoch = epoch.saturating_add(1);
                        restart_needed = true;
                    }
                }
            }
            if restart_needed {
                break;
            }
            if drain_gst_bus_errors(&pipeline) {
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
