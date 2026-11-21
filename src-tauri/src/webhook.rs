//! Optional Discord Webhook file uploader using Windows `curl.exe` (`CREATE_NO_WINDOW`).

use std::fs;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;
use uuid::Uuid;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Uploads `file_path` (e.g. a Discord-compressed `.mp4` or `.gif`) via HTTPS multipart POST to
/// a validated Discord Webhook URL.
pub fn send_file_to_discord_webhook(
    webhook_url: &str,
    file_path: &Path,
    content: Option<&str>,
) -> Result<(), String> {
    let trimmed = webhook_url.trim();
    if !is_valid_discord_webhook_url(trimmed) {
        return Err("Invalid Discord Webhook URL (expected https://discord.com/api/webhooks/...)"
            .to_string());
    }
    if !file_path.exists() {
        return Err(format!("Upload file '{}' does not exist", file_path.display()));
    }

    let payload_json = serde_json::json!({
        "content": content.unwrap_or("Clipped with iKoch")
    })
    .to_string();

    let resp_path = std::env::temp_dir().join(format!("ikoch_webhook_{}.txt", Uuid::new_v4()));
    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new("C:\\Windows\\System32\\curl.exe");
        c.creation_flags(CREATE_NO_WINDOW);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = Command::new("curl");
    cmd.arg("-sS")
        .arg("-o")
        .arg(&resp_path)
        .arg("-w")
        .arg("%{http_code}")
        .arg("-F")
        .arg(format!("payload_json={payload_json}"))
        .arg("-F")
        .arg(format!("files[0]=@{}", file_path.display()))
        .arg(trimmed);

    let output = cmd
        .output()
        .map_err(|e| format!("Failed to launch curl.exe for Discord webhook: {e}"))?;

    let body = fs::read_to_string(&resp_path).unwrap_or_default();
    let _ = fs::remove_file(&resp_path);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Discord webhook curl error: {stderr}"));
    }

    let status_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let status_code = status_str.parse::<u16>().unwrap_or(0);
    if !(200..300).contains(&status_code) {
        return Err(format!(
            "Discord webhook returned HTTP {status_code}: {body}"
        ));
    }

    Ok(())
}

/// Validates that `url` targets an official Discord webhook HTTPS endpoint.
#[must_use]
pub fn is_valid_discord_webhook_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("https://discord.com/api/webhooks/")
        || lower.starts_with("https://discordapp.com/api/webhooks/")
        || lower.starts_with("https://ptb.discord.com/api/webhooks/")
        || lower.starts_with("https://canary.discord.com/api/webhooks/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_discord_webhook_urls_strictly() {
        assert!(is_valid_discord_webhook_url(
            "https://discord.com/api/webhooks/123456/abcdef"
        ));
        assert!(is_valid_discord_webhook_url(
            "https://discordapp.com/api/webhooks/123/xyz"
        ));
        assert!(!is_valid_discord_webhook_url("http://discord.com/api/webhooks/123/xyz"));
        assert!(!is_valid_discord_webhook_url("https://example.com/webhook"));
    }
}
