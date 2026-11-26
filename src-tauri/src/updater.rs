//! GitHub Releases auto-updater using built-in Windows `curl.exe` (`CREATE_NO_WINDOW`).

use crate::state::AppState;
use serde::{Deserialize, Serialize};
use std::fs;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use tauri::{AppHandle, Manager};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const GITHUB_LATEST_RELEASE_URL: &str =
    "https://api.github.com/repos/its-lemon/iKoch/releases/latest";

/// Metadata returned to the UI after querying GitHub Releases.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub has_update: bool,
    pub current_version: String,
    pub latest_version: String,
    pub release_notes: String,
    pub download_url: Option<String>,
    pub installer_name: Option<String>,
    pub html_url: String,
}

/// Queries GitHub Releases for the latest `iKoch` version and compares against `current_version`.
pub fn check_for_updates(current_version: &str) -> Result<UpdateInfo, String> {
    let resp_path =
        std::env::temp_dir().join(format!("ikoch_update_check_{}.json", std::process::id()));

    let mut cmd = Command::new("C:\\Windows\\System32\\curl.exe");
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd.arg("-sS")
        .arg("-L")
        .arg("--connect-timeout")
        .arg("10")
        .arg("--max-time")
        .arg("20")
        .arg("-H")
        .arg("User-Agent: iKoch-Updater")
        .arg("-H")
        .arg("Accept: application/vnd.github+json")
        .arg("-o")
        .arg(&resp_path)
        .arg("-w")
        .arg("%{http_code}")
        .arg(GITHUB_LATEST_RELEASE_URL);

    let output = cmd
        .output()
        .map_err(|e| format!("Failed to launch curl.exe for update check: {e}"))?;

    let body = fs::read_to_string(&resp_path).unwrap_or_default();
    let _ = fs::remove_file(&resp_path);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Update check failed: {stderr}"));
    }

    let status_code = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u16>()
        .unwrap_or(0);

    if status_code == 404 {
        return Ok(UpdateInfo {
            has_update: false,
            current_version: normalize_version_str(current_version),
            latest_version: normalize_version_str(current_version),
            release_notes: String::new(),
            download_url: None,
            installer_name: None,
            html_url: "https://github.com/its-lemon/iKoch/releases".to_string(),
        });
    }

    if !(200..300).contains(&status_code) {
        return Err(format!(
            "GitHub API returned HTTP {status_code}: {}",
            body.chars().take(240).collect::<String>()
        ));
    }

    let json: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| format!("Invalid JSON from GitHub Releases API: {e}"))?;

    let tag_name = json
        .get("tag_name")
        .and_then(|v| v.as_str())
        .unwrap_or(current_version);
    let latest_version = normalize_version_str(tag_name);
    let current_norm = normalize_version_str(current_version);
    let release_notes = json
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let html_url = json
        .get("html_url")
        .and_then(|v| v.as_str())
        .unwrap_or("https://github.com/its-lemon/iKoch/releases")
        .to_string();

    let (installer_name, download_url) = select_windows_installer_asset(&json);
    let has_update = is_newer_version(&latest_version, &current_norm);

    Ok(UpdateInfo {
        has_update,
        current_version: current_norm,
        latest_version,
        release_notes,
        download_url,
        installer_name,
        html_url,
    })
}

/// Downloads the Windows installer to `%TEMP%`, launches it, and cleanly exits `iKoch`.
pub fn download_and_install_update(
    app: &AppHandle,
    download_url: &str,
    installer_name: Option<&str>,
) -> Result<(), String> {
    if !is_allowed_github_download_url(download_url) {
        return Err("Update download URL must be a valid GitHub HTTPS release asset URL".to_string());
    }

    let filename = sanitize_installer_name(installer_name.unwrap_or("iKoch-setup.exe"));
    let target_path: PathBuf = std::env::temp_dir().join(&filename);
    let part_path: PathBuf = std::env::temp_dir().join(format!("{filename}.part"));
    let _ = fs::remove_file(&part_path);

    let mut cmd = Command::new("C:\\Windows\\System32\\curl.exe");
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd.arg("-fsSL")
        .arg("--connect-timeout")
        .arg("15")
        .arg("--max-time")
        .arg("300")
        .arg("-H")
        .arg("User-Agent: iKoch-Updater")
        .arg("-o")
        .arg(&part_path)
        .arg(download_url);

    let output = cmd
        .output()
        .map_err(|e| format!("Failed to launch curl.exe for update download: {e}"))?;

    if !output.status.success() {
        let _ = fs::remove_file(&part_path);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Failed to download update installer: {stderr}"));
    }

    let meta = fs::metadata(&part_path)
        .map_err(|e| format!("Downloaded installer missing: {e}"))?;
    if meta.len() < 100 * 1024 {
        let _ = fs::remove_file(&part_path);
        return Err("Downloaded installer file is unexpectedly small or corrupted".to_string());
    }

    if target_path.exists() {
        let _ = fs::remove_file(&target_path);
    }
    fs::rename(&part_path, &target_path)
        .map_err(|e| format!("Failed to finalize installer file: {e}"))?;

    Command::new(&target_path)
        .spawn()
        .map_err(|e| format!("Failed to launch installer '{}': {e}", target_path.display()))?;

    let state = app.state::<AppState>();
    state.mark_quitting();
    state.shutdown_daemon();
    app.exit(0);

    Ok(())
}

fn normalize_version_str(raw: &str) -> String {
    raw.trim().trim_start_matches(['v', 'V']).to_string()
}

fn parse_semver_tuple(raw: &str) -> Option<(u64, u64, u64)> {
    let core = normalize_version_str(raw);
    let base = core.split(['-', '+']).next().unwrap_or(&core);
    let mut parts = base.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next().unwrap_or("0").parse::<u64>().ok()?;
    let patch = parts.next().unwrap_or("0").parse::<u64>().ok()?;
    Some((major, minor, patch))
}

#[must_use]
pub fn is_newer_version(latest: &str, current: &str) -> bool {
    match (parse_semver_tuple(latest), parse_semver_tuple(current)) {
        (Some(l), Some(c)) => l > c,
        _ => normalize_version_str(latest) != normalize_version_str(current),
    }
}

fn select_windows_installer_asset(json: &serde_json::Value) -> (Option<String>, Option<String>) {
    let Some(assets) = json.get("assets").and_then(|a| a.as_array()) else {
        return (None, None);
    };

    let mut fallback_msi: Option<(String, String)> = None;

    for asset in assets {
        let name = asset
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let url = asset
            .get("browser_download_url")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if name.is_empty() || url.is_empty() {
            continue;
        }
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".exe") {
            return (Some(name.to_string()), Some(url.to_string()));
        }
        if lower.ends_with(".msi") && fallback_msi.is_none() {
            fallback_msi = Some((name.to_string(), url.to_string()));
        }
    }

    match fallback_msi {
        Some((name, url)) => (Some(name), Some(url)),
        None => (None, None),
    }
}

fn is_allowed_github_download_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("https://github.com/its-lemon/ikoch/releases/download/")
        || lower.starts_with("https://objects.githubusercontent.com/")
        || lower.starts_with("https://release-assets.githubusercontent.com/")
}

fn sanitize_installer_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if clean.is_empty() || clean == "." || clean == ".." {
        "iKoch-setup.exe".to_string()
    } else {
        clean
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_semver_versions_correctly() {
        assert!(is_newer_version("v0.2.2", "0.2.1"));
        assert!(is_newer_version("v0.2.3", "0.2.2"));
        assert!(is_newer_version("v0.1.4", "0.1.3"));
        assert!(is_newer_version("0.2.0", "v0.1.9"));
        assert!(is_newer_version("1.0.0", "0.9.9"));
        assert!(!is_newer_version("v0.1.3", "0.1.3"));
        assert!(!is_newer_version("0.1.2", "0.1.3"));
    }

    #[test]
    fn picks_exe_over_msi_from_release_assets() {
        let json = serde_json::json!({
            "assets": [
                {
                    "name": "iKoch_0.1.4_x64_en-US.msi",
                    "browser_download_url": "https://github.com/its-lemon/iKoch/releases/download/v0.1.4/iKoch_0.1.4_x64_en-US.msi"
                },
                {
                    "name": "iKoch_0.1.4_x64-setup.exe",
                    "browser_download_url": "https://github.com/its-lemon/iKoch/releases/download/v0.1.4/iKoch_0.1.4_x64-setup.exe"
                }
            ]
        });
        let (name, url) = select_windows_installer_asset(&json);
        assert_eq!(name.as_deref(), Some("iKoch_0.1.4_x64-setup.exe"));
        assert!(url.unwrap_or_default().ends_with("setup.exe"));
    }

    #[test]
    fn validates_download_urls_strictly() {
        assert!(is_allowed_github_download_url(
            "https://github.com/its-lemon/iKoch/releases/download/v0.1.4/iKoch_0.1.4_x64-setup.exe"
        ));
        assert!(!is_allowed_github_download_url("http://github.com/evil.exe"));
        assert!(!is_allowed_github_download_url("https://evil.example.com/setup.exe"));
    }
}
