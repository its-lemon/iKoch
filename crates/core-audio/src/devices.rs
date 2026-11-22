//! WASAPI render (loopback) and capture (microphone) endpoint enumeration and live peak probing.

use crate::resampler::linear_to_dbfs;
use core_types::{ActiveAudioAppInfo, AudioDeviceInfo, AudioError};
use std::collections::HashSet;
use std::path::Path;
use std::thread;
use std::time::Duration;
use wasapi::{
    initialize_mta, AudioMeterInformation, DeviceCollection, DeviceEnumerator, DeviceState,
    Direction, Role,
};
use windows::core::{Interface, PWSTR};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Media::Audio::{
    eMultimedia, eRender, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator,
    MMDeviceEnumerator,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

/// Lists all active audio output (render) devices suitable for WASAPI loopback capture.
pub fn enumerate_render_devices() -> Result<Vec<AudioDeviceInfo>, AudioError> {
    enumerate_direction(Direction::Render)
}

/// Lists all active audio input (microphone) devices.
pub fn enumerate_capture_devices() -> Result<Vec<AudioDeviceInfo>, AudioError> {
    enumerate_direction(Direction::Capture)
}

/// Enumerates all active Render devices along with their Windows default role flags
/// (`Console` / `Multimedia` / `Communications`) and a live ~150 ms meter peak (`0.0..=1.0`).
pub fn enumerate_render_devices_with_activity(
) -> Result<Vec<(AudioDeviceInfo, f32, String)>, AudioError> {
    let _ = initialize_mta();
    let enumerator = DeviceEnumerator::new().map_err(|err| {
        AudioError::WasapiInitFailed(format!("DeviceEnumerator::new failed: {err}"))
    })?;

    let console_id = enumerator
        .get_default_device_for_role(Direction::Render, Role::Console)
        .ok()
        .and_then(|d| d.get_id().ok());
    let multimedia_id = enumerator
        .get_default_device_for_role(Direction::Render, Role::Multimedia)
        .ok()
        .and_then(|d| d.get_id().ok());
    let comm_id = enumerator
        .get_default_device_for_role(Direction::Render, Role::Communications)
        .ok()
        .and_then(|d| d.get_id().ok());

    let collection: DeviceCollection = enumerator
        .get_device_collection(Direction::Render)
        .map_err(|err| {
            AudioError::WasapiInitFailed(format!("get_device_collection(Render) failed: {err}"))
        })?;

    let count = collection.get_nbr_devices().unwrap_or(0);
    struct ProbeItem {
        info: AudioDeviceInfo,
        roles: String,
        meter: Option<AudioMeterInformation>,
        peak: f32,
    }
    let mut items: Vec<ProbeItem> = Vec::with_capacity(count as usize);

    for idx in 0..count {
        let Ok(dev) = collection.get_device_at_index(idx) else {
            continue;
        };
        if !matches!(dev.get_state(), Ok(DeviceState::Active)) {
            continue;
        }
        let Ok(id) = dev.get_id() else {
            continue;
        };
        let name = dev
            .get_friendlyname()
            .or_else(|_| dev.get_description())
            .unwrap_or_else(|_| format!("Audio Device {}", idx + 1));

        let mut roles_vec = Vec::new();
        if console_id.as_deref() == Some(id.as_str()) {
            roles_vec.push("Console");
        }
        if multimedia_id.as_deref() == Some(id.as_str()) {
            roles_vec.push("Multimedia");
        }
        if comm_id.as_deref() == Some(id.as_str()) {
            roles_vec.push("Communications");
        }
        let is_default = !roles_vec.is_empty();
        let roles = if roles_vec.is_empty() {
            "-".to_string()
        } else {
            roles_vec.join("+")
        };

        let meter = dev.get_audiometerinformation().ok();
        items.push(ProbeItem {
            info: AudioDeviceInfo {
                id,
                name,
                is_default,
            },
            roles,
            meter,
            peak: 0.0,
        });
    }

    // Sample meter peaks across ~150 ms so bursty playback audio is caught reliably.
    for sample_idx in 0..6 {
        for item in &mut items {
            if let Some(meter) = item.meter.as_ref() {
                if let Ok(p) = meter.get_peak_value() {
                    item.peak = item.peak.max(p);
                }
            }
        }
        if sample_idx < 5 {
            thread::sleep(Duration::from_millis(25));
        }
    }

    let mut result: Vec<(AudioDeviceInfo, f32, String)> = items
        .into_iter()
        .map(|it| {
            let role_note = format!("roles={}, dBFS={:.1}", it.roles, linear_to_dbfs(it.peak));
            (it.info, it.peak, role_note)
        })
        .collect();

    result.sort_by(|a, b| {
        b.0.is_default
            .cmp(&a.0.is_default)
            .then_with(|| b.1.total_cmp(&a.1))
            .then_with(|| a.0.name.cmp(&b.0.name))
    });
    Ok(result)
}

fn enumerate_direction(direction: Direction) -> Result<Vec<AudioDeviceInfo>, AudioError> {
    let _ = initialize_mta();
    let enumerator = DeviceEnumerator::new().map_err(|err| {
        AudioError::WasapiInitFailed(format!("DeviceEnumerator::new failed: {err}"))
    })?;

    let default_id = enumerator
        .get_default_device(direction)
        .ok()
        .and_then(|dev| dev.get_id().ok());

    let collection: DeviceCollection = enumerator
        .get_device_collection(direction)
        .map_err(|err| {
            AudioError::WasapiInitFailed(format!(
                "get_device_collection({direction:?}) failed: {err}"
            ))
        })?;

    let count = collection.get_nbr_devices().unwrap_or(0);
    let mut devices = Vec::with_capacity(count as usize);

    for idx in 0..count {
        let Ok(dev) = collection.get_device_at_index(idx) else {
            continue;
        };
        if !matches!(dev.get_state(), Ok(DeviceState::Active)) {
            continue;
        }
        let Ok(id) = dev.get_id() else {
            continue;
        };
        let name = dev
            .get_friendlyname()
            .or_else(|_| dev.get_description())
            .unwrap_or_else(|_| format!("Audio Device {}", idx + 1));
        let is_default = default_id.as_deref() == Some(id.as_str());
        devices.push(AudioDeviceInfo {
            id,
            name,
            is_default,
        });
    }

    devices.sort_by(|a, b| b.is_default.cmp(&a.is_default).then_with(|| a.name.cmp(&b.name)));
    Ok(devices)
}

/// Enumerates running Windows applications that currently have an active audio session
/// on the default multimedia render endpoint (`IAudioSessionManager2` / `IAudioSessionEnumerator`).
pub fn enumerate_active_audio_apps() -> Result<Vec<ActiveAudioAppInfo>, AudioError> {
    // SAFETY: COM initialization & Windows CoreAudio session enumeration with scoped handles.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| {
                AudioError::WasapiInitFailed(format!("MMDeviceEnumerator failed: {e}"))
            })?;
        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eMultimedia)
            .map_err(|e| {
                AudioError::WasapiInitFailed(format!("GetDefaultAudioEndpoint failed: {e}"))
            })?;
        let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None).map_err(|e| {
            AudioError::WasapiInitFailed(format!("Activate IAudioSessionManager2 failed: {e}"))
        })?;
        let session_enum = manager.GetSessionEnumerator().map_err(|e| {
            AudioError::WasapiInitFailed(format!("GetSessionEnumerator failed: {e}"))
        })?;
        let count = session_enum.GetCount().unwrap_or(0);

        let mut seen_names: HashSet<String> = HashSet::new();
        let mut apps = Vec::new();
        let self_pid = std::process::id();

        for idx in 0..count {
            let Ok(ctrl) = session_enum.GetSession(idx) else {
                continue;
            };
            let Ok(ctrl2) = ctrl.cast::<IAudioSessionControl2>() else {
                continue;
            };
            let Ok(pid) = ctrl2.GetProcessId() else {
                continue;
            };
            if pid == 0 || pid == self_pid {
                continue;
            }
            let Some(process_name) = query_process_exe_name(pid) else {
                continue;
            };
            let lower = process_name.to_ascii_lowercase();
            if lower == "ikoch.exe" || !seen_names.insert(lower) {
                continue;
            }
            let raw_display = ctrl2
                .GetDisplayName()
                .ok()
                .map(|pw| {
                    let s = pw.to_string().unwrap_or_default();
                    windows::Win32::System::Com::CoTaskMemFree(Some(pw.0.cast()));
                    s
                })
                .unwrap_or_default();
            let display_name = if raw_display.trim().is_empty() || raw_display.starts_with('@') {
                process_name
                    .strip_suffix(".exe")
                    .or_else(|| process_name.strip_suffix(".EXE"))
                    .unwrap_or(&process_name)
                    .to_string()
            } else {
                raw_display.trim().to_string()
            };
            apps.push(ActiveAudioAppInfo {
                process_name,
                display_name,
                pid,
            });
        }

        apps.sort_by_key(|a| a.display_name.to_lowercase());
        Ok(apps)
    }
}

/// Resolves the executable filename (e.g. `Discord.exe`) for a running PID, or `None` if exited.
#[must_use]
pub fn find_running_pid_by_process_names(process_names: &[String]) -> Option<u32> {
    if process_names.is_empty() {
        return None;
    }
    let targets: Vec<String> = process_names
        .iter()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    if targets.is_empty() {
        return None;
    }
    let Ok(apps) = enumerate_active_audio_apps() else {
        return None;
    };
    for app in apps {
        if targets.iter().any(|t| *t == app.process_name.to_ascii_lowercase()) {
            return Some(app.pid);
        }
    }
    None
}

unsafe fn query_process_exe_name(pid: u32) -> Option<String> {
    // SAFETY: Caller passes a non-zero PID and we close `handle` before returning.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(handle);
        if !ok || len == 0 {
            return None;
        }
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        Path::new(&full)
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
    }
}
