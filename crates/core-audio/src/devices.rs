//! WASAPI render (loopback) and capture (microphone) endpoint enumeration and live peak probing.

use crate::resampler::linear_to_dbfs;
use core_types::{AudioDeviceInfo, AudioError};
use std::thread;
use std::time::Duration;
use wasapi::{
    initialize_mta, AudioMeterInformation, DeviceCollection, DeviceEnumerator, DeviceState,
    Direction, Role,
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
