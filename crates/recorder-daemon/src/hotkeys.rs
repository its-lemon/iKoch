//! Global Win32 hotkey registration and event pumping via `global-hotkey`.
//!
//! Runs a dedicated message-pump thread owning [`GlobalHotKeyManager`] because Windows
//! delivers `WM_HOTKEY` only to the thread that registered the hotkeys.

use core_types::HotkeySettings;
use crossbeam_channel::{bounded, Receiver, Sender};
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tracing::{debug, info, warn};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
};

/// Logical action triggered when a user presses a global shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HotkeyAction {
    SaveClip,
    ToggleSession,
    AddBookmark,
    ToggleMicMute,
    TogglePrivacyMode,
}

enum HotkeyControl {
    Rebind(HotkeySettings),
    Stop,
}

/// Parses user-friendly hotkey text (e.g., `"F9"`, `"Ctrl+Shift+M"`, `"Ctrl+Shift+P"`) into `HotKey`.
pub fn parse_hotkey_binding(raw: &str) -> Result<HotKey, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Empty hotkey string".to_string());
    }
    let normalized = trimmed
        .split('+')
        .map(|tok| match tok.trim().to_ascii_lowercase().as_str() {
            "ctrl" => "control".to_string(),
            "win" | "super" => "meta".to_string(),
            other => other.to_string(),
        })
        .collect::<Vec<_>>()
        .join("+");

    HotKey::from_str(&normalized).map_err(|e| format!("Invalid hotkey '{raw}': {e}"))
}

/// Dedicated background hotkey listener delivering [`HotkeyAction`] events.
pub struct GlobalHotkeyService {
    cmd_tx: Sender<HotkeyControl>,
    stop_flag: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl GlobalHotkeyService {
    /// Starts the global hotkey Win32 message-pump thread and registers `initial` bindings.
    pub fn start(
        initial: HotkeySettings,
        action_tx: Sender<HotkeyAction>,
    ) -> Result<Self, String> {
        let (cmd_tx, cmd_rx) = bounded::<HotkeyControl>(16);
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop_flag);

        let thread = thread::Builder::new()
            .name("rustclip-hotkeys".to_string())
            .spawn(move || run_hotkey_loop(initial, cmd_rx, action_tx, stop_clone))
            .map_err(|e| format!("Spawn hotkey thread failed: {e}"))?;

        Ok(Self {
            cmd_tx,
            stop_flag,
            thread: Some(thread),
        })
    }

    /// Re-registers global hotkeys live after settings change.
    pub fn rebind(&self, settings: HotkeySettings) {
        let _ = self.cmd_tx.send(HotkeyControl::Rebind(settings));
    }

    /// Stops the hotkey message-pump thread and unregisters all shortcuts.
    pub fn stop(mut self) {
        self.shutdown_inner();
    }

    fn shutdown_inner(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        let _ = self.cmd_tx.send(HotkeyControl::Stop);
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
    }
}

impl Drop for GlobalHotkeyService {
    fn drop(&mut self) {
        self.shutdown_inner();
    }
}

fn run_hotkey_loop(
    initial: HotkeySettings,
    cmd_rx: Receiver<HotkeyControl>,
    action_tx: Sender<HotkeyAction>,
    stop_flag: Arc<AtomicBool>,
) {
    #[cfg(target_os = "linux")]
    let _portal_guard = spawn_linux_portal_shortcuts(&initial, action_tx.clone(), Arc::clone(&stop_flag));

    let manager = match GlobalHotKeyManager::new() {
        Ok(m) => Some(m),
        Err(err) => {
            warn!("GlobalHotKeyManager unavailable: {err}");
            None
        }
    };

    let mut registered: HashMap<u32, (HotKey, HotkeyAction)> = HashMap::new();
    if let Some(mgr) = manager.as_ref() {
        apply_bindings(mgr, &initial, &mut registered);
    }

    let event_rx = GlobalHotKeyEvent::receiver();

    while !stop_flag.load(Ordering::Relaxed) {
        // Pump Win32 message queue on this thread so RegisterHotKey WM_HOTKEY fires.
        #[cfg(windows)]
        {
            let mut msg = MSG::default();
            while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
                let _ = unsafe { TranslateMessage(&msg) };
                unsafe { DispatchMessageW(&msg) };
            }
        }

        while let Ok(evt) = event_rx.try_recv() {
            if evt.state == HotKeyState::Pressed {
                if let Some((_, action)) = registered.get(&evt.id) {
                    debug!("Global hotkey triggered: {:?}", action);
                    let _ = action_tx.try_send(*action);
                }
            }
        }

        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                HotkeyControl::Rebind(new_cfg) => {
                    if let Some(mgr) = manager.as_ref() {
                        apply_bindings(mgr, &new_cfg, &mut registered);
                    }
                }
                HotkeyControl::Stop => {
                    if let Some(mgr) = manager.as_ref() {
                        unregister_all_map(mgr, &mut registered);
                    }
                    return;
                }
            }
        }

        thread::sleep(Duration::from_millis(12));
    }

    if let Some(mgr) = manager.as_ref() {
        unregister_all_map(mgr, &mut registered);
    }
}

#[cfg(target_os = "linux")]
fn spawn_linux_portal_shortcuts(
    initial: &HotkeySettings,
    action_tx: Sender<HotkeyAction>,
    stop_flag: Arc<AtomicBool>,
) -> Option<JoinHandle<()>> {
    use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
    let cfg = initial.clone();
    thread::Builder::new()
        .name("ikoch-wayland-hotkeys".to_string())
        .spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            rt.block_on(async move {
                let Ok(proxy) = GlobalShortcuts::new().await else {
                    debug!("Wayland GlobalShortcuts portal unavailable; using X11 fallback");
                    return;
                };
                let Ok(session) = proxy.create_session().await else {
                    return;
                };
                let shortcuts = [
                    NewShortcut::new("save_clip", "Save Instant Replay Clip")
                        .preferred_trigger(Some(cfg.save_clip.as_str())),
                    NewShortcut::new("toggle_session", "Start / Stop Session Recording")
                        .preferred_trigger(Some(cfg.toggle_session.as_str())),
                    NewShortcut::new("add_bookmark", "Add Bookmark Marker")
                        .preferred_trigger(Some(cfg.add_bookmark.as_str())),
                    NewShortcut::new("toggle_mic_mute", "Mute / Unmute Microphone")
                        .preferred_trigger(Some(cfg.toggle_mic_mute.as_str())),
                    NewShortcut::new("toggle_privacy", "Toggle Privacy Mode")
                        .preferred_trigger(Some(cfg.toggle_privacy_mode.as_str())),
                ];
                if proxy.bind_shortcuts(&session, &shortcuts, None).await.is_err() {
                    return;
                }
                info!("Registered Wayland GlobalShortcuts portal bindings");
                let Ok(mut stream) = proxy.receive_activated().await else {
                    return;
                };
                use futures_util::StreamExt;
                while !stop_flag.load(Ordering::Relaxed) {
                    tokio::select! {
                        maybe_evt = stream.next() => {
                            let Some(evt) = maybe_evt else { break };
                            let action = match evt.shortcut_id() {
                                "save_clip" => Some(HotkeyAction::SaveClip),
                                "toggle_session" => Some(HotkeyAction::ToggleSession),
                                "add_bookmark" => Some(HotkeyAction::AddBookmark),
                                "toggle_mic_mute" => Some(HotkeyAction::ToggleMicMute),
                                "toggle_privacy" => Some(HotkeyAction::TogglePrivacyMode),
                                _ => None,
                            };
                            if let Some(act) = action {
                                let _ = action_tx.try_send(act);
                            }
                        }
                        _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                    }
                }
            });
        })
        .ok()
}

fn unregister_all_map(
    manager: &GlobalHotKeyManager,
    registered: &mut HashMap<u32, (HotKey, HotkeyAction)>,
) {
    let hotkeys: Vec<HotKey> = registered.values().map(|(hk, _)| *hk).collect();
    if !hotkeys.is_empty() {
        let _ = manager.unregister_all(&hotkeys);
    }
    registered.clear();
}

fn apply_bindings(
    manager: &GlobalHotKeyManager,
    cfg: &HotkeySettings,
    registered: &mut HashMap<u32, (HotKey, HotkeyAction)>,
) {
    unregister_all_map(manager, registered);

    let specs = [
        (&cfg.save_clip, HotkeyAction::SaveClip),
        (&cfg.toggle_session, HotkeyAction::ToggleSession),
        (&cfg.add_bookmark, HotkeyAction::AddBookmark),
        (&cfg.toggle_mic_mute, HotkeyAction::ToggleMicMute),
        (&cfg.toggle_privacy_mode, HotkeyAction::TogglePrivacyMode),
    ];

    for (binding, action) in specs {
        let Ok(hk) = parse_hotkey_binding(binding) else {
            debug!("Skipping unparsed hotkey '{binding}' for {:?}", action);
            continue;
        };
        match manager.register(hk) {
            Ok(()) => {
                info!("Registered global hotkey '{}' -> {:?}", binding, action);
                registered.insert(hk.id(), (hk, action));
            }
            Err(err) => {
                warn!("Could not register global hotkey '{binding}' ({:?}): {err}", action);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_hotkey_strings() -> Result<(), String> {
        let f9 = parse_hotkey_binding("F9")?;
        let f10 = parse_hotkey_binding("F10")?;
        let f8 = parse_hotkey_binding("F8")?;
        let mute = parse_hotkey_binding("Ctrl+Shift+M")?;
        let priv_hk = parse_hotkey_binding("Ctrl+Shift+P")?;

        assert_ne!(f9.id(), f10.id());
        assert_ne!(f8.id(), f9.id());
        assert_ne!(mute.id(), priv_hk.id());
        assert!(parse_hotkey_binding("").is_err());
        Ok(())
    }
}
