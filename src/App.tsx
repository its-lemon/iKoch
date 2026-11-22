import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api } from "./api";
import { EditorView } from "./components/EditorView";
import { LibraryView, type LibraryPersistedState } from "./components/LibraryView";
import { SettingsView } from "./components/SettingsView";
import { Sidebar, type AppTab } from "./components/Sidebar";
import { ToastBanner, type UiToast } from "./components/ToastBanner";
import { TopStatusBar } from "./components/TopStatusBar";
import { getTranslations, I18nProvider } from "./i18n";
import type {
  AppLanguage,
  AppSettings,
  AppTheme,
  CaptureTarget,
  ClipMetadata,
  CustomThemeColors,
  DaemonStateSnapshot,
  MonitorInfo,
  RecorderEvent,
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
  tray_state: "idle",
  capture_target: { kind: "auto_game" },
  active_game: null,
  buffer_fill_sec: 0,
  buffer_capacity_sec: 30,
  buffer_bytes: 0,
  active_session: null,
  dropped_video_frames: 0,
  dropped_audio_frames: 0,
  game_muted: false,
  mic_muted: false,
  secondary_muted: false,
  mic_noise_suppression: true,
  privacy_mode: false,
};

function tabFromHash(): AppTab {
  const raw = window.location.hash.replace(/^#/, "").toLowerCase();
  if (raw === "settings") return "settings";
  if (raw === "editor") return "editor";
  return "library";
}

export function App() {
  const [activeTab, setActiveTab] = useState<AppTab>(() => tabFromHash());
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
    previewLang ?? settings?.language ?? loadStoredLanguage();
  const t = getTranslations(activeLanguage);

  useEffect(() => {
    applyThemeToDocument(activeTheme, activeCustomTheme, activeUi);
    saveStoredTheme(activeTheme, activeCustomTheme);
  }, [activeTheme, activeCustomTheme, activeUi]);

  useEffect(() => {
    saveStoredLanguage(activeLanguage);
  }, [activeLanguage]);

  // Suppress WebView2 native context menu outside editable text inputs.
  useEffect(() => {
    const handleGlobalContextMenu = (e: MouseEvent) => {
      const target = e.target as HTMLElement | null;
      if (
        target &&
        (target.tagName === "INPUT" ||
          target.tagName === "TEXTAREA" ||
          target.isContentEditable)
      ) {
        return;
      }
      e.preventDefault();
    };
    window.addEventListener("contextmenu", handleGlobalContextMenu);
    return () =>
      window.removeEventListener("contextmenu", handleGlobalContextMenu);
  }, []);

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

  // Initial load + background telemetry polling.
  useEffect(() => {
    getCurrentWindow().setTitle("iKoch").catch(() => {});

    void (async () => {
      try {
        const [st, cfg] = await Promise.all([
          api.getDaemonState(),
          api.getSettings(),
        ]);
        setDaemon(st);
        setSettings(cfg);
        if (cfg.check_updates_on_startup) {
          void api
            .checkForUpdates()
            .then((info) => {
              if (info.has_update) {
                const tr = getTranslations(cfg.language);
                notify(
                  `${tr.settings.updateAvailable}: v${info.latest_version}`,
                  tr.settings.updatesTitle,
                  "info"
                );
              }
            })
            .catch(() => {});
        }
      } catch (err) {
        notify(
          getTranslations(loadStoredLanguage()).app.initError,
          String(err),
          "error"
        );
      }
      await Promise.all([refreshSources(), refreshClips()]);
    })();

    const pollTimer = window.setInterval(() => {
      api
        .getDaemonState()
        .then((snap) => setDaemon(snap))
        .catch(() => {});
    }, 1500);

    return () => window.clearInterval(pollTimer);
  }, [notify, refreshClips, refreshSources]);

  // Refresh state and clips when window regains focus (e.g. reopened from tray).
  useEffect(() => {
    const onFocus = () => {
      api
        .getDaemonState()
        .then((snap) => setDaemon(snap))
        .catch(() => {});
      void refreshClips();
      void refreshSources();
    };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [refreshClips, refreshSources]);

  // Hash navigation listener for tray shortcuts (#gallery / #settings).
  useEffect(() => {
    const onHashChange = () => setActiveTab(tabFromHash());
    window.addEventListener("hashchange", onHashChange);
    return () => window.removeEventListener("hashchange", onHashChange);
  }, []);

  // Tauri live event bus subscription (cleaned up properly on unmount/reload).
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    listen<RecorderEvent>("ikoch://event", (ev) => {
      const payload = ev.payload;
      if (!payload) return;

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
        case "show_overlay_toast": {
          if (payload.kind === "warning" || payload.kind === "error") {
            notify(
              payload.title,
              payload.message,
              payload.kind === "error" ? "error" : "info"
            );
          }
          break;
        }
        case "storage_cleanup_completed": {
          if (payload.deleted_files > 0) {
            void refreshClips();
          }
          break;
        }
        case "pipeline_error": {
          notify(
            t.app.pipelineErrorTitle(payload.component),
            payload.message,
            "error"
          );
          break;
        }
        default:
          break;
      }
    })
      .then((fn) => {
        if (disposed) {
          fn();
        } else {
          unlisten = fn;
        }
      })
      .catch(() => {});

    return () => {
      disposed = true;
      if (unlisten) unlisten();
    };
  }, [notify, refreshClips, t]);

  const selectTab = (tab: AppTab) => {
    setActiveTab(tab);
    window.location.hash =
      tab === "library" ? "gallery" : tab === "settings" ? "settings" : "editor";
  };

  const handleSelectTarget = async (target: CaptureTarget) => {
    try {
      await api.setCaptureTarget(target);
      setDaemon((p) => ({ ...p, capture_target: target }));
      setSettings((p) =>
        p ? { ...p, capture: { ...p.capture, target } } : p
      );
    } catch (err) {
      notify(t.app.targetSwitchError, String(err), "error");
    }
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
    try {
      const saved = await api.saveSettings({ ...settings, language: lang });
      setSettings(saved);
      setPreviewLang(null);
    } catch (err) {
      notify(
        getTranslations(lang).settings.saveErrorTitle,
        String(err),
        "error"
      );
    }
  };

  const handleOpenFolder = () => {
    api.openClipsFolder().catch((err) => {
      notify(t.app.openFolderError, String(err), "error");
    });
  };

  return (
    <I18nProvider lang={activeLanguage}>
      <div className="flex h-screen w-screen flex-col overflow-hidden bg-[var(--bg-app)] text-[var(--text-primary)]">
        <TopStatusBar
          daemon={daemon}
          settings={settings}
          monitors={monitors}
          windows={windows}
          refreshingSources={refreshingSources}
          onRefreshSources={() => void refreshSources()}
          onSelectTarget={(tgt) => void handleSelectTarget(tgt)}
          onSaveClip={() =>
            void api
              .saveClipNow()
              .catch((e) => notify(t.app.clipError, String(e), "error"))
          }
          onToggleSession={() =>
            void api
              .toggleSessionNow()
              .catch((e) => notify(t.app.sessionError, String(e), "error"))
          }
          onAddBookmark={() =>
            void api
              .addBookmarkNow(t.app.bookmarkUiLabel)
              .catch((e) => notify(t.app.bookmarkError, String(e), "error"))
          }
          onToggleMicMute={() =>
            void api
              .toggleMicMuteNow()
              .catch((e) => notify(t.app.micError, String(e), "error"))
          }
          onTogglePrivacy={() =>
            void api
              .togglePrivacyModeNow()
              .catch((e) => notify(t.app.privacyError, String(e), "error"))
          }
        />

        <div className="flex flex-1 overflow-hidden">
          <Sidebar
            activeTab={activeTab}
            onSelectTab={selectTab}
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
                  selectTab("editor");
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
                onOpenSettings={() => selectTab("settings")}
                onNotify={notify}
              />
            )}

            {activeTab === "editor" && settings && (
              <EditorView
                clip={selectedClip}
                allClips={clips}
                settings={settings}
                onBackToLibrary={() => selectTab("library")}
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

export default App;
