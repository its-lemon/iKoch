//! Enumeration of monitors and windows plus creation of `GraphicsCaptureItem` handles
//! for dynamic `CaptureTarget` switching.

use core_types::{CaptureError, CaptureTarget, MonitorInfo, WindowInfo};
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use windows::core::{factory, BOOL, HSTRING, PWSTR};
use windows::Graphics::Capture::GraphicsCaptureItem;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromWindow, HDC, HMONITOR, MONITORINFOEXW,
    MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClientRect, GetForegroundWindow, GetShellWindow, GetWindowLongW,
    GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible,
    GWL_EXSTYLE, WS_EX_TOOLWINDOW,
};

/// Enumerates all currently attached display monitors.
pub fn enumerate_monitors() -> Result<Vec<MonitorInfo>, CaptureError> {
    let mut monitors: Vec<MonitorInfo> = Vec::new();
    let ptr = &mut monitors as *mut Vec<MonitorInfo>;
    let ok = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(enum_monitor_proc),
            LPARAM(ptr as isize),
        )
    };
    if !ok.as_bool() && monitors.is_empty() {
        return Err(CaptureError::InitFailed(
            "EnumDisplayMonitors failed to discover any display".to_string(),
        ));
    }
    Ok(monitors)
}

unsafe extern "system" fn enum_monitor_proc(
    hmonitor: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let list_ptr = lparam.0 as *mut Vec<MonitorInfo>;
    if list_ptr.is_null() {
        return BOOL(0);
    }

    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    if !unsafe { GetMonitorInfoW(hmonitor, &mut info.monitorInfo as *mut _ as *mut _) }.as_bool() {
        return BOOL(1);
    }

    let rc = info.monitorInfo.rcMonitor;
    let width = u32::try_from(rc.right - rc.left).unwrap_or(0);
    let height = u32::try_from(rc.bottom - rc.top).unwrap_or(0);
    let is_primary = (info.monitorInfo.dwFlags & 1) != 0; // MONITORINFOF_PRIMARY

    let raw_name = &info.szDevice;
    let nul_pos = raw_name
        .iter()
        .position(|&ch| ch == 0)
        .unwrap_or(raw_name.len());
    let device_name = OsString::from_wide(&raw_name[..nul_pos])
        .to_string_lossy()
        .into_owned();

    let list = unsafe { &mut *list_ptr };
    let display_idx = list.len() + 1;
    let friendly_name = if is_primary {
        format!("Monitor {display_idx} (Primary, {width}x{height})")
    } else {
        format!("Monitor {display_idx} ({width}x{height})")
    };

    list.push(MonitorInfo {
        monitor_id: device_name,
        name: friendly_name,
        hmonitor: hmonitor.0 as isize,
        width,
        height,
        x: rc.left,
        y: rc.top,
        is_primary,
    });

    BOOL(1)
}

/// Enumerates visible, non-cloaked, non-tool top-level windows eligible for WGC capture.
pub fn enumerate_windows() -> Result<Vec<WindowInfo>, CaptureError> {
    let mut windows_list: Vec<WindowInfo> = Vec::new();
    let ptr = &mut windows_list as *mut Vec<WindowInfo>;
    if let Err(err) = unsafe { EnumWindows(Some(enum_window_proc), LPARAM(ptr as isize)) } {
        tracing::debug!("EnumWindows returned error (returning collected windows): {err}");
    }
    Ok(windows_list)
}

unsafe extern "system" fn enum_window_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let list_ptr = lparam.0 as *mut Vec<WindowInfo>;
    if list_ptr.is_null() {
        return BOOL(0);
    }

    if !is_capturable_window(hwnd) {
        return BOOL(1);
    }

    let title = read_window_title(hwnd);
    if title.is_empty() {
        return BOOL(1);
    }

    let mut pid: u32 = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 || pid == std::process::id() {
        return BOOL(1);
    }

    let process_name = query_process_exe_name(pid).unwrap_or_else(|| format!("pid_{pid}.exe"));
    let is_minimized = unsafe { IsIconic(hwnd) }.as_bool();
    let mut rect = RECT::default();
    let (width, height) = if unsafe { GetClientRect(hwnd, &mut rect) }.is_ok() {
        (
            u32::try_from(rect.right - rect.left).unwrap_or(0),
            u32::try_from(rect.bottom - rect.top).unwrap_or(0),
        )
    } else {
        (0, 0)
    };

    let list = unsafe { &mut *list_ptr };
    list.push(WindowInfo {
        hwnd: hwnd.0 as isize,
        process_id: pid,
        process_name,
        title,
        width,
        height,
        is_minimized,
    });

    BOOL(1)
}

/// Checks whether an `HWND` is visible, uncloaked, non-shell, and not a tool window.
#[must_use]
pub fn is_capturable_window(hwnd: HWND) -> bool {
    if hwnd.0.is_null() || !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
        return false;
    }
    if hwnd == unsafe { GetShellWindow() } {
        return false;
    }
    if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
        return false;
    }
    let ex_style = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32;
    if (ex_style & WS_EX_TOOLWINDOW.0) != 0 {
        return false;
    }
    let mut cloaked: u32 = 0;
    let dwm_res = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&mut cloaked as *mut u32).cast(),
            size_of::<u32>() as u32,
        )
    };
    !(dwm_res.is_ok() && cloaked != 0)
}

/// Reads the window title string for an `HWND`.
#[must_use]
pub fn read_window_title(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    let len = unsafe { GetWindowTextW(hwnd, &mut buf) };
    if len <= 0 {
        return String::new();
    }
    OsString::from_wide(&buf[..len as usize])
        .to_string_lossy()
        .trim()
        .to_string()
}

/// Queries the full executable path (e.g., `C:\...\cs2.exe`) for a PID.
#[must_use]
pub fn query_process_exe_path(pid: u32) -> Option<PathBuf> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buf = [0u16; 1024];
    let mut size = buf.len() as u32;
    let ok = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut size,
        )
    };
    let _ = unsafe { CloseHandle(handle) };
    ok.ok()?;

    let path_str = OsString::from_wide(&buf[..size as usize])
        .to_string_lossy()
        .into_owned();
    Some(PathBuf::from(path_str))
}

/// Queries the executable basename (e.g., `"cs2.exe"`) for a PID.
#[must_use]
pub fn query_process_exe_name(pid: u32) -> Option<String> {
    query_process_exe_path(pid)?
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

/// Resolved capture item with metadata needed by the WGC capture loop.
pub struct ResolvedCaptureItem {
    pub item: GraphicsCaptureItem,
    pub display_label: String,
    pub window_hwnd: Option<HWND>,
}

/// Creates a WinRT `GraphicsCaptureItem` for any `CaptureTarget` variant.
pub fn create_capture_item(
    target: &CaptureTarget,
    fallback_primary_for_autogame: bool,
) -> Result<ResolvedCaptureItem, CaptureError> {
    let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(|err| {
        CaptureError::InitFailed(format!(
            "Failed to acquire IGraphicsCaptureItemInterop: {err}"
        ))
    })?;

    match target {
        CaptureTarget::Monitor { monitor_id, name } => {
            let monitors = enumerate_monitors()?;
            let selected = monitors
                .iter()
                .find(|m| !monitor_id.is_empty() && m.monitor_id.eq_ignore_ascii_case(monitor_id))
                .or_else(|| monitors.iter().find(|m| m.is_primary))
                .or_else(|| monitors.first())
                .ok_or_else(|| {
                    CaptureError::InitFailed("No active display monitor found".to_string())
                })?;

            let hmonitor = HMONITOR(selected.hmonitor as *mut _);
            let item: GraphicsCaptureItem = unsafe { interop.CreateForMonitor(hmonitor) }
                .map_err(|err| {
                    CaptureError::InitFailed(format!(
                        "CreateForMonitor({}) failed: {err}",
                        selected.monitor_id
                    ))
                })?;
            let label = if name.is_empty() {
                selected.name.clone()
            } else {
                name.clone()
            };
            Ok(ResolvedCaptureItem {
                item,
                display_label: label,
                window_hwnd: None,
            })
        }
        CaptureTarget::SpecificWindow {
            hwnd,
            process_name,
            title,
        } => {
            let hwnd_handle = HWND(*hwnd as *mut _);
            if !unsafe { IsWindow(Some(hwnd_handle)) }.as_bool() {
                return Err(CaptureError::TargetClosed);
            }
            let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(hwnd_handle) }
                .map_err(|err| {
                    CaptureError::InitFailed(format!(
                        "CreateForWindow({hwnd:#x}, {process_name}) failed: {err}"
                    ))
                })?;
            let label = if !title.is_empty() {
                title.clone()
            } else {
                read_window_title(hwnd_handle)
            };
            Ok(ResolvedCaptureItem {
                item,
                display_label: label,
                window_hwnd: Some(hwnd_handle),
            })
        }
        CaptureTarget::AutoGame => {
            let fg = unsafe { GetForegroundWindow() };
            if !fallback_primary_for_autogame && is_capturable_window(fg) {
                let mut pid = 0;
                unsafe { GetWindowThreadProcessId(fg, Some(&mut pid)) };
                if pid != 0 && pid != std::process::id() {
                    if let Ok(item) = unsafe { interop.CreateForWindow::<GraphicsCaptureItem>(fg) }
                    {
                        let title = read_window_title(fg);
                        return Ok(ResolvedCaptureItem {
                            item,
                            display_label: if title.is_empty() {
                                "Foreground Window".to_string()
                            } else {
                                title
                            },
                            window_hwnd: Some(fg),
                        });
                    }
                }
            }

            if fallback_primary_for_autogame {
                let hmonitor = unsafe {
                    MonitorFromWindow(fg, MONITOR_DEFAULTTOPRIMARY)
                };
                let item: GraphicsCaptureItem = unsafe { interop.CreateForMonitor(hmonitor) }
                    .map_err(|err| {
                        CaptureError::InitFailed(format!(
                            "AutoGame fallback CreateForMonitor failed: {err}"
                        ))
                    })?;
                let label = item
                    .DisplayName()
                    .unwrap_or_else(|_| HSTRING::from("Primary Monitor"))
                    .to_string_lossy();
                return Ok(ResolvedCaptureItem {
                    item,
                    display_label: label,
                    window_hwnd: None,
                });
            }

            Err(CaptureError::TargetClosed)
        }
    }
}
