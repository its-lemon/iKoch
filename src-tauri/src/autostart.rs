//! Windows startup registry integration (`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`)
//! and Linux XDG autostart (`~/.config/autostart/ikoch.desktop`).

#[cfg(windows)]
mod platform_impl {

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
}

#[cfg(target_os = "linux")]
mod platform_impl {
    use std::fs;
    use std::path::PathBuf;
    use tracing::info;

    fn desktop_file_path() -> Result<PathBuf, String> {
        let config_home = std::env::var("XDG_CONFIG_HOME")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".config")))
            .ok_or_else(|| "Cannot resolve XDG_CONFIG_HOME or HOME".to_string())?;
        Ok(config_home.join("autostart").join("ikoch.desktop"))
    }

    pub fn sync_windows_autostart(enabled: bool) -> Result<(), String> {
        let path = desktop_file_path()?;
        if enabled {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|e| format!("create autostart dir: {e}"))?;
            }
            let exe = std::env::current_exe()
                .map_err(|e| format!("resolve current_exe: {e}"))?;
            let contents = format!(
                "[Desktop Entry]\nType=Application\nName=iKoch\nExec=\"{}\" --minimized\nX-GNOME-Autostart-enabled=true\nTerminal=false\n",
                exe.display()
            );
            fs::write(&path, contents).map_err(|e| format!("write autostart .desktop: {e}"))?;
            info!("Linux XDG autostart ENABLED -> {}", path.display());
        } else if path.exists() {
            fs::remove_file(&path).map_err(|e| format!("remove autostart .desktop: {e}"))?;
            info!("Linux XDG autostart DISABLED");
        }
        Ok(())
    }

    #[must_use]
    pub fn is_windows_autostart_enabled() -> bool {
        desktop_file_path().is_ok_and(|p| p.exists())
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod platform_impl {
    pub fn sync_windows_autostart(_enabled: bool) -> Result<(), String> {
        Ok(())
    }

    #[must_use]
    pub fn is_windows_autostart_enabled() -> bool {
        false
    }
}

pub use platform_impl::{is_windows_autostart_enabled, sync_windows_autostart};
