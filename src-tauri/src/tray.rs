//! Dynamic system tray icon, status badge renderer (`Idle`/`Buffering`/`Recording`/`PrivacyMode`),
//! and interactive Win32 context menu (gallery, settings, live capture targets, hot actions, exit).

use crate::state::{open_or_focus_main_window, AppState};
use core_capture::{enumerate_monitors, enumerate_windows};
use core_types::{AppLanguage, CaptureTarget, DaemonStateSnapshot, RecorderCommand, TrayIndicatorState};
use std::collections::HashMap;
use std::sync::Arc;
use parking_lot::Mutex;
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};
use tracing::warn;

const TRAY_ID: &str = "ikoch-tray";

/// Visual state represented in the system tray icon & tooltip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayStatusKind {
    Idle,
    Buffering,
    Recording,
    PrivacyMode,
}

impl From<&DaemonStateSnapshot> for TrayStatusKind {
    fn from(snap: &DaemonStateSnapshot) -> Self {
        if snap.privacy_mode {
            Self::PrivacyMode
        } else {
            match snap.tray_state {
                TrayIndicatorState::Recording => Self::Recording,
                TrayIndicatorState::Buffering => Self::Buffering,
                TrayIndicatorState::Idle | TrayIndicatorState::Error => Self::Idle,
            }
        }
    }
}

/// Manages tray icon updates and dynamic capture target bindings inside the context menu.
#[derive(Clone)]
pub struct TrayController {
    tray: TrayIcon<Wry>,
    dynamic_targets: Arc<Mutex<HashMap<String, CaptureTarget>>>,
}

impl TrayController {
    /// Initializes the system tray icon and dynamic context menu on `app`.
    pub fn setup(app: &AppHandle) -> Result<Self, String> {
        let lang = app
            .try_state::<AppState>()
            .map_or(AppLanguage::Ru, |s| s.current_settings().language);
        let initial_targets = Arc::new(Mutex::new(HashMap::new()));
        let menu = build_tray_menu(app, &DaemonStateSnapshot::default(), &initial_targets, lang)?;
        let icon = build_status_icon(TrayStatusKind::Idle);

        let targets_for_menu = Arc::clone(&initial_targets);
        let initial_tooltip = if lang == AppLanguage::En {
            "iKoch — Idle"
        } else {
            "iKoch — Ожидание"
        };
        let tray = TrayIconBuilder::with_id(TRAY_ID)
            .icon(icon)
            .tooltip(initial_tooltip)
            .menu(&menu)
            .show_menu_on_left_click(false)
            .on_menu_event(move |app_handle, event| {
                handle_tray_menu_event(app_handle, event.id().as_ref(), &targets_for_menu);
            })
            .on_tray_icon_event(|tray_icon, event| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    open_or_focus_main_window(tray_icon.app_handle(), None);
                } else if let TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                } = event
                {
                    open_or_focus_main_window(tray_icon.app_handle(), None);
                }
            })
            .build(app)
            .map_err(|e| format!("TrayIconBuilder failed: {e}"))?;

        Ok(Self {
            tray,
            dynamic_targets: initial_targets,
        })
    }

    /// Updates tray icon color, localized tooltip, and live checkmarks/capture-source items.
    pub fn refresh(&self, app: &AppHandle, snap: &DaemonStateSnapshot) {
        let lang = app
            .try_state::<AppState>()
            .map_or(AppLanguage::Ru, |s| s.current_settings().language);
        self.refresh_with_language(app, snap, lang);
    }

    /// Refreshes tray icon, tooltip, and localized menu explicitly using `lang`.
    pub fn refresh_with_language(
        &self,
        app: &AppHandle,
        snap: &DaemonStateSnapshot,
        lang: AppLanguage,
    ) {
        let status = TrayStatusKind::from(snap);
        let _ = self.tray.set_icon(Some(build_status_icon(status)));
        let tooltip = format_tooltip(snap, status, lang);
        let _ = self.tray.set_tooltip(Some(tooltip));

        if let Ok(menu) = build_tray_menu(app, snap, &self.dynamic_targets, lang) {
            let _ = self.tray.set_menu(Some(menu));
        }
    }
}

fn format_tooltip(snap: &DaemonStateSnapshot, status: TrayStatusKind, lang: AppLanguage) -> String {
    let en = lang == AppLanguage::En;
    match status {
        TrayStatusKind::PrivacyMode => if en {
            "iKoch — Privacy Mode (buffer cleared)"
        } else {
            "iKoch — Privacy Mode (буфер очищен)"
        }
        .to_string(),
        TrayStatusKind::Recording => {
            if let Some(sess) = &snap.active_session {
                if en {
                    format!("iKoch — Recording ({:.0}s)", sess.elapsed_sec)
                } else {
                    format!("iKoch — Идёт запись ({:.0} сек)", sess.elapsed_sec)
                }
            } else if en {
                "iKoch — Recording".to_string()
            } else {
                "iKoch — Идёт запись".to_string()
            }
        }
        TrayStatusKind::Buffering => {
            if let Some(game) = &snap.active_game {
                if en {
                    format!(
                        "iKoch — Buffering ({}, {:.0}s)",
                        game.display_name, snap.buffer_fill_sec
                    )
                } else {
                    format!(
                        "iKoch — Буферизация ({}, {:.0} сек)",
                        game.display_name, snap.buffer_fill_sec
                    )
                }
            } else if en {
                format!("iKoch — Buffering ({:.0}s)", snap.buffer_fill_sec)
            } else {
                format!("iKoch — Буферизация ({:.0} сек)", snap.buffer_fill_sec)
            }
        }
        TrayStatusKind::Idle => if en {
            "iKoch — Idle"
        } else {
            "iKoch — Ожидание"
        }
        .to_string(),
    }
}

fn build_tray_menu(
    app: &AppHandle,
    snap: &DaemonStateSnapshot,
    targets_map: &Arc<Mutex<HashMap<String, CaptureTarget>>>,
    lang: AppLanguage,
) -> Result<Menu<Wry>, String> {
    let en = lang == AppLanguage::En;
    let menu = Menu::new(app).map_err(|e| e.to_string())?;
    let mut map = HashMap::new();

    let open_gallery = MenuItem::with_id(
        app,
        "open_gallery",
        if en { "Open Library" } else { "Открыть галерею" },
        true,
        None::<&str>,
    )
    .map_err(|e| e.to_string())?;
    let open_settings = MenuItem::with_id(
        app,
        "open_settings",
        if en { "Settings" } else { "Настройки" },
        true,
        None::<&str>,
    )
    .map_err(|e| e.to_string())?;

    // Target selection submenu (`Авто (Игра)`, active Monitors, active top-level Windows).
    let target_sub = Submenu::new(
        app,
        if en { "Capture Source" } else { "Источник захвата" },
        true,
    )
    .map_err(|e| e.to_string())?;
    let is_auto = matches!(snap.capture_target, CaptureTarget::AutoGame);
    let auto_item = CheckMenuItem::with_id(
        app,
        "target_autogame",
        if en { "Auto (Active Game)" } else { "Авто (Игра)" },
        true,
        is_auto,
        None::<&str>,
    )
    .map_err(|e| e.to_string())?;
    map.insert("target_autogame".to_string(), CaptureTarget::AutoGame);
    let _ = target_sub.append(&auto_item);

    if let Ok(monitors) = enumerate_monitors() {
        let _ = target_sub.append(&PredefinedMenuItem::separator(app).map_err(|e| e.to_string())?);
        for (idx, m) in monitors.into_iter().enumerate() {
            let id = format!("target_mon_{idx}");
            let checked = matches!(
                &snap.capture_target,
                CaptureTarget::Monitor { monitor_id, .. } if monitor_id == &m.monitor_id
            );
            let target = CaptureTarget::Monitor {
                monitor_id: m.monitor_id,
                name: m.name.clone(),
            };
            let item = CheckMenuItem::with_id(app, &id, &m.name, true, checked, None::<&str>)
                .map_err(|e| e.to_string())?;
            map.insert(id, target);
            let _ = target_sub.append(&item);
        }
    }

    if let Ok(windows) = enumerate_windows() {
        if !windows.is_empty() {
            let _ =
                target_sub.append(&PredefinedMenuItem::separator(app).map_err(|e| e.to_string())?);
            for w in windows.into_iter().take(8) {
                let id = format!("target_win_{}", w.hwnd);
                let label = if w.title.is_empty() {
                    w.process_name.clone()
                } else {
                    format!("[{}] {}", w.process_name, w.title.chars().take(36).collect::<String>())
                };
                let checked = matches!(
                    &snap.capture_target,
                    CaptureTarget::SpecificWindow { hwnd, .. } if *hwnd == w.hwnd
                );
                let target = CaptureTarget::SpecificWindow {
                    hwnd: w.hwnd,
                    process_name: w.process_name,
                    title: w.title,
                };
                let item = CheckMenuItem::with_id(app, &id, &label, true, checked, None::<&str>)
                    .map_err(|e| e.to_string())?;
                map.insert(id, target);
                let _ = target_sub.append(&item);
            }
        }
    }
    *targets_map.lock() = map;

    let save_clip = MenuItem::with_id(
        app,
        "save_clip",
        if en { "Save Clip (F9)" } else { "Сохранить клип (F9)" },
        true,
        None::<&str>,
    )
    .map_err(|e| e.to_string())?;
    let rec_label = if snap.active_session.is_some() {
        if en { "Stop Recording (F10)" } else { "Остановить запись (F10)" }
    } else if en {
        "Start Recording (F10)"
    } else {
        "Начать запись (F10)"
    };
    let toggle_session =
        MenuItem::with_id(app, "toggle_session", rec_label, true, None::<&str>)
            .map_err(|e| e.to_string())?;
    let mute_mic = CheckMenuItem::with_id(
        app,
        "toggle_mic_mute",
        if en { "Mute Microphone" } else { "Выключить микрофон" },
        true,
        snap.mic_muted,
        None::<&str>,
    )
    .map_err(|e| e.to_string())?;
    let privacy = CheckMenuItem::with_id(
        app,
        "toggle_privacy",
        "Privacy Mode",
        true,
        snap.privacy_mode,
        None::<&str>,
    )
    .map_err(|e| e.to_string())?;
    let exit_item = MenuItem::with_id(
        app,
        "exit_app",
        if en { "Quit" } else { "Выход" },
        true,
        None::<&str>,
    )
    .map_err(|e| e.to_string())?;

    let sep1 = PredefinedMenuItem::separator(app).map_err(|e| e.to_string())?;
    let sep2 = PredefinedMenuItem::separator(app).map_err(|e| e.to_string())?;
    let sep3 = PredefinedMenuItem::separator(app).map_err(|e| e.to_string())?;

    menu.append_items(&[
        &open_gallery,
        &open_settings,
        &sep1,
        &target_sub,
        &sep2,
        &save_clip,
        &toggle_session,
        &mute_mic,
        &privacy,
        &sep3,
        &exit_item,
    ])
    .map_err(|e| e.to_string())?;

    Ok(menu)
}

fn handle_tray_menu_event(
    app: &AppHandle,
    menu_id: &str,
    targets: &Arc<Mutex<HashMap<String, CaptureTarget>>>,
) {
    let state = app.state::<AppState>();
    match menu_id {
        "open_gallery" => open_or_focus_main_window(app, Some("gallery")),
        "open_settings" => open_or_focus_main_window(app, Some("settings")),
        "save_clip" => {
            let _ = state.send_command(RecorderCommand::SaveReplayClip);
        }
        "toggle_session" => {
            let _ = state.send_command(RecorderCommand::ToggleSession);
        }
        "toggle_mic_mute" => {
            let _ = state.send_command(RecorderCommand::ToggleMicMuted);
        }
        "toggle_privacy" => {
            let _ = state.send_command(RecorderCommand::TogglePrivacyMode);
        }
        "exit_app" => {
            state.mark_quitting();
            state.shutdown_daemon();
            app.exit(0);
        }
        other => {
            if let Some(target) = targets.lock().get(other).cloned() {
                let mut cfg = state.current_settings();
                cfg.capture.target = target.clone();
                let _ = state.persist_settings(&cfg);
                if let Err(err) = state.send_command(RecorderCommand::SetCaptureTarget(target)) {
                    warn!("Failed to switch target from tray: {err}");
                }
            }
        }
    }
}

fn dist_seg_2d(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let (vx, vy) = (bx - ax, by - ay);
    let (wx, wy) = (px - ax, py - ay);
    let c1 = vx * wx + vy * wy;
    if c1 <= 0.0 {
        return (px - ax).hypot(py - ay);
    }
    let c2 = vx * vx + vy * vy;
    if c2 <= c1 {
        return (px - bx).hypot(py - by);
    }
    let b = c1 / c2;
    (px - (ax + b * vx)).hypot(py - (ay + b * vy))
}

/// Renders a minimalist 32x32 RGBA rounded badge icon with a crisp `'K'` and status border.
#[must_use]
pub fn build_status_icon(status: TrayStatusKind) -> Image<'static> {
    const SIZE: usize = 32;
    let mut rgba = vec![0u8; SIZE * SIZE * 4];

    let accent_rgb: [u8; 3] = match status {
        TrayStatusKind::Idle => [148, 163, 184],       // Cool steel
        TrayStatusKind::Buffering => [34, 197, 94],    // Neon emerald
        TrayStatusKind::Recording => [244, 63, 94],    // Neon crimson
        TrayStatusKind::PrivacyMode => [245, 158, 11], // Amber shield
    };
    let bg_rgb: [u8; 3] = [15, 17, 26];

    for y in 0..SIZE {
        for x in 0..SIZE {
            let u = (x as f32 + 0.5) / SIZE as f32;
            let v = (y as f32 + 0.5) / SIZE as f32;
            let qx = (u - 0.5).abs() - (0.45 - 0.14);
            let qy = (v - 0.5).abs() - (0.45 - 0.14);
            let d_box = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - 0.14;
            if d_box > 0.025 {
                continue;
            }

            let box_alpha = (1.0 - ((d_box + 0.015) / 0.04).clamp(0.0, 1.0)).clamp(0.0, 1.0);
            let border = (1.0 - ((d_box + 0.025).abs() / 0.045).clamp(0.0, 1.0)).clamp(0.0, 1.0);

            let stroke = 0.078;
            let d_stem = dist_seg_2d(u, v, 0.30, 0.24, 0.30, 0.76) - stroke;
            let d_up = dist_seg_2d(u, v, 0.35, 0.52, 0.68, 0.24) - stroke * 0.92;
            let d_dn = dist_seg_2d(u, v, 0.43, 0.46, 0.69, 0.76) - stroke * 0.92;
            let d_k = d_stem.min(d_up).min(d_dn);
            let k_mask = (1.0 - ((d_k + 0.015) / 0.03).clamp(0.0, 1.0)).clamp(0.0, 1.0);

            let mut r = f32::from(bg_rgb[0]) * (1.0 - border) + f32::from(accent_rgb[0]) * border;
            let mut g = f32::from(bg_rgb[1]) * (1.0 - border) + f32::from(accent_rgb[1]) * border;
            let mut b = f32::from(bg_rgb[2]) * (1.0 - border) + f32::from(accent_rgb[2]) * border;

            r = r * (1.0 - k_mask) + 248.0 * k_mask;
            g = g * (1.0 - k_mask) + 250.0 * k_mask;
            b = b * (1.0 - k_mask) + 255.0 * k_mask;

            let idx = (y * SIZE + x) * 4;
            rgba[idx] = r.clamp(0.0, 255.0) as u8;
            rgba[idx + 1] = g.clamp(0.0, 255.0) as u8;
            rgba[idx + 2] = b.clamp(0.0, 255.0) as u8;
            rgba[idx + 3] = (box_alpha * 255.0).clamp(0.0, 255.0) as u8;
        }
    }

    Image::new_owned(rgba, SIZE as u32, SIZE as u32)
}
