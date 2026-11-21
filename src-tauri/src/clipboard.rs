//! Cross-platform Shell File Clipboard:
//! - Windows: `CF_HDROP` (`DROPFILES`)
//! - Linux: `text/uri-list` via `wl-copy` (Wayland) or `xclip` (X11)

#[cfg(windows)]
mod platform_impl {
use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE, GMEM_ZEROINIT,
};

const CF_HDROP: u32 = 15;
const DROPFILES_HEADER_BYTES: usize = 20;

pub(super) fn normalize_windows_clipboard_path(path: &Path) -> Result<PathBuf, String> {
    if !path.exists() {
        return Err(format!("File '{}' does not exist", path.display()));
    }
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| format!("current_dir failed: {e}"))?
            .join(path)
    };
    let canonical = abs.canonicalize().unwrap_or(abs);
    let raw = canonical.to_string_lossy();
    let clean = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
    Ok(PathBuf::from(clean))
}

fn open_clipboard_with_retry() -> Result<(), String> {
    for attempt in 0..6 {
        if unsafe { OpenClipboard(None) }.is_ok() {
            return Ok(());
        }
        if attempt < 5 {
            thread::sleep(Duration::from_millis(15));
        }
    }
    Err("OpenClipboard failed after retries".to_string())
}

struct ClipboardGuard;

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        let _ = unsafe { CloseClipboard() };
    }
}

/// Copies `path` to the Windows clipboard as a `CF_HDROP` file drop object for instant `Ctrl+V` pasting.
pub fn copy_file_to_clipboard_cf_hdrop(path: &Path) -> Result<(), String> {
    let clean_path = normalize_windows_clipboard_path(path)?;
    let mut wide_chars: Vec<u16> = clean_path.as_os_str().encode_wide().collect();
    // CF_HDROP requires a double-NUL terminated list of UTF-16LE paths.
    wide_chars.push(0);
    wide_chars.push(0);

    let total_bytes = DROPFILES_HEADER_BYTES + wide_chars.len() * size_of::<u16>();
    let hglobal: HGLOBAL = unsafe { GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, total_bytes) }
        .map_err(|e| format!("GlobalAlloc failed: {e}"))?;

    let locked_ptr = unsafe { GlobalLock(hglobal) };
    if locked_ptr.is_null() {
        let _ = unsafe { GlobalFree(Some(hglobal)) };
        return Err("GlobalLock failed while building CF_HDROP".to_string());
    }

    unsafe {
        let buf = std::slice::from_raw_parts_mut(locked_ptr.cast::<u8>(), total_bytes);
        buf.fill(0);
        // DROPFILES struct: pFiles = 20 (u32 LE), pt = (0,0), fNC = 0, fWide = 1 (u32 LE) at offset 16.
        buf[0..4].copy_from_slice(&(DROPFILES_HEADER_BYTES as u32).to_le_bytes());
        buf[16..20].copy_from_slice(&1u32.to_le_bytes());

        let wide_bytes = std::slice::from_raw_parts(
            wide_chars.as_ptr().cast::<u8>(),
            wide_chars.len() * size_of::<u16>(),
        );
        buf[DROPFILES_HEADER_BYTES..DROPFILES_HEADER_BYTES + wide_bytes.len()]
            .copy_from_slice(wide_bytes);
    }
    let _ = unsafe { GlobalUnlock(hglobal) };

    if let Err(err) = open_clipboard_with_retry() {
        let _ = unsafe { GlobalFree(Some(hglobal)) };
        return Err(err);
    }
    let _guard = ClipboardGuard;

    if let Err(err) = unsafe { EmptyClipboard() } {
        let _ = unsafe { GlobalFree(Some(hglobal)) };
        return Err(format!("EmptyClipboard failed: {err}"));
    }

    // SetClipboardData takes ownership of `hglobal` when it succeeds.
    if let Err(err) = unsafe { SetClipboardData(CF_HDROP, Some(HANDLE(hglobal.0))) } {
        let _ = unsafe { GlobalFree(Some(hglobal)) };
        return Err(format!("SetClipboardData(CF_HDROP) failed: {err}"));
    }

    Ok(())
}

/// Reads all file paths currently stored in the Windows clipboard under `CF_HDROP`.
pub fn read_clipboard_cf_hdrop_files() -> Result<Vec<PathBuf>, String> {
    open_clipboard_with_retry()?;
    let _guard = ClipboardGuard;

    let handle = unsafe { GetClipboardData(CF_HDROP) }
        .map_err(|e| format!("GetClipboardData(CF_HDROP) failed: {e}"))?;
    if handle.0.is_null() {
        return Ok(Vec::new());
    }

    let hglobal = HGLOBAL(handle.0);
    let total_size = unsafe { GlobalSize(hglobal) };
    if total_size < DROPFILES_HEADER_BYTES {
        return Err("Clipboard CF_HDROP buffer smaller than DROPFILES header".to_string());
    }

    let ptr = unsafe { GlobalLock(hglobal) };
    if ptr.is_null() {
        return Err("GlobalLock failed while reading CF_HDROP".to_string());
    }

    let mut files = Vec::new();
    unsafe {
        let buf = std::slice::from_raw_parts(ptr.cast::<u8>(), total_size);
        let p_files = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        let f_wide = u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]) != 0;

        if p_files < buf.len() && f_wide {
            let tail = &buf[p_files..];
            let u16_count = tail.len() / 2;
            let mut wide = Vec::with_capacity(u16_count);
            for chunk in tail[..u16_count * 2].chunks_exact(2) {
                wide.push(u16::from_le_bytes([chunk[0], chunk[1]]));
            }
            for segment in wide.split(|&ch| ch == 0) {
                if segment.is_empty() {
                    break;
                }
                let s = OsString::from_wide(segment);
                files.push(PathBuf::from(s));
            }
        }
    }
    let _ = unsafe { GlobalUnlock(hglobal) };
    Ok(files)
}
}

#[cfg(target_os = "linux")]
mod platform_impl {
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    /// Copies `path` to the Linux Wayland/X11 clipboard as `text/uri-list` for `Ctrl+V` pasting.
    pub fn copy_file_to_clipboard_cf_hdrop(path: &Path) -> Result<(), String> {
        if !path.exists() {
            return Err(format!("File '{}' does not exist", path.display()));
        }
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let uri = format!("file://{}\r\n", canonical.display());

        if let Ok(mut child) = Command::new("wl-copy")
            .args(["--type", "text/uri-list"])
            .stdin(Stdio::piped())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(uri.as_bytes());
            }
            if child.wait().is_ok_and(|s| s.success()) {
                return Ok(());
            }
        }

        let mut child = Command::new("xclip")
            .args(["-selection", "clipboard", "-t", "text/uri-list", "-i"])
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Install wl-clipboard (Wayland) or xclip (X11): {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(uri.as_bytes());
        }
        let status = child.wait().map_err(|e| format!("xclip wait failed: {e}"))?;
        if !status.success() {
            return Err("xclip failed to write text/uri-list".to_string());
        }
        Ok(())
    }

    pub fn read_clipboard_cf_hdrop_files() -> Result<Vec<PathBuf>, String> {
        Ok(Vec::new())
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod platform_impl {
    use std::path::{Path, PathBuf};

    pub fn copy_file_to_clipboard_cf_hdrop(_path: &Path) -> Result<(), String> {
        Err("Clipboard file copy is not supported on this platform".to_string())
    }

    pub fn read_clipboard_cf_hdrop_files() -> Result<Vec<PathBuf>, String> {
        Ok(Vec::new())
    }
}

pub use platform_impl::{copy_file_to_clipboard_cf_hdrop, read_clipboard_cf_hdrop_files};

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::fs;
    use uuid::Uuid;

    #[test]
    fn cf_hdrop_clipboard_roundtrip_places_file_for_ctrl_v() -> Result<(), String> {
        let temp_dir = std::env::temp_dir().join(format!("ikoch_clip_{}", Uuid::new_v4()));
        fs::create_dir_all(&temp_dir).map_err(|e| e.to_string())?;
        let test_mp4 = temp_dir.join("test_clipboard_clip.mp4");
        fs::write(&test_mp4, b"dummy mp4 bytes").map_err(|e| e.to_string())?;

        copy_file_to_clipboard_cf_hdrop(&test_mp4)?;
        let pasted = read_clipboard_cf_hdrop_files()?;
        let expected = platform_impl::normalize_windows_clipboard_path(&test_mp4)?;

        assert!(
            pasted.iter().any(|p| {
                p.to_string_lossy()
                    .eq_ignore_ascii_case(&expected.to_string_lossy())
            }),
            "expected '{}' in clipboard CF_HDROP, got {:?}",
            expected.display(),
            pasted
        );

        let _ = fs::remove_dir_all(&temp_dir);
        Ok(())
    }
}
