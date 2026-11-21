//! Foreground game auto-detection for `CaptureTarget::AutoGame`.
//!
//! Uses non-invasive Win32 window inspection (`GetForegroundWindow`, `GetWindowRect`,
//! `GetMonitorInfoW`), built-in + custom whitelist/blacklist matching, and `sysinfo`
//! process liveness checking so Alt+Tabbing doesn't falsely abort a live game session.

use core_capture::{query_process_exe_name, query_process_exe_path};
use core_types::{DetectedGameInfo, GameDetectionConfig};
#[cfg(windows)]
use std::ffi::OsString;
#[cfg(windows)]
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::Path;
use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
#[cfg(windows)]
use windows::core::PCWSTR;
#[cfg(windows)]
use windows::Win32::Foundation::{HWND, RECT};
#[cfg(windows)]
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
#[cfg(windows)]
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
#[cfg(windows)]
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetForegroundWindow, GetShellWindow, GetWindowRect, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible,
};

/// Built-in popular PC games recognized immediately even in windowed mode.
const BUILTIN_GAME_EXES: &[&str] = &[
    "cs2.exe",
    "csgo.exe",
    "valorant-win64-shipping.exe",
    "dota2.exe",
    "eldenring.exe",
    "cyberpunk2077.exe",
    "fortniteclient-win64-shipping.exe",
    "r5apex.exe",
    "overwatch.exe",
    "gta5.exe",
    "rdr2.exe",
    "minecraft.exe",
    "javaw.exe",
    "bg3.exe",
    "bg3_dx11.exe",
    "helldivers2.exe",
    "rustclient.exe",
    "tslgame.exe",
    "league of legends.exe",
    "wow.exe",
    "escapefromtarkov.exe",
    "stalker2-win64-shipping.exe",
    "witcher3.exe",
    "hogwartslegacy.exe",
    "terraria.exe",
    "hades.exe",
    "hades2.exe",
    "destiny2.exe",
    "rocketleague.exe",
    "warframe.x64.exe",
    "pathofexile_x64steam.exe",
    "deadlock.exe",
];

#[cfg(windows)]
const SHELL_CLASSES: &[&str] = &["progman", "workerw", "shell_traywnd", "windows.ui.core.corewindow"];

/// Transition emitted by [`GameDetector::poll`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameDetectionTransition {
    NoChange,
    Detected(DetectedGameInfo),
    Lost(String),
}

/// Classifies processes against built-in, custom whitelist, and blacklist rules.
#[must_use]
pub fn classify_executable(
    exe_name: &str,
    is_fullscreen_or_borderless: bool,
    cfg: &GameDetectionConfig,
) -> Option<String> {
    let lower = exe_name.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return None;
    }

    if cfg
        .blacklist_exes
        .iter()
        .any(|b| b.trim().eq_ignore_ascii_case(&lower))
    {
        return None;
    }

    if cfg
        .custom_whitelist_exes
        .iter()
        .any(|w| w.trim().eq_ignore_ascii_case(&lower))
    {
        return Some(pretty_game_name(&lower));
    }

    if cfg.use_builtin_game_list && BUILTIN_GAME_EXES.contains(&lower.as_str()) {
        return Some(pretty_game_name(&lower));
    }

    if is_fullscreen_or_borderless {
        return Some(pretty_game_name(&lower));
    }

    None
}

fn pretty_game_name(exe_lower: &str) -> String {
    if let Some(known) = lookup_builtin_game_title(exe_lower) {
        return known.to_string();
    }
    clean_exe_stem_title(exe_lower)
}

fn lookup_builtin_game_title(exe_lower: &str) -> Option<&'static str> {
    match exe_lower {
        "cs2.exe" => Some("Counter-Strike 2"),
        "csgo.exe" => Some("Counter-Strike Global Offensive"),
        "valorant-win64-shipping.exe" => Some("VALORANT"),
        "dota2.exe" => Some("Dota 2"),
        "eldenring.exe" => Some("ELDEN RING"),
        "cyberpunk2077.exe" => Some("Cyberpunk 2077"),
        "fortniteclient-win64-shipping.exe" => Some("Fortnite"),
        "r5apex.exe" => Some("Apex Legends"),
        "overwatch.exe" => Some("Overwatch 2"),
        "gta5.exe" => Some("Grand Theft Auto V"),
        "rdr2.exe" => Some("Red Dead Redemption 2"),
        "minecraft.exe" | "javaw.exe" => Some("Minecraft"),
        "bg3.exe" | "bg3_dx11.exe" => Some("Baldur's Gate 3"),
        "helldivers2.exe" => Some("HELLDIVERS 2"),
        "rustclient.exe" => Some("Rust"),
        "tslgame.exe" => Some("PUBG BATTLEGROUNDS"),
        "league of legends.exe" => Some("League of Legends"),
        "wow.exe" => Some("World of Warcraft"),
        "escapefromtarkov.exe" => Some("Escape from Tarkov"),
        "stalker2-win64-shipping.exe" => Some("S.T.A.L.K.E.R. 2"),
        "witcher3.exe" => Some("The Witcher 3"),
        "hogwartslegacy.exe" => Some("Hogwarts Legacy"),
        "terraria.exe" => Some("Terraria"),
        "hades.exe" => Some("Hades"),
        "hades2.exe" => Some("Hades II"),
        "destiny2.exe" => Some("Destiny 2"),
        "rocketleague.exe" => Some("Rocket League"),
        "warframe.x64.exe" => Some("Warframe"),
        "pathofexile_x64steam.exe" => Some("Path of Exile"),
        "deadlock.exe" => Some("Deadlock"),
        _ => None,
    }
}

fn clean_exe_stem_title(exe_name: &str) -> String {
    let mut stem = exe_name
        .trim()
        .strip_suffix(".exe")
        .or_else(|| exe_name.trim().strip_suffix(".EXE"))
        .unwrap_or(exe_name.trim())
        .to_string();
    for noise in [
        "-Win64-Shipping",
        "-win64-shipping",
        "-Win32-Shipping",
        "-win32-shipping",
        "_x64",
        ".x64",
        "Client",
    ] {
        if let Some(stripped) = stem.strip_suffix(noise) {
            if !stripped.trim().is_empty() {
                stem = stripped.to_string();
            }
        }
    }
    let clean = stem.replace(['_', '-'], " ").trim().to_string();
    if clean.is_empty() {
        "Desktop Application".to_string()
    } else {
        clean
    }
}

#[cfg(windows)]
fn clean_version_info_string(raw: &str) -> Option<String> {
    let mut s = raw.trim().to_string();
    for suffix in ["-Win64-Shipping", "-Win32-Shipping", "Shipping", ".exe"] {
        if let Some(stripped) = s.strip_suffix(suffix) {
            s = stripped.trim().to_string();
        }
    }
    let lower = s.to_ascii_lowercase();
    if s.is_empty()
        || lower == "unreal engine"
        || lower == "bootstrappackagedgame"
        || lower == "bootstrap packaged game"
        || lower == "unityplayer"
        || lower == "game"
    {
        return None;
    }
    Some(s)
}

/// Reads `FileDescription` or `ProductName` from a PE executable's `VS_VERSION_INFO` resource.
#[cfg(windows)]
#[must_use]
pub fn read_exe_version_description(exe_path: &Path) -> Option<String> {
    let wide_path: Vec<u16> = exe_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut handle = 0u32;
    let size = unsafe { GetFileVersionInfoSizeW(PCWSTR(wide_path.as_ptr()), Some(&mut handle)) };
    if size == 0 {
        return None;
    }

    let mut data = vec![0u8; size as usize];
    unsafe {
        GetFileVersionInfoW(
            PCWSTR(wide_path.as_ptr()),
            Some(0),
            size,
            data.as_mut_ptr().cast(),
        )
    }
    .ok()?;

    let mut lang_cp_pairs: Vec<(u16, u16)> = Vec::new();
    let trans_sub: Vec<u16> = "\\VarFileInfo\\Translation\0".encode_utf16().collect();
    let mut trans_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
    let mut trans_len: u32 = 0;
    if unsafe {
        VerQueryValueW(
            data.as_ptr().cast(),
            PCWSTR(trans_sub.as_ptr()),
            &mut trans_ptr,
            &mut trans_len,
        )
    }
    .as_bool()
        && !trans_ptr.is_null()
        && trans_len >= 4
    {
        let count = (trans_len as usize) / 4;
        let pairs = unsafe { std::slice::from_raw_parts(trans_ptr.cast::<u16>(), count * 2) };
        for chunk in pairs.chunks_exact(2) {
            lang_cp_pairs.push((chunk[0], chunk[1]));
        }
    }
    lang_cp_pairs.push((0x0409, 0x04B0));
    lang_cp_pairs.push((0x0409, 0x04E4));

    for (lang, cp) in lang_cp_pairs {
        for field in ["ProductName", "FileDescription"] {
            let sub = format!("\\StringFileInfo\\{lang:04x}{cp:04x}\\{field}\0");
            let sub_w: Vec<u16> = sub.encode_utf16().collect();
            let mut val_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
            let mut val_len: u32 = 0;
            let ok = unsafe {
                VerQueryValueW(
                    data.as_ptr().cast(),
                    PCWSTR(sub_w.as_ptr()),
                    &mut val_ptr,
                    &mut val_len,
                )
            }
            .as_bool();
            if ok && !val_ptr.is_null() && val_len > 1 {
                let slice = unsafe {
                    std::slice::from_raw_parts(val_ptr.cast::<u16>(), val_len as usize)
                };
                let nul = slice.iter().position(|&c| c == 0).unwrap_or(slice.len());
                let text = OsString::from_wide(&slice[..nul])
                    .to_string_lossy()
                    .into_owned();
                if let Some(cleaned) = clean_version_info_string(&text) {
                    return Some(cleaned);
                }
            }
        }
    }
    None
}

/// Reads `FileDescription` or `ProductName` from a PE executable's `VS_VERSION_INFO` resource.
#[cfg(not(windows))]
#[must_use]
pub fn read_exe_version_description(_exe_path: &Path) -> Option<String> {
    None
}

/// Resolves a human-friendly game name using built-in dictionary, `.exe` VersionInfo,
/// window title, or cleaned executable name.
#[must_use]
pub fn resolve_human_game_name(
    exe_path: Option<&Path>,
    exe_name: &str,
    window_title: &str,
) -> String {
    let lower = exe_name.trim().to_ascii_lowercase();
    if let Some(known) = lookup_builtin_game_title(&lower) {
        return known.to_string();
    }
    if let Some(path) = exe_path {
        if let Some(desc) = read_exe_version_description(path) {
            return desc;
        }
    }
    let clean_title = window_title.trim();
    if !clean_title.is_empty() {
        return clean_title.to_string();
    }
    clean_exe_stem_title(exe_name)
}

/// Stateful foreground game watcher supporting grace-period pause on Alt+Tab.
pub struct GameDetector {
    config: GameDetectionConfig,
    sys: Option<System>,
    active_game: Option<DetectedGameInfo>,
    lost_since: Option<Instant>,
}

impl GameDetector {
    /// Creates a new `GameDetector` using `config`.
    #[must_use]
    pub fn new(config: GameDetectionConfig) -> Self {
        Self {
            config,
            sys: System::new().ok(),
            active_game: None,
            lost_since: None,
        }
    }

    /// Updates game detection configuration live.
    pub fn update_config(&mut self, config: GameDetectionConfig) {
        self.config = config;
    }

    /// Returns the currently active game info, if one is tracked.
    #[must_use]
    pub fn active_game(&self) -> Option<&DetectedGameInfo> {
        self.active_game.as_ref()
    }

    /// Inspects the foreground window & active process tree and returns any state transition.
    pub fn poll(&mut self) -> GameDetectionTransition {
        if let Some(candidate) = probe_foreground_game(&self.config) {
            self.lost_since = None;
            let switched = self
                .active_game
                .as_ref()
                .is_none_or(|cur| cur.process_id != candidate.process_id || cur.hwnd != candidate.hwnd);
            if switched {
                self.active_game = Some(candidate.clone());
                return GameDetectionTransition::Detected(candidate);
            }
            return GameDetectionTransition::NoChange;
        }

        // Foreground window is not a qualifying game; verify whether the previously tracked
        // game is still running (Alt+Tab grace period) or exited.
        let Some(current) = self.active_game.clone() else {
            self.lost_since = None;
            return GameDetectionTransition::NoChange;
        };

        let target_pid = Pid::from_u32(current.process_id);
        let sys_alive = self.sys.as_mut().is_none_or(|sys| {
            sys.refresh_processes_specifics(
                ProcessesToUpdate::Some(&[target_pid]),
                true,
                ProcessRefreshKind::nothing(),
            );
            sys.process(target_pid).is_some()
        });
        #[cfg(windows)]
        let process_alive = sys_alive
            && unsafe { IsWindow(Some(HWND(current.hwnd as *mut _))) }.as_bool();
        #[cfg(not(windows))]
        let process_alive = sys_alive;

        if !process_alive {
            self.active_game = None;
            self.lost_since = None;
            return GameDetectionTransition::Lost(current.executable_name);
        }

        let grace = Duration::from_secs(u64::from(self.config.auto_pause_delay_secs));
        let lost_at = self.lost_since.get_or_insert_with(Instant::now);
        if lost_at.elapsed() >= grace {
            self.active_game = None;
            self.lost_since = None;
            return GameDetectionTransition::Lost(current.executable_name);
        }

        GameDetectionTransition::NoChange
    }
}

#[cfg(windows)]
fn probe_foreground_game(cfg: &GameDetectionConfig) -> Option<DetectedGameInfo> {
    let fg = unsafe { GetForegroundWindow() }.0 as isize;
    probe_window_as_game(fg, cfg)
}

#[cfg(target_os = "linux")]
fn probe_foreground_game(cfg: &GameDetectionConfig) -> Option<DetectedGameInfo> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};

    if let Ok((conn, screen_num)) = x11rb::connect(None) {
        if let Some(screen) = conn.setup().roots.get(screen_num) {
            if let (Ok(a_act), Ok(a_pid), Ok(a_name), Ok(a_utf8)) = (
                conn.intern_atom(false, b"_NET_ACTIVE_WINDOW").map(|c| c.reply()),
                conn.intern_atom(false, b"_NET_WM_PID").map(|c| c.reply()),
                conn.intern_atom(false, b"_NET_WM_NAME").map(|c| c.reply()),
                conn.intern_atom(false, b"UTF8_STRING").map(|c| c.reply()),
            ) {
                if let (Ok(a_act), Ok(a_pid), Ok(a_name), Ok(a_utf8)) =
                    (a_act, a_pid, a_name, a_utf8)
                {
                    if let Some(win) = conn
                        .get_property(false, screen.root, a_act.atom, AtomEnum::WINDOW, 0, 1)
                        .ok()
                        .and_then(|c| c.reply().ok())
                        .and_then(|r| r.value32().and_then(|mut it| it.next()))
                    {
                        if win != 0 {
                            let pid = conn
                                .get_property(false, win, a_pid.atom, AtomEnum::CARDINAL, 0, 1)
                                .ok()
                                .and_then(|c| c.reply().ok())
                                .and_then(|r| r.value32().and_then(|mut it| it.next()))
                                .unwrap_or(0);
                            if pid > 0 && pid != std::process::id() {
                                let exe_path = query_process_exe_path(pid);
                                let exe_name = exe_path
                                    .as_deref()
                                    .and_then(|p| p.file_name())
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .or_else(|| query_process_exe_name(pid))?;
                                let title = conn
                                    .get_property(false, win, a_name.atom, a_utf8.atom, 0, 512)
                                    .ok()
                                    .and_then(|c| c.reply().ok())
                                    .and_then(|r| String::from_utf8(r.value).ok())
                                    .unwrap_or_default()
                                    .trim()
                                    .to_string();
                                let fullscreen = conn
                                    .get_geometry(win)
                                    .ok()
                                    .and_then(|c| c.reply().ok())
                                    .is_some_and(|g| {
                                        g.width >= screen.width_in_pixels.saturating_sub(2)
                                            && g.height >= screen.height_in_pixels.saturating_sub(2)
                                    });
                                if classify_executable(&exe_name, fullscreen, cfg).is_some() {
                                    let display_name = resolve_human_game_name(
                                        exe_path.as_deref(),
                                        &exe_name,
                                        &title,
                                    );
                                    return Some(DetectedGameInfo {
                                        process_id: pid,
                                        executable_name: exe_name,
                                        display_name,
                                        window_title: title,
                                        hwnd: isize::try_from(win).unwrap_or(0),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Wayland / headless fallback: scan `/proc` via `sysinfo` for known or whitelisted games.
    let mut sys = System::new_all();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    for (pid, proc_info) in sys.processes() {
        let exe_name = proc_info.name().to_string_lossy().into_owned();
        if classify_executable(&exe_name, false, cfg).is_some() {
            let pid_u32 = pid.as_u32();
            let exe_path = proc_info.exe().map(Path::to_path_buf);
            let display_name = resolve_human_game_name(exe_path.as_deref(), &exe_name, "");
            return Some(DetectedGameInfo {
                process_id: pid_u32,
                executable_name: exe_name,
                display_name,
                window_title: String::new(),
                hwnd: 0,
            });
        }
    }
    None
}

#[cfg(windows)]
fn probe_window_as_game(hwnd_raw: isize, cfg: &GameDetectionConfig) -> Option<DetectedGameInfo> {
    if hwnd_raw == 0 {
        return None;
    }
    let hwnd = HWND(hwnd_raw as *mut _);
    let shell = unsafe { GetShellWindow() };
    if hwnd == shell {
        return None;
    }
    if !unsafe { IsWindowVisible(hwnd) }.as_bool() || unsafe { IsIconic(hwnd) }.as_bool() {
        return None;
    }

    let mut cloaked: u32 = 0;
    let _ = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            std::ptr::addr_of_mut!(cloaked).cast(),
            size_of::<u32>() as u32,
        )
    };
    if cloaked != 0 {
        return None;
    }

    let class_name = read_window_class(hwnd).to_ascii_lowercase();
    if SHELL_CLASSES.contains(&class_name.as_str()) {
        return None;
    }

    let mut pid: u32 = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 || pid == std::process::id() {
        return None;
    }

    let exe_path = query_process_exe_path(pid);
    let exe_name = exe_path
        .as_deref()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .or_else(|| query_process_exe_name(pid))?;
    let fullscreen = is_fullscreen_or_borderless(hwnd);
    let _classified = classify_executable(&exe_name, fullscreen, cfg)?;
    let title = read_window_title_raw(hwnd);
    let display_name = resolve_human_game_name(exe_path.as_deref(), &exe_name, &title);

    Some(DetectedGameInfo {
        process_id: pid,
        executable_name: exe_name,
        display_name,
        window_title: title,
        hwnd: hwnd_raw,
    })
}

#[cfg(windows)]
fn is_fullscreen_or_borderless(hwnd: HWND) -> bool {
    let mut win_rc = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut win_rc) }.is_err() {
        return false;
    }
    let win_w = win_rc.right - win_rc.left;
    let win_h = win_rc.bottom - win_rc.top;
    if win_w <= 0 || win_h <= 0 {
        return false;
    }

    let hmon = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let mut mon_info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !unsafe { GetMonitorInfoW(hmon, &mut mon_info) }.as_bool() {
        return false;
    }
    let mon_w = mon_info.rcMonitor.right - mon_info.rcMonitor.left;
    let mon_h = mon_info.rcMonitor.bottom - mon_info.rcMonitor.top;
    win_w >= mon_w - 2 && win_h >= mon_h - 2
}

#[cfg(windows)]
fn read_window_title_raw(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let len = unsafe { GetWindowTextW(hwnd, &mut buf) };
    if len <= 0 {
        String::new()
    } else {
        OsString::from_wide(&buf[..len as usize])
            .to_string_lossy()
            .into_owned()
    }
}

#[cfg(windows)]
fn read_window_class(hwnd: HWND) -> String {
    let mut buf = [0u16; 128];
    let len = unsafe { GetClassNameW(hwnd, &mut buf) };
    if len <= 0 {
        String::new()
    } else {
        OsString::from_wide(&buf[..len as usize])
            .to_string_lossy()
            .into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_classification_respects_blacklist_whitelist_and_builtin_games() {
        let mut cfg = GameDetectionConfig::default();
        cfg.custom_whitelist_exes.push("my_indie_game.exe".to_string());

        // Blacklisted apps must never trigger AutoGame even in fullscreen.
        assert!(classify_executable("chrome.exe", true, &cfg).is_none());
        assert!(classify_executable("explorer.exe", true, &cfg).is_none());

        // Built-in games match even when windowed.
        assert!(classify_executable("cs2.exe", false, &cfg).is_some());
        assert!(classify_executable("VALORANT-Win64-Shipping.exe", false, &cfg).is_some());

        // Custom whitelist matches case-insensitively.
        assert!(classify_executable("MY_INDIE_GAME.EXE", false, &cfg).is_some());

        // Arbitrary windowed app does not match unless fullscreen/borderless.
        assert!(classify_executable("notepad.exe", false, &cfg).is_none());
        assert!(classify_executable("unlisted_dx12.exe", true, &cfg).is_some());
    }
}
