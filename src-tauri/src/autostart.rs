//! Windows startup registry integration (`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`).
//!
//! Registers or removes `"<exe_path>" --minimized` so iKoch starts silently in the system tray.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use tracing::info;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, WIN32_ERROR};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE,
    REG_SZ,
};

const RUN_SUBKEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const VALUE_NAME: &str = "iKoch";
const LEGACY_VALUE_NAMES: &[&str] = &["Ferrix", "RustClip"];

fn to_wide_null(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// Enables or disables starting iKoch minimized in the tray on Windows sign-in.
pub fn sync_windows_autostart(enabled: bool) -> Result<(), String> {
    let subkey_w = to_wide_null(OsStr::new(RUN_SUBKEY));
    let val_name_w = to_wide_null(OsStr::new(VALUE_NAME));

    let mut hkey = HKEY::default();
    let status = unsafe {
        if enabled {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey_w.as_ptr()),
                Some(0),
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_READ | KEY_WRITE,
                None,
                &mut hkey,
                None,
            )
        } else {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey_w.as_ptr()),
                Some(0),
                KEY_READ | KEY_WRITE,
                &mut hkey,
            )
        }
    };

    if status != WIN32_ERROR(0) {
        if !enabled && status == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        return Err(format!("Failed to open HKCU Run registry key: {status:?}"));
    }

    let result = if enabled {
        let exe_path = std::env::current_exe()
            .map_err(|e| format!("Failed to resolve current_exe for autostart: {e}"))?;
        let command = format!("\"{}\" --minimized", exe_path.display());
        let cmd_w = to_wide_null(OsStr::new(&command));
        let bytes = unsafe {
            std::slice::from_raw_parts(cmd_w.as_ptr().cast::<u8>(), cmd_w.len() * 2)
        };
        let set_res = unsafe {
            RegSetValueExW(
                hkey,
                PCWSTR(val_name_w.as_ptr()),
                Some(0),
                REG_SZ,
                Some(bytes),
            )
        };
        if set_res == WIN32_ERROR(0) {
            for &legacy in LEGACY_VALUE_NAMES {
                let legacy_w = to_wide_null(OsStr::new(legacy));
                let _ = unsafe { RegDeleteValueW(hkey, PCWSTR(legacy_w.as_ptr())) };
            }
            info!("Windows autostart ENABLED -> {}", command);
            Ok(())
        } else {
            Err(format!("RegSetValueExW failed: {set_res:?}"))
        }
    } else {
        let del_res = unsafe { RegDeleteValueW(hkey, PCWSTR(val_name_w.as_ptr())) };
        for &legacy in LEGACY_VALUE_NAMES {
            let legacy_w = to_wide_null(OsStr::new(legacy));
            let _ = unsafe { RegDeleteValueW(hkey, PCWSTR(legacy_w.as_ptr())) };
        }
        if del_res == WIN32_ERROR(0) || del_res == ERROR_FILE_NOT_FOUND {
            info!("Windows autostart DISABLED");
            Ok(())
        } else {
            Err(format!("RegDeleteValueW failed: {del_res:?}"))
        }
    };

    unsafe {
        let _ = RegCloseKey(hkey);
    }
    result
}

/// Checks whether the `iKoch` (or legacy `Ferrix` / `RustClip`) autostart entry is present in `HKCU\...\Run`.
#[must_use]
pub fn is_windows_autostart_enabled() -> bool {
    let subkey_w = to_wide_null(OsStr::new(RUN_SUBKEY));
    let mut hkey = HKEY::default();
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey_w.as_ptr()),
            Some(0),
            KEY_QUERY_VALUE,
            &mut hkey,
        )
    };
    if status != WIN32_ERROR(0) {
        return false;
    }
    let found = std::iter::once(VALUE_NAME)
        .chain(LEGACY_VALUE_NAMES.iter().copied())
        .any(|name| {
            let name_w = to_wide_null(OsStr::new(name));
            let res = unsafe {
                RegQueryValueExW(
                    hkey,
                    PCWSTR(name_w.as_ptr()),
                    None,
                    None,
                    None,
                    None,
                )
            };
            res == WIN32_ERROR(0)
        });
    unsafe {
        let _ = RegCloseKey(hkey);
    }
    found
}
