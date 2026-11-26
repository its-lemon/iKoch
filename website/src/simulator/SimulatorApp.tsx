import { useCallback, useEffect, useRef, useState } from "react";
import { EditorView } from "./components/EditorView";
import { LibraryView, type LibraryPersistedState } from "./components/LibraryView";
import { SettingsView } from "./components/SettingsView";
import { Sidebar, type AppTab } from "./components/Sidebar";
import { ToastBanner, type UiToast } from "./components/ToastBanner";
import { TopStatusBar } from "./components/TopStatusBar";
import { getTranslations, I18nProvider } from "./i18n";
import { api, subscribeMockRecorderEvents } from "./mockApi";
import type {
  AppLanguage,
  AppSettings,
  AppTheme,
  CaptureTarget,
  ClipMetadata,
  CustomThemeColors,
  DaemonStateSnapshot,
  MonitorInfo,
  UiSettings,
  WindowInfo,
} from "./types";
import {
  applyThemeToDocument,
  loadStoredLanguage,
  loadStoredTheme,
  saveStoredLanguage,
  saveStoredTheme,
} from "./utils/theme";

const INITIAL_DAEMON: DaemonStateSnapshot = {
  tray_state: "buffering",
  capture_target: { kind: "auto_game" },
  active_game: {
    process_id: 14820,
    executable_name: "cs2.exe",
    display_name: "Counter-Strike 2",
    window_title: "Counter-Strike 2",
    hwnd: 98304,
  },
  buffer_fill_sec: 30,
  buffer_capacity_sec: 30,
  buffer_bytes: Math.round(22.4 * 1024 * 1024),
  active_session: null,
  dropped_video_frames: 0,
  dropped_audio_frames: 0,
  game_muted: false,
  mic_muted: false,
  secondary_muted: false,
  mic_noise_suppression: true,
  privacy_mode: false,
};

interface SimulatorAppProps {
  externalLang?: AppLanguage;
  onSyncAccent?: (accentHex: string, bgPanelHex?: string, bgCardHex?: string) => void;
}

export function SimulatorApp({ externalLang, onSyncAccent }: SimulatorAppProps) {
  const [activeTab, setActiveTab] = useState<AppTab>("library");
  const [daemon, setDaemon] = useState<DaemonStateSnapshot>(INITIAL_DAEMON);
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [monitors, setMonitors] = useState<MonitorInfo[]>([]);
  const [windows, setWindows] = useState<WindowInfo[]>([]);
  const [refreshingSources, setRefreshingSources] = useState(false);

  const [clips, setClips] = useState<ClipMetadata[]>([]);
  const [loadingClips, setLoadingClips] = useState(false);
  const [selectedClip, setSelectedClip] = useState<ClipMetadata | null>(null);
  const [libraryState, setLibraryState] = useState<LibraryPersistedState>({
    kindTab: "all",
    gameFilter: "all",
    searchQuery: "",
    sortOrder: "newest",
  });
  const libraryScrollTopRef = useRef(0);

  const [toasts, setToasts] = useState<UiToast[]>([]);

  const initialStored = loadStoredTheme();
  const [previewTheme, setPreviewTheme] = useState<AppTheme | null>(null);
  const [previewCustomTheme, setPreviewCustomTheme] =
    useState<CustomThemeColors | null>(null);
  const [previewUi, setPreviewUi] = useState<UiSettings | null>(null);
  const [previewLang, setPreviewLang] = useState<AppLanguage | null>(null);

  const activeTheme: AppTheme =
    previewTheme ?? settings?.theme ?? initialStored.theme;
  const activeCustomTheme: CustomThemeColors =
    previewCustomTheme ?? settings?.custom_theme ?? initialStored.custom;
  const activeUi: UiSettings | undefined = previewUi ?? settings?.ui;
  const activeLanguage: AppLanguage =
    previewLang ?? externalLang ?? settings?.language ?? loadStoredLanguage();
  const t = getTranslations(activeLanguage);

  useEffect(() => {
    applyThemeToDocument(activeTheme, activeCustomTheme, activeUi);
    saveStoredTheme(activeTheme, activeCustomTheme);
    if (onSyncAccent) {
      if (activeTheme === "custom") {
        onSyncAccent(
          activeCustomTheme.accent,
          activeCustomTheme.bg_panel,
          activeCustomTheme.bg_panel
        );
      } else {
        const computed = getComputedStyle(document.documentElement);
        onSyncAccent(
          computed.getPropertyValue("--accent").trim() || "#4f6ef7",
          computed.getPropertyValue("--bg-panel").trim() || undefined,
          computed.getPropertyValue("--bg-card").trim() || undefined
        );
      }
    }
  }, [activeTheme, activeCustomTheme, activeUi, onSyncAccent]);

  useEffect(() => {
    saveStoredLanguage(activeLanguage);
  }, [activeLanguage]);

  const notify = useCallback(
    (
      title: string,
      message = "",
      variant: "success" | "error" | "info" = "info"
    ) => {
      const id = `${Date.now()}_${Math.random().toString(36).slice(2, 7)}`;
      setToasts((prev) => [...prev.slice(-3), { id, title, message, variant }]);
      window.setTimeout(() => {
        setToasts((prev) => prev.filter((item) => item.id !== id));
      }, 4200);
    },
    []
  );

  const dismissToast = useCallback((id: string) => {
    setToasts((prev) => prev.filter((item) => item.id !== id));
  }, []);

  const refreshClips = useCallback(async () => {
    setLoadingClips(true);
    try {
      const list = await api.listClips(null);
      setClips(list);
      setSelectedClip((prev) => {
        if (!prev) return null;
        return list.find((c) => c.file_path === prev.file_path) ?? prev;
      });
    } catch (err) {
      notify(t.app.libraryLoadError, String(err), "error");
    } finally {
      setLoadingClips(false);
    }
  }, [notify, t.app.libraryLoadError]);

  const refreshSources = useCallback(async () => {
    setRefreshingSources(true);
    try {
      const [mons, wins] = await Promise.all([
        api.listMonitors(),
        api.listWindows(),
      ]);
      setMonitors(mons);
      setWindows(wins);
    } catch (err) {
      notify(t.app.sourcesRefreshError, String(err), "error");
    } finally {
      setRefreshingSources(false);
    }
  }, [notify, t.app.sourcesRefreshError]);

  useEffect(() => {
    void (async () => {
      const [st, cfg] = await Promise.all([
        api.getDaemonState(),
        api.getSettings(),
      ]);
      setDaemon(st);
      setSettings(cfg);
      await Promise.all([refreshSources(), refreshClips()]);
    })();
  }, [refreshClips, refreshSources]);

  useEffect(() => {
    return subscribeMockRecorderEvents((payload) => {
      switch (payload.event) {
        case "state_changed": {
          const { event: _ev, ...snap } = payload;
          setDaemon(snap);
          break;
        }
        case "clip_saved": {
          void refreshClips();
          notify(t.app.clipSaved, payload.title, "success");
          break;
        }
        case "session_stopped": {
          void refreshClips();
          notify(t.app.sessionSaved, payload.title, "success");
          break;
        }
        case "bookmark_added": {
          notify(
            t.app.bookmarkAdded,
            `${payload.marker.label} (${payload.marker.timestamp_sec.toFixed(1)}${t.secShort})`,
            "info"
          );
          break;
        }
        default:
          break;
      }
    });
  }, [notify, refreshClips, t]);

  const handleSelectTarget = async (target: CaptureTarget) => {
    await api.setCaptureTarget(target);
    setDaemon((p) => ({ ...p, capture_target: target }));
    setSettings((p) =>
      p ? { ...p, capture: { ...p.capture, target } } : p
    );
  };

  const handleSelectMonitorOne = async () => {
    const primary = monitors.find((m) => m.is_primary) ?? monitors[0];
    const target: CaptureTarget = primary
      ? {
          kind: "monitor",
          monitor_id: primary.monitor_id,
          name: primary.name,
        }
      : {
          kind: "monitor",
          monitor_id: "\\\\.\\DISPLAY1",
          name: "Monitor 1",
        };
    await handleSelectTarget(target);
    notify(
      t.app.screenCaptureStarted,
      t.app.screenCaptureSource(target.name),
      "success"
    );
  };

  const handleSaveSettings = async (next: AppSettings) => {
    const saved = await api.saveSettings(next);
    setSettings(saved);
    setPreviewTheme(null);
    setPreviewCustomTheme(null);
    setPreviewUi(null);
    setPreviewLang(null);
    const snap = await api.getDaemonState();
    setDaemon(snap);
    await refreshClips();
  };

  const handleChangeLanguageInstant = async (lang: AppLanguage) => {
    setPreviewLang(lang);
    saveStoredLanguage(lang);
    if (!settings) return;
    const saved = await api.saveSettings({ ...settings, language: lang });
    setSettings(saved);
    setPreviewLang(null);
  };

  const handleOpenFolder = () => {
    notify(
      t.library.openFolder,
      settings?.storage.output_dir ?? "C:\\Users\\Gamer\\Videos\\iKoch",
      "info"
    );
  };

  return (
    <I18nProvider lang={activeLanguage}>
      <div className="relative flex h-[840px] w-full flex-col overflow-hidden rounded-2xl border border-white/[0.1] bg-[var(--bg-app)] text-[var(--text-primary)] shadow-2xl">
        <TopStatusBar
          daemon={daemon}
          settings={settings}
          monitors={monitors}
          windows={windows}
          refreshingSources={refreshingSources}
          onRefreshSources={() => void refreshSources()}
          onSelectTarget={(tgt) => void handleSelectTarget(tgt)}
          onSaveClip={() => void api.saveClipNow()}
          onToggleSession={() => void api.toggleSessionNow()}
          onAddBookmark={() => void api.addBookmarkNow(t.app.bookmarkUiLabel)}
          onToggleMicMute={() => void api.toggleMicMuteNow()}
          onTogglePrivacy={() => void api.togglePrivacyModeNow()}
        />

        <div className="flex flex-1 overflow-hidden">
          <Sidebar
            activeTab={activeTab}
            onSelectTab={setActiveTab}
            daemon={daemon}
            selectedClip={selectedClip}
            clipCount={clips.length}
          />

          <main className="flex-1 overflow-hidden bg-[var(--bg-app)]">
            {activeTab === "library" && (
              <LibraryView
                clips={clips}
                loading={loadingClips}
                showCardSpecs={activeUi?.show_card_specs ?? true}
                persistedState={libraryState}
                onPersistedStateChange={setLibraryState}
                scrollTopRef={libraryScrollTopRef}
                onRefresh={refreshClips}
                onOpenEditor={(clip) => {
                  setSelectedClip(clip);
                  setActiveTab("editor");
                }}
                onClipUpdated={(updated) => {
                  setClips((prev) =>
                    prev.map((c) =>
                      c.file_path === updated.file_path ? updated : c
                    )
                  );
                  setSelectedClip((prev) =>
                    prev && prev.file_path === updated.file_path ? updated : prev
                  );
                }}
                onClipDeleted={(filePath) => {
                  setClips((prev) =>
                    prev.filter((c) => c.file_path !== filePath)
                  );
                  setSelectedClip((prev) =>
                    prev && prev.file_path === filePath ? null : prev
                  );
                }}
                onOpenFolder={handleOpenFolder}
                onSelectMonitorOne={() => void handleSelectMonitorOne()}
                onOpenSettings={() => setActiveTab("settings")}
                onNotify={notify}
              />
            )}

            {activeTab === "editor" && settings && (
              <EditorView
                clip={selectedClip ?? clips[0] ?? null}
                settings={settings}
                onBackToLibrary={() => setActiveTab("library")}
                onClipCreatedOrUpdated={refreshClips}
                onOpenFolder={handleOpenFolder}
                onNotify={notify}
              />
            )}

            {activeTab === "settings" && settings && (
              <SettingsView
                settings={settings}
                monitors={monitors}
                windows={windows}
                onRefreshSources={refreshSources}
                onSaveSettings={handleSaveSettings}
                onPreviewTheme={setPreviewTheme}
                onPreviewCustomTheme={setPreviewCustomTheme}
                onPreviewUi={setPreviewUi}
                onChangeLanguageInstant={(l) =>
                  void handleChangeLanguageInstant(l)
                }
                onOpenFolder={handleOpenFolder}
                onNotify={notify}
              />
            )}
          </main>
        </div>

        <ToastBanner toasts={toasts} onDismiss={dismissToast} />
      </div>
    </I18nProvider>
  );
}
