import { useCallback, useEffect, useRef, useState } from "react";
import {
  Activity,
  Bell,
  Check,
  CheckCircle2,
  ChevronDown,
  Database,
  Download,
  FolderOpen,
  Gamepad2,
  Globe,
  HardDrive,
  Keyboard,
  Mic,
  Monitor,
  Palette,
  Plus,
  RefreshCw,
  RotateCcw,
  Sliders,
  Trash2,
  Volume2,
  VolumeX,
  X,
} from "lucide-react";
import { api } from "../mockApi";
import { useI18n } from "../i18n";
import type {
  ActiveAudioAppInfo,
  AppAudioSlotConfig,
  AppLanguage,
  AppSettings,
  AppTheme,
  AudioDeviceInfo,
  UpdateInfo,
  CacheCleanupMode,
  CaptureTarget,
  CustomThemeColors,
  EditorCacheStats,
  GpuVendorPreference,
  HdrMode,
  MonitorInfo,
  NotificationSoundEntry,
  RateControlMode,
  ResolutionPreset,
  SoundNotificationKind,
  UiCornerRadius,
  UiDensity,
  UiSettings,
  VideoCodec,
  WindowInfo,
} from "../types";
import { captureTargetKey, formatBytes } from "../utils/format";
import {
  DEFAULT_CUSTOM_THEME,
  DEFAULT_UI_SETTINGS,
  THEME_PRESETS,
  isValidHex,
  normalizeCustomColors,
} from "../utils/theme";
import { AccentSlider } from "./AccentSlider";
import { CustomSelect, type SelectOption } from "./CustomSelect";
import { InfoTooltip } from "./InfoTooltip";

type SettingsTab =
  | "capture"
  | "audio"
  | "notifications"
  | "appearance"
  | "hotkeys_storage";

interface SettingsViewProps {
  settings: AppSettings;
  monitors: MonitorInfo[];
  windows: WindowInfo[];
  onRefreshSources: () => Promise<void>;
  onSaveSettings: (next: AppSettings) => Promise<void>;
  onPreviewTheme: (theme: AppTheme | null) => void;
  onPreviewCustomTheme: (colors: CustomThemeColors | null) => void;
  onPreviewUi: (ui: UiSettings | null) => void;
  onChangeLanguageInstant: (lang: AppLanguage) => void;
  onOpenFolder: () => void;
  onNotify: (
    title: string,
    message: string,
    variant?: "success" | "error" | "info"
  ) => void;
}

function ToggleSwitch({
  checked,
  onChange,
  label,
  hint,
  tooltip,
}: {
  checked: boolean;
  onChange: (next: boolean) => void;
  label: string;
  hint?: string;
  tooltip?: string;
}) {
  return (
    <div
      role="switch"
      aria-checked={checked}
      tabIndex={0}
      onClick={() => onChange(!checked)}
      onKeyDown={(e) => {
        if (e.key === " " || e.key === "Enter") {
          e.preventDefault();
          onChange(!checked);
        }
      }}
      className="flex w-full cursor-pointer items-center justify-between gap-3 rounded-xl bg-[var(--bg-card,rgba(255,255,255,0.025))] px-3.5 py-2.5 text-left ring-1 ring-[var(--border-subtle,rgba(255,255,255,0.06))] transition hover:ring-[rgb(var(--accent-rgb)/0.45)]"
    >
      <div className="min-w-0">
        <div className="flex items-center gap-1.5 text-xs font-medium text-[var(--text-primary,#e2e8f0)]">
          <span>{label}</span>
          {tooltip && <InfoTooltip text={tooltip} />}
        </div>
        {hint && (
          <div className="mt-0.5 text-[11px] text-[var(--text-secondary,#94a3b8)]">
            {hint}
          </div>
        )}
      </div>
      <span
        className={`relative inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors ${
          checked ? "bg-[var(--accent)]" : "bg-white/[0.14]"
        }`}
      >
        <span
          className={`inline-block h-3.5 w-3.5 transform rounded-full bg-white shadow transition-transform ${
            checked ? "translate-x-5" : "translate-x-1"
          }`}
        />
      </span>
    </div>
  );
}

function SegmentedControl<T extends string>({
  value,
  options,
  onChange,
}: {
  value: T;
  options: { value: T; label: string; warn?: boolean; title?: string }[];
  onChange: (next: T) => void;
}) {
  return (
    <div className="flex flex-wrap gap-1 rounded-xl bg-black/35 p-1">
      {options.map((opt) => {
        const active = opt.value === value;
        return (
          <button
            key={opt.value}
            type="button"
            title={opt.title}
            onClick={() => onChange(opt.value)}
            className={`inline-flex flex-1 items-center justify-center gap-1.5 whitespace-nowrap rounded-lg px-3 py-1.5 text-xs font-semibold transition ${
              active
                ? "bg-[var(--accent)] text-white shadow-sm"
                : opt.warn
                  ? "text-amber-300/80 hover:bg-white/[0.06] hover:text-amber-200"
                  : "text-slate-400 hover:bg-white/[0.06] hover:text-slate-100"
            }`}
          >
            {opt.warn && (
              <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-amber-400" />
            )}
            <span>{opt.label}</span>
          </button>
        );
      })}
    </div>
  );
}

function AdvancedAccordion({
  title,
  hint,
  children,
}: {
  title: string;
  hint?: string;
  children: React.ReactNode;
}) {
  const [open, setOpen] = useState(false);
  return (
    <div className="overflow-hidden rounded-xl bg-white/[0.02] ring-1 ring-white/[0.05]">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="flex w-full items-center justify-between gap-3 px-4 py-3 text-left transition hover:bg-white/[0.03]"
      >
        <div className="flex min-w-0 items-center gap-2.5">
          <Sliders className="h-3.5 w-3.5 shrink-0 text-[var(--accent-hover)]" />
          <div className="min-w-0">
            <div className="text-xs font-semibold text-slate-200">{title}</div>
            {hint && !open && (
              <div className="truncate text-[11px] text-slate-500">{hint}</div>
            )}
          </div>
        </div>
        <ChevronDown
          className={`h-4 w-4 shrink-0 text-slate-400 transition-transform duration-200 ${
            open ? "rotate-180 text-white" : ""
          }`}
        />
      </button>
      {open && (
        <div className="space-y-4 border-t border-white/[0.05] px-4 py-4">
          {children}
        </div>
      )}
    </div>
  );
}

function keyEventToComboString(e: React.KeyboardEvent): string | null {
  const ignored = ["Control", "Shift", "Alt", "Meta", "OS"];
  if (ignored.includes(e.key)) return null;
  const parts: string[] = [];
  if (e.ctrlKey) parts.push("Ctrl");
  if (e.altKey) parts.push("Alt");
  if (e.shiftKey) parts.push("Shift");

  let keyName = e.key;
  if (keyName === " ") keyName = "Space";
  else if (keyName.length === 1) keyName = keyName.toUpperCase();
  parts.push(keyName);
  return parts.join("+");
}

function ExeListEditor({
  title,
  hint,
  items,
  placeholder,
  addLabel,
  emptyLabel,
  onChange,
}: {
  title: string;
  hint: string;
  items: string[];
  placeholder: string;
  addLabel: string;
  emptyLabel: string;
  onChange: (next: string[]) => void;
}) {
  const [input, setInput] = useState("");
  const add = () => {
    const clean = input.trim().toLowerCase();
    if (!clean || items.includes(clean)) return;
    onChange([...items, clean]);
    setInput("");
  };
  return (
    <div className="rounded-xl bg-white/[0.025] p-3.5 ring-1 ring-white/[0.06]">
      <div className="text-xs font-semibold text-white">{title}</div>
      <div className="mt-0.5 text-[11px] text-slate-400">{hint}</div>
      <div className="mt-2.5 flex gap-2">
        <input
          type="text"
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              add();
            }
          }}
          placeholder={placeholder}
          className="flex-1 rounded-xl bg-black/35 px-3 py-1.5 text-xs text-white placeholder-slate-500 ring-1 ring-white/[0.08] outline-none focus:ring-[var(--accent)]"
        />
        <button
          type="button"
          onClick={add}
          className="inline-flex items-center gap-1 rounded-xl bg-[var(--accent)] px-3 py-1.5 text-xs font-semibold text-white transition hover:bg-[var(--accent-hover)]"
        >
          <Plus className="h-3.5 w-3.5" />
          {addLabel}
        </button>
      </div>
      <div className="mt-2.5 flex flex-wrap gap-1.5">
        {items.length === 0 ? (
          <span className="text-[11px] text-slate-500">{emptyLabel}</span>
        ) : (
          items.map((exe) => (
            <span
              key={exe}
              className="inline-flex items-center gap-1 rounded-lg bg-white/[0.06] px-2.5 py-1 font-mono text-[11px] text-slate-200 ring-1 ring-white/[0.07]"
            >
              {exe}
              <button
                type="button"
                onClick={() => onChange(items.filter((x) => x !== exe))}
                className="text-slate-400 hover:text-rose-400"
              >
                <X className="h-3 w-3" />
              </button>
            </span>
          ))
        )}
      </div>
    </div>
  );
}

export function SettingsView({
  settings,
  monitors,
  windows,
  onRefreshSources,
  onSaveSettings,
  onPreviewTheme,
  onPreviewCustomTheme,
  onPreviewUi,
  onChangeLanguageInstant,
  onOpenFolder,
  onNotify,
}: SettingsViewProps) {
  const { t, lang } = useI18n();
  const [tab, setTab] = useState<SettingsTab>("capture");
  const [draft, setDraft] = useState<AppSettings>(() =>
    structuredClone(settings)
  );
  const [hexInputs, setHexInputs] = useState<CustomThemeColors>(() =>
    normalizeCustomColors(settings.custom_theme)
  );
  const prevSettingsRef = useRef<AppSettings>(settings);
  const [saving, setSaving] = useState(false);
  const [justSaved, setJustSaved] = useState(false);
  const autoSaveTimerRef = useRef<number | null>(null);

  const [outDevices, setOutDevices] = useState<AudioDeviceInfo[]>([]);
  const [inDevices, setInDevices] = useState<AudioDeviceInfo[]>([]);
  const [capturingKeyField, setCapturingKeyField] = useState<string | null>(
    null
  );

  const [cacheStats, setCacheStats] = useState<EditorCacheStats>({
    temp_bytes: 0,
    temp_file_count: 0,
    metadata_bytes: 0,
    metadata_file_count: 0,
  });
  const [clearingCache, setClearingCache] = useState(false);

  // Live microphone level tester state
  const [micTesting, setMicTesting] = useState(false);
  const [micLevelPct, setMicLevelPct] = useState(0);
  const micStreamRef = useRef<MediaStream | null>(null);
  const micCtxRef = useRef<AudioContext | null>(null);
  const micRafRef = useRef<number | null>(null);

  useEffect(() => {
    const prev = prevSettingsRef.current;
    prevSettingsRef.current = settings;
    const onlyLanguageChanged =
      prev.language !== settings.language &&
      JSON.stringify({ ...prev, language: settings.language }) ===
        JSON.stringify(settings);

    if (onlyLanguageChanged) {
      setDraft((d) => ({ ...d, language: settings.language }));
      return;
    }

    setDraft(structuredClone(settings));
    setHexInputs(normalizeCustomColors(settings.custom_theme));
  }, [settings]);

  // Restore saved theme if user exits Settings without saving a previewed theme.
  useEffect(() => {
    return () => {
      onPreviewTheme(null);
      onPreviewCustomTheme(null);
      onPreviewUi(null);
    };
  }, [onPreviewCustomTheme, onPreviewTheme, onPreviewUi]);

  const refreshCacheStats = useCallback(async () => {
    try {
      const stats = await api.getEditorCacheStats();
      setCacheStats(stats);
    } catch {
      // Non-critical background readout
    }
  }, []);

  useEffect(() => {
    void refreshCacheStats();
  }, [refreshCacheStats]);

  const [activeAudioApps, setActiveAudioApps] = useState<ActiveAudioAppInfo[]>(
    []
  );
  const [appPickerOpen, setAppPickerOpen] = useState(false);
  const [loadingAudioApps, setLoadingAudioApps] = useState(false);

  const loadActiveAudioApps = useCallback(async () => {
    setLoadingAudioApps(true);
    try {
      const apps = await api.listActiveAudioApps();
      setActiveAudioApps(apps);
      setAppPickerOpen(true);
    } catch {
      setActiveAudioApps([]);
      setAppPickerOpen(true);
    } finally {
      setLoadingAudioApps(false);
    }
  }, []);

  const stopMicTester = () => {
    if (micRafRef.current !== null) {
      cancelAnimationFrame(micRafRef.current);
      micRafRef.current = null;
    }
    if (micStreamRef.current) {
      for (const track of micStreamRef.current.getTracks()) {
        track.stop();
      }
      micStreamRef.current = null;
    }
    if (micCtxRef.current) {
      void micCtxRef.current.close().catch(() => {});
      micCtxRef.current = null;
    }
    setMicTesting(false);
    setMicLevelPct(0);
  };

  useEffect(() => {
    return () => stopMicTester();
  }, []);

  const toggleMicTester = async () => {
    if (micTesting) {
      stopMicTester();
      return;
    }
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      const ctx = new AudioContext();
      const src = ctx.createMediaStreamSource(stream);
      const analyser = ctx.createAnalyser();
      analyser.fftSize = 256;
      src.connect(analyser);

      micStreamRef.current = stream;
      micCtxRef.current = ctx;
      setMicTesting(true);

      const buf = new Uint8Array(analyser.frequencyBinCount);
      const update = () => {
        analyser.getByteTimeDomainData(buf);
        let peak = 0;
        for (let i = 0; i < buf.length; i += 1) {
          const v = Math.abs(buf[i] - 128) / 128;
          if (v > peak) peak = v;
        }
        setMicLevelPct(Math.min(100, Math.round(peak * 180)));
        micRafRef.current = requestAnimationFrame(update);
      };
      update();
    } catch (err) {
      onNotify(t.settings.micTesterUnavailable, String(err), "info");
    }
  };

  const loadAudioDevices = useCallback(async () => {
    try {
      const [outs, ins] = await Promise.all([
        api.listAudioOutputDevices(),
        api.listAudioInputDevices(),
      ]);
      setOutDevices(outs);
      setInDevices(ins);
    } catch (err) {
      onNotify(t.settings.audioDevicesError, String(err), "error");
    }
  }, [onNotify, t.settings.audioDevicesError]);

  useEffect(() => {
    void loadAudioDevices();
  }, [loadAudioDevices]);

  // Auto-save settings on the fly whenever `draft` diverges from `settings`.
  useEffect(() => {
    if (JSON.stringify(draft) === JSON.stringify(settings)) {
      return;
    }
    if (autoSaveTimerRef.current !== null) {
      window.clearTimeout(autoSaveTimerRef.current);
    }
    autoSaveTimerRef.current = window.setTimeout(() => {
      autoSaveTimerRef.current = null;
      setSaving(true);
      void onSaveSettings(draft)
        .then(() => {
          setJustSaved(true);
          window.setTimeout(() => setJustSaved(false), 2200);
        })
        .catch((err) => {
          onNotify(t.settings.saveErrorTitle, String(err), "error");
        })
        .finally(() => {
          setSaving(false);
        });
    }, 280);

    return () => {
      if (autoSaveTimerRef.current !== null) {
        window.clearTimeout(autoSaveTimerRef.current);
        autoSaveTimerRef.current = null;
      }
    };
  }, [draft, onNotify, onSaveSettings, settings, t.settings.saveErrorTitle]);

  const handleSelectLanguage = (nextLang: AppLanguage) => {
    setDraft((p) => ({ ...p, language: nextLang }));
    onChangeLanguageInstant(nextLang);
  };

  const handleCustomColorInput = (
    field: keyof CustomThemeColors,
    rawValue: string
  ) => {
    setHexInputs((prev) => ({ ...prev, [field]: rawValue }));
    if (isValidHex(rawValue)) {
      const cleanHex = rawValue.trim().toUpperCase();
      setHexInputs((prev) => ({ ...prev, [field]: cleanHex }));
      const nextCustom = normalizeCustomColors({
        ...draft.custom_theme,
        [field]: cleanHex,
      });
      setDraft((p) => ({
        ...p,
        theme: "custom",
        custom_theme: nextCustom,
      }));
      onPreviewTheme("custom");
      onPreviewCustomTheme(nextCustom);
    }
  };

  const handleResetCustomTheme = () => {
    const reset = { ...DEFAULT_CUSTOM_THEME };
    setHexInputs(reset);
    setDraft((p) => ({
      ...p,
      theme: "custom",
      custom_theme: reset,
    }));
    onPreviewTheme("custom");
    onPreviewCustomTheme(reset);
  };

  const applyLiveVolumes = (
    gameVol: number,
    micVol: number,
    secVol: number
  ) => {
    void api
      .sendRecorderCommand({
        cmd: "set_audio_volumes",
        game_volume: gameVol,
        mic_volume: micVol,
        secondary_volume: secVol,
      })
      .catch(() => {});
  };

  const [checkingUpdates, setCheckingUpdates] = useState(false);
  const [installingUpdate, setInstallingUpdate] = useState(false);
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null);
  const [supportedCodecs, setSupportedCodecs] = useState<VideoCodec[]>([
    "h264",
    "hevc",
    "av1",
  ]);

  useEffect(() => {
    api
      .getSupportedCodecs()
      .then((codecs) => {
        if (Array.isArray(codecs) && codecs.length > 0) {
          setSupportedCodecs(codecs);
        }
      })
      .catch(() => {});
  }, []);

  const handleCheckUpdates = async () => {
    setCheckingUpdates(true);
    try {
      const info = await api.checkForUpdates();
      setUpdateInfo(info);
      if (!info.has_update) {
        onNotify(t.settings.updatesTitle, t.settings.upToDate, "info");
      }
    } catch (err) {
      onNotify(t.settings.updatesTitle, String(err), "error");
    } finally {
      setCheckingUpdates(false);
    }
  };

  const handleInstallUpdate = async () => {
    if (!updateInfo?.download_url) return;
    setInstallingUpdate(true);
    try {
      await api.downloadAndInstallUpdate(
        updateInfo.download_url,
        updateInfo.installer_name
      );
    } catch (err) {
      onNotify(t.settings.updatesTitle, String(err), "error");
      setInstallingUpdate(false);
    }
  };

  const applyLiveRnnoise = (
    enabled: boolean,
    strength: number,
    gateDb: number | null
  ) => {
    void api
      .sendRecorderCommand({
        cmd: "set_mic_noise_suppression",
        enabled,
        strength,
        noise_gate_db: gateDb,
      })
      .catch(() => {});
  };

  const handleTestSound = async (kind: SoundNotificationKind) => {
    try {
      await onSaveSettings(draft);
      await api.playTestSound(kind);
    } catch (err) {
      onNotify(t.settings.testSoundError, String(err), "error");
    }
  };

  const handleCleanupNow = async () => {
    try {
      await onSaveSettings(draft);
      const [deleted, freed] = await api.runStorageCleanupNow();
      onNotify(
        t.settings.diskCleanedTitle,
        t.settings.diskCleanedMsg(deleted, formatBytes(freed, lang)),
        "info"
      );
    } catch (err) {
      onNotify(t.settings.diskCleanError, String(err), "error");
    }
  };

  const handleClearEditorCacheNow = async () => {
    setClearingCache(true);
    try {
      const [deleted, freed] = await api.clearEditorCacheNow();
      await refreshCacheStats();
      onNotify(
        t.settings.cacheClearedTitle,
        t.settings.cacheClearedMsg(deleted, formatBytes(freed, lang)),
        "success"
      );
    } catch (err) {
      onNotify(t.settings.cacheClearError, String(err), "error");
    } finally {
      setClearingCache(false);
    }
  };

  const targetOptions: { key: string; label: string; target: CaptureTarget }[] =
    [
      {
        key: "auto_game",
        label: t.topBar.autoGame,
        target: { kind: "auto_game" },
      },
      ...monitors.map((m) => ({
        key: `monitor:${m.monitor_id}`,
        label: t.topBar.monitorPrefix(m.name),
        target: {
          kind: "monitor",
          monitor_id: m.monitor_id,
          name: m.name,
        } as CaptureTarget,
      })),
      ...windows.map((w) => ({
        key: `window:${w.hwnd}`,
        label: t.topBar.windowPrefix(w.process_name, w.title),
        target: {
          kind: "specific_window",
          hwnd: w.hwnd,
          process_name: w.process_name,
          title: w.title,
        } as CaptureTarget,
      })),
    ];
  const draftTargetKey = captureTargetKey(draft.capture.target);
  if (!targetOptions.some((o) => o.key === draftTargetKey)) {
    if (draft.capture.target.kind === "monitor") {
      targetOptions.push({
        key: draftTargetKey,
        label: t.topBar.monitorPrefix(draft.capture.target.name),
        target: draft.capture.target,
      });
    } else if (draft.capture.target.kind === "specific_window") {
      targetOptions.push({
        key: draftTargetKey,
        label: t.topBar.windowPrefix(
          draft.capture.target.process_name,
          draft.capture.target.title
        ),
        target: draft.capture.target,
      });
    }
  }

  const updateSoundEntry = (
    key: keyof AppSettings["notifications"]["sounds"],
    updater: (prev: NotificationSoundEntry) => NotificationSoundEntry
  ) => {
    setDraft((prev) => ({
      ...prev,
      notifications: {
        ...prev.notifications,
        sounds: {
          ...prev.notifications.sounds,
          [key]: updater(prev.notifications.sounds[key]),
        },
      },
    }));
  };

  const renderHotkeyInput = (
    fieldId: string,
    label: string,
    value: string,
    onChangeVal: (val: string) => void
  ) => {
    const isRecording = capturingKeyField === fieldId;
    return (
      <div className="flex items-center justify-between gap-3 rounded-xl bg-white/[0.025] px-3.5 py-2.5 ring-1 ring-white/[0.06]">
        <span className="text-xs font-medium text-slate-200">{label}</span>
        <div className="flex items-center gap-2">
          <input
            type="text"
            value={isRecording ? t.settings.pressKey : value}
            readOnly={isRecording}
            onKeyDown={(e) => {
              if (!isRecording) return;
              e.preventDefault();
              if (e.key === "Escape") {
                setCapturingKeyField(null);
                return;
              }
              const combo = keyEventToComboString(e);
              if (combo) {
                onChangeVal(combo);
                setCapturingKeyField(null);
              }
            }}
            onChange={(e) => {
              if (!isRecording) onChangeVal(e.target.value);
            }}
            onBlur={() => {
              if (isRecording) setCapturingKeyField(null);
            }}
            className={`w-40 rounded-xl px-2.5 py-1.5 text-center font-mono text-xs ring-1 outline-none ${
              isRecording
                ? "bg-[rgb(var(--accent-rgb)/0.2)] text-white ring-[var(--accent)]"
                : "bg-black/35 text-white ring-white/[0.08]"
            }`}
          />
          <button
            type="button"
            onClick={() => setCapturingKeyField(isRecording ? null : fieldId)}
            className="rounded-xl bg-white/[0.06] px-3 py-1.5 text-[11px] font-medium text-slate-200 transition hover:bg-white/[0.12] hover:text-white"
          >
            {isRecording ? t.settings.cancelBtn : t.settings.assignBtn}
          </button>
        </div>
      </div>
    );
  };

  const fieldCls =
    "mt-1.5 w-full rounded-xl bg-black/35 px-3 py-2 text-xs text-white ring-1 ring-white/[0.08] outline-none transition focus:ring-[var(--accent)]";
  const labelTitleCls = "mb-1.5 text-xs font-medium text-slate-300";
  const sectionCls =
    "space-y-5 rounded-2xl bg-[var(--bg-panel,rgba(255,255,255,0.025))] p-6 ring-1 ring-white/[0.05]";
  const actionChipBtn =
    "inline-flex items-center gap-1.5 rounded-xl bg-white/[0.06] px-3 py-1.5 text-[11px] font-medium text-slate-200 transition hover:bg-white/[0.12] hover:text-white";

  // SelectOption lists
  const captureSourceSelectOptions: SelectOption[] = targetOptions.map((o) => ({
    value: o.key,
    label: o.label,
  }));

  const hdrOptions: SelectOption[] = [
    { value: "auto_tone_map_to_sdr", label: t.settings.hdrAuto },
    { value: "force_sdr_clamp", label: t.settings.hdrForce },
    { value: "preserve_hdr10", label: t.settings.hdrPreserve },
  ];

  const av1Supported = supportedCodecs.includes("av1");

  const gpuOptions: SelectOption[] = [
    { value: "auto", label: t.settings.gpuAuto },
    { value: "nvidia_nvenc", label: t.settings.gpuNvenc },
    { value: "amd_amf", label: t.settings.gpuAmf },
    { value: "intel_qsv", label: t.settings.gpuQsv },
    { value: "software_fallback", label: t.settings.gpuSoft },
  ];

  const rateControlOptions: SelectOption[] = [
    { value: "vbr", label: t.settings.rcVbr },
    { value: "cbr", label: t.settings.rcCbr },
    { value: "cqp", label: t.settings.rcCqp },
  ];

  const gameAudioOptions: SelectOption[] = [
    { value: "default_device", label: t.settings.gameAudioDefault },
    { value: "game_process_only", label: t.settings.gameAudioProcess },
    ...outDevices.map((d) => ({
      value: `spec:${d.id}`,
      label: `${d.is_default ? "[Default] " : ""}${d.name}`,
    })),
  ];

  const secondaryAudioOptions: SelectOption[] = [
    { value: "", label: t.settings.disabledOption },
    ...outDevices.map((d) => ({
      value: d.id,
      label: d.name,
    })),
  ];

  const micDeviceOptions: SelectOption[] = [
    { value: "none", label: t.settings.micNone },
    { value: "default", label: t.settings.micDefault },
    ...inDevices.map((d) => ({
      value: `spec:${d.id}`,
      label: `${d.is_default ? "[Default] " : ""}${d.name}`,
    })),
  ];

  const micModeOptions: SelectOption[] = [
    { value: "always_on", label: t.settings.micAlwaysOn },
    { value: "push_to_talk", label: t.settings.micPtt },
    { value: "push_to_mute", label: t.settings.micPtm },
  ];

  const soundKindOptions: SelectOption[] = [
    { value: "preset", label: t.settings.soundPresetOpt },
    { value: "custom_file", label: t.settings.soundCustomOpt },
    { value: "silent", label: t.settings.soundSilentOpt },
  ];

  const defaultPresetForKind = (kind: SoundNotificationKind): string => {
    switch (kind) {
      case "clip_saved":
        return "clip_crystal";
      case "recording_started":
        return "start_double";
      case "recording_stopped":
        return "stop_double";
      case "marker_added":
        return "marker_tick";
    }
  };

  const normalizePresetValueForKind = (
    kind: SoundNotificationKind,
    raw: string
  ): string => {
    const low = raw.trim().toLowerCase();
    if (kind === "clip_saved" && low === "chime") return "clip_crystal";
    if (kind === "recording_started" && low === "start") return "start_double";
    if (kind === "recording_stopped" && low === "stop") return "stop_double";
    if (kind === "marker_added" && low === "tick") return "marker_tick";
    return raw;
  };

  const soundPresetOptionsFor = (kind: SoundNotificationKind): SelectOption[] => {
    switch (kind) {
      case "clip_saved":
        return [
          { value: "clip_crystal", label: t.settings.presetClipCrystal },
          { value: "clip_medal", label: t.settings.presetClipMedal },
          { value: "clip_bell", label: t.settings.presetClipBell },
          { value: "clip_studio", label: t.settings.presetClipStudio },
        ];
      case "recording_started":
        return [
          { value: "start_double", label: t.settings.presetStartDouble },
          { value: "start_synth", label: t.settings.presetStartSynth },
          { value: "start_click", label: t.settings.presetStartClick },
        ];
      case "recording_stopped":
        return [
          { value: "stop_double", label: t.settings.presetStopDouble },
          { value: "stop_synth", label: t.settings.presetStopSynth },
          { value: "stop_click", label: t.settings.presetStopClick },
        ];
      case "marker_added":
        return [
          { value: "marker_tick", label: t.settings.presetMarkerTick },
          { value: "marker_wood", label: t.settings.presetMarkerWood },
          { value: "marker_pulse", label: t.settings.presetMarkerPulse },
        ];
    }
  };

  const msUnit = lang === "en" ? "ms" : "мс";
  const minShortUnit = lang === "en" ? "m" : "м";
  const customPreviewColors = normalizeCustomColors(draft.custom_theme);
  const currentUi: UiSettings = draft.ui ?? DEFAULT_UI_SETTINGS;

  const updateUiSetting = (patch: Partial<UiSettings>) => {
    const nextUi: UiSettings = { ...currentUi, ...patch };
    setDraft((p) => ({ ...p, ui: nextUi }));
    onPreviewUi(nextUi);
  };

  const customColorFields: {
    key: keyof CustomThemeColors;
    label: string;
  }[] = [
    { key: "bg_app", label: t.settings.colorBgApp },
    { key: "bg_sidebar", label: t.settings.colorBgSidebar },
    { key: "bg_panel", label: t.settings.colorBgPanel },
    { key: "border_color", label: t.settings.colorBorder },
    { key: "accent", label: t.settings.colorAccent },
    { key: "text_primary", label: t.settings.colorTextPrimary },
    { key: "text_secondary", label: t.settings.colorTextSecondary },
  ];

  return (
    <div className="flex h-full flex-col overflow-hidden bg-[var(--bg-app)]">
      {/* Header with Quick Language Toggle & Quiet Auto-Save Indicator */}
      <div className="flex flex-wrap items-center justify-between gap-4 border-b border-white/[0.06] bg-[var(--bg-panel)] px-6 py-4">
        <div>
          <h1 className="text-lg font-bold tracking-tight text-white">
            {t.settings.title}
          </h1>
          <p className="text-xs text-slate-400">{t.settings.subtitle}</p>
        </div>

        <div className="flex flex-wrap items-center gap-3">
          {/* Compact RU / EN pill */}
          <div className="inline-flex items-center rounded-xl bg-white/[0.04] p-0.5 ring-1 ring-white/[0.07]">
            {(["ru", "en"] as AppLanguage[]).map((code) => (
              <button
                key={code}
                type="button"
                onClick={() => handleSelectLanguage(code)}
                className={`rounded-lg px-2.5 py-1 text-[11px] font-bold uppercase transition ${
                  lang === code
                    ? "bg-[var(--accent)] text-white shadow-sm"
                    : "text-slate-400 hover:text-white"
                }`}
              >
                {code.toUpperCase()}
              </button>
            ))}
          </div>

          <span
            className={`inline-flex items-center gap-1.5 rounded-full px-3 py-1 text-[11px] font-semibold ring-1 transition ${
              saving
                ? "bg-white/[0.06] text-slate-300 ring-white/[0.1]"
                : "bg-emerald-500/10 text-emerald-300 ring-emerald-500/30"
            }`}
          >
            <CheckCircle2 className="h-3.5 w-3.5" />
            {saving
              ? t.settings.saving
              : justSaved
                ? t.settings.savedPill
                : t.settings.applied}
          </span>
        </div>
      </div>

      {/* Category tabs in exact required order */}
      <div className="flex flex-wrap gap-1.5 border-b border-white/[0.06] bg-[var(--bg-panel)] px-6 py-2.5">
        {(
          [
            ["capture", t.settings.tabCapture, Monitor],
            ["audio", t.settings.tabAudio, Mic],
            ["notifications", t.settings.tabNotifications, Bell],
            ["appearance", t.settings.tabAppearance, Palette],
            ["hotkeys_storage", t.settings.tabHotkeysStorage, HardDrive],
          ] as [SettingsTab, string, typeof Monitor][]
        ).map(([id, label, Icon]) => (
          <button
            key={id}
            type="button"
            onClick={() => setTab(id)}
            className={`inline-flex items-center gap-2 rounded-xl px-3.5 py-2 text-xs font-semibold transition ${
              tab === id
                ? "bg-[rgb(var(--accent-rgb)/0.18)] text-[var(--accent-hover)] ring-1 ring-[rgb(var(--accent-rgb)/0.4)]"
                : "text-slate-400 hover:bg-white/[0.05] hover:text-slate-200"
            }`}
          >
            <Icon className="h-3.5 w-3.5" />
            {label}
          </button>
        ))}
      </div>

      {/* Settings Content */}
      <div className="flex-1 overflow-y-auto p-6">
        <div className="mx-auto max-w-4xl space-y-6">
          {tab === "capture" && (() => {
            const resChoice =
              draft.video.resolution.preset === "p720"
                ? "p720"
                : draft.video.resolution.preset === "p1080"
                  ? "p1080"
                  : draft.video.resolution.preset === "custom" &&
                      draft.video.resolution.width === 2560 &&
                      draft.video.resolution.height === 1440
                    ? "p1440"
                    : "native";
            const qualityPreset =
              draft.video.bitrate_kbps === 3000
                ? "discord"
                : draft.video.bitrate_kbps === 12000
                  ? "optimal"
                  : draft.video.bitrate_kbps === 25000
                    ? "high"
                    : "custom";
            const fpsValue =
              draft.video.fps === 30 ||
              draft.video.fps === 60 ||
              draft.video.fps === 120
                ? String(draft.video.fps)
                : String(draft.video.fps);
            const fpsPillValues = Array.from(
              new Set([30, 60, 120, ...(fpsValue !== "30" && fpsValue !== "60" && fpsValue !== "120" ? [draft.video.fps] : [])])
            ).sort((a, b) => a - b);

            return (
            <>
              <section className={sectionCls}>
                <div className="flex flex-wrap items-center justify-between gap-3">
                  <div className="flex items-center gap-3">
                    <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-xl bg-[rgb(var(--accent-rgb)/0.14)] text-[var(--accent-hover)]">
                      <Monitor className="h-4 w-4" />
                    </div>
                    <div>
                      <h2 className="text-sm font-bold text-white">
                        {t.settings.videoSourceTitle}
                      </h2>
                      <p className="text-[11px] text-slate-400">
                        {t.settings.videoSourceDesc}
                      </p>
                    </div>
                  </div>
                  <button
                    type="button"
                    onClick={() => void onRefreshSources()}
                    className={actionChipBtn}
                  >
                    <RefreshCw className="h-3 w-3" />
                    {t.settings.refreshSources}
                  </button>
                </div>

                {/* Capture Source */}
                <div>
                  <div className={`${labelTitleCls} flex items-center gap-1.5`}>
                    <span>{t.settings.defaultCaptureSource}</span>
                    <InfoTooltip text={t.settings.tips.captureSource} />
                  </div>
                  <CustomSelect
                    value={draftTargetKey}
                    options={captureSourceSelectOptions}
                    onChange={(val) => {
                      const found = targetOptions.find((o) => o.key === val);
                      if (found) {
                        setDraft((p) => ({
                          ...p,
                          capture: { ...p.capture, target: found.target },
                        }));
                      }
                    }}
                    ariaLabel={t.settings.defaultCaptureSource}
                  />
                </div>

                {/* Resolution & FPS Segmented Pills */}
                <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
                  <div>
                    <div className={`${labelTitleCls} flex items-center gap-1.5`}>
                      <span>{t.settings.resolution}</span>
                      <InfoTooltip text={t.settings.tips.resolution} />
                    </div>
                    <SegmentedControl
                      value={resChoice}
                      options={[
                        { value: "p720", label: "720p" },
                        { value: "p1080", label: "1080p" },
                        { value: "p1440", label: "1440p" },
                        { value: "native", label: t.settings.resNativeShort },
                      ]}
                      onChange={(choice) => {
                        const nextRes: ResolutionPreset =
                          choice === "p720"
                            ? { preset: "p720" }
                            : choice === "p1080"
                              ? { preset: "p1080" }
                              : choice === "p1440"
                                ? { preset: "custom", width: 2560, height: 1440 }
                                : { preset: "native" };
                        setDraft((p) => ({
                          ...p,
                          video: { ...p.video, resolution: nextRes },
                        }));
                      }}
                    />
                  </div>

                  <div>
                    <div className={`${labelTitleCls} flex items-center gap-1.5`}>
                      <span>{t.settings.fps}</span>
                      <InfoTooltip text={t.settings.tips.fps} />
                    </div>
                    <SegmentedControl
                      value={fpsValue}
                      options={fpsPillValues.map((f) => ({
                        value: String(f),
                        label: `${f} FPS`,
                      }))}
                      onChange={(val) =>
                        setDraft((p) => ({
                          ...p,
                          video: { ...p.video, fps: Number(val) },
                        }))
                      }
                    />
                  </div>
                </div>

                {/* Video Codec & Clip Quality Presets */}
                <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
                  <div>
                    <div className={`${labelTitleCls} flex items-center gap-1.5`}>
                      <span>{t.settings.videoCodec}</span>
                      <InfoTooltip text={t.settings.tips.videoCodec} />
                    </div>
                    <SegmentedControl
                      value={draft.video.codec}
                      options={[
                        { value: "h264", label: "H.264" },
                        { value: "hevc", label: "HEVC" },
                        {
                          value: "av1",
                          label: "AV1",
                          warn: !av1Supported,
                          title: av1Supported
                            ? t.settings.codecAv1
                            : t.settings.codecAv1Unsupported,
                        },
                      ]}
                      onChange={(val) =>
                        setDraft((p) => ({
                          ...p,
                          video: {
                            ...p.video,
                            codec: val as VideoCodec,
                          },
                        }))
                      }
                    />
                    {draft.video.codec === "av1" && !av1Supported && (
                      <p className="mt-2 rounded-lg border border-amber-500/30 bg-amber-500/10 px-2.5 py-1.5 text-[11px] leading-snug text-amber-300">
                        {t.settings.codecAv1UnavailableWarning}
                      </p>
                    )}
                  </div>

                  <div>
                    <div className={`${labelTitleCls} flex items-center gap-1.5`}>
                      <span>{t.settings.qualityPresetLabel}</span>
                      <InfoTooltip text={t.settings.tips.rateControl} />
                    </div>
                    <SegmentedControl
                      value={qualityPreset}
                      options={[
                        { value: "discord", label: t.settings.qualityDiscord },
                        { value: "optimal", label: t.settings.qualityOptimal },
                        { value: "high", label: t.settings.qualityHigh },
                        { value: "custom", label: t.settings.qualityCustom },
                      ]}
                      onChange={(preset) => {
                        const kbps =
                          preset === "discord"
                            ? 3000
                            : preset === "optimal"
                              ? 12000
                              : preset === "high"
                                ? 25000
                                : draft.video.bitrate_kbps === 3000 ||
                                    draft.video.bitrate_kbps === 12000 ||
                                    draft.video.bitrate_kbps === 25000
                                  ? 18000
                                  : draft.video.bitrate_kbps;
                        setDraft((p) => ({
                          ...p,
                          video: { ...p.video, bitrate_kbps: kbps },
                        }));
                      }}
                    />
                  </div>
                </div>

                {/* Fine Bitrate Slider */}
                <AccentSlider
                  label={t.settings.bitrate}
                  valueBadge={`${(draft.video.bitrate_kbps / 1000).toFixed(0)} ${t.mbpsWord}`}
                  min={2000}
                  max={80000}
                  step={1000}
                  value={draft.video.bitrate_kbps}
                  onChange={(val) =>
                    setDraft((p) => ({
                      ...p,
                      video: { ...p.video, bitrate_kbps: val },
                    }))
                  }
                />

                <AdvancedAccordion
                  title={t.settings.advancedSettings}
                  hint={t.settings.advancedCaptureHint}
                >
                  <div className="grid grid-cols-1 gap-4 md:grid-cols-3">
                    <div>
                      <div className={`${labelTitleCls} flex items-center gap-1.5`}>
                        <span>{t.settings.hdrConversion}</span>
                        <InfoTooltip text={t.settings.tips.hdr} />
                      </div>
                      <CustomSelect
                        value={draft.capture.hdr_mode}
                        options={hdrOptions}
                        onChange={(val) =>
                          setDraft((p) => ({
                            ...p,
                            capture: {
                              ...p.capture,
                              hdr_mode: val as HdrMode,
                            },
                          }))
                        }
                        ariaLabel={t.settings.hdrConversion}
                      />
                    </div>

                    <div>
                      <div className={`${labelTitleCls} flex items-center gap-1.5`}>
                        <span>{t.settings.gpuEncoder}</span>
                        <InfoTooltip text={t.settings.tips.gpuEncoder} />
                      </div>
                      <CustomSelect
                        value={draft.video.gpu_vendor}
                        options={gpuOptions}
                        onChange={(val) =>
                          setDraft((p) => ({
                            ...p,
                            video: {
                              ...p.video,
                              gpu_vendor: val as GpuVendorPreference,
                            },
                          }))
                        }
                        ariaLabel={t.settings.gpuEncoder}
                      />
                    </div>

                    <div>
                      <div className={`${labelTitleCls} flex items-center gap-1.5`}>
                        <span>{t.settings.rateControl}</span>
                        <InfoTooltip text={t.settings.tips.rateControl} />
                      </div>
                      <CustomSelect
                        value={draft.video.rate_control}
                        options={rateControlOptions}
                        onChange={(val) =>
                          setDraft((p) => ({
                            ...p,
                            video: {
                              ...p.video,
                              rate_control: val as RateControlMode,
                            },
                          }))
                        }
                        ariaLabel={t.settings.rateControl}
                      />
                    </div>
                  </div>

                  <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
                    <ToggleSwitch
                      checked={draft.capture.capture_cursor}
                      onChange={(v) =>
                        setDraft((p) => ({
                          ...p,
                          capture: { ...p.capture, capture_cursor: v },
                        }))
                      }
                      label={t.settings.captureCursor}
                      tooltip={t.settings.tips.captureCursor}
                    />
                    <ToggleSwitch
                      checked={draft.capture.capture_border}
                      onChange={(v) =>
                        setDraft((p) => ({
                          ...p,
                          capture: { ...p.capture, capture_border: v },
                        }))
                      }
                      label={t.settings.captureBorder}
                      tooltip={t.settings.tips.captureBorder}
                    />
                  </div>
                </AdvancedAccordion>
              </section>

              <section className={sectionCls}>
                <div className="flex items-center gap-3">
                  <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-xl bg-[rgb(var(--accent-rgb)/0.14)] text-[var(--accent-hover)]">
                    <RefreshCw className="h-4 w-4" />
                  </div>
                  <div>
                    <div className="flex items-center gap-1.5">
                      <h2 className="text-sm font-bold text-white">
                        {t.settings.replayBufferTitle}
                      </h2>
                      <InfoTooltip text={t.settings.tips.replayDuration} />
                    </div>
                    <p className="text-[11px] text-slate-400">
                      {t.settings.replayBufferDesc}
                    </p>
                  </div>
                </div>

                <div className="space-y-3">
                  <SegmentedControl
                    value={String(draft.replay_buffer.duration_secs)}
                    options={[
                      { value: "15", label: `15${t.secShort}` },
                      { value: "30", label: `30${t.secShort}` },
                      { value: "60", label: `60${t.secShort}` },
                      { value: "120", label: `2${minShortUnit}` },
                      { value: "300", label: `5${minShortUnit}` },
                    ]}
                    onChange={(val) =>
                      setDraft((p) => ({
                        ...p,
                        replay_buffer: {
                          ...p.replay_buffer,
                          duration_secs: Number(val),
                        },
                      }))
                    }
                  />
                  <AccentSlider
                    label={t.settings.bufferDurationLabel}
                    valueBadge={`${draft.replay_buffer.duration_secs} ${t.secWord}`}
                    min={15}
                    max={300}
                    step={5}
                    value={draft.replay_buffer.duration_secs}
                    onChange={(val) =>
                      setDraft((p) => ({
                        ...p,
                        replay_buffer: {
                          ...p.replay_buffer,
                          duration_secs: val,
                        },
                      }))
                    }
                  />
                </div>

                <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
                  <ToggleSwitch
                    checked={draft.replay_buffer.enabled}
                    onChange={(v) =>
                      setDraft((p) => ({
                        ...p,
                        replay_buffer: { ...p.replay_buffer, enabled: v },
                      }))
                    }
                    label={t.settings.enableBufferingStartup}
                    tooltip={t.settings.tips.replayEnabled}
                  />
                  <ToggleSwitch
                    checked={draft.replay_buffer.reset_buffer_on_clip}
                    onChange={(v) =>
                      setDraft((p) => ({
                        ...p,
                        replay_buffer: {
                          ...p.replay_buffer,
                          reset_buffer_on_clip: v,
                        },
                      }))
                    }
                    label={t.settings.resetBufferOnClipLabel}
                    hint={t.settings.resetBufferOnClipHint}
                    tooltip={t.settings.tips.resetBufferOnClip}
                  />
                </div>

                <AdvancedAccordion
                  title={t.settings.advancedSettings}
                  hint={t.settings.bufferRamLabel}
                >
                  <AccentSlider
                    label={t.settings.bufferRamLabel}
                    valueBadge={`${draft.replay_buffer.max_ram_mb} ${t.mbWord}`}
                    min={128}
                    max={2048}
                    step={64}
                    value={draft.replay_buffer.max_ram_mb}
                    onChange={(val) =>
                      setDraft((p) => ({
                        ...p,
                        replay_buffer: {
                          ...p.replay_buffer,
                          max_ram_mb: val,
                        },
                      }))
                    }
                  />
                </AdvancedAccordion>
              </section>

              <section className={sectionCls}>
                <div className="flex items-center gap-3">
                  <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-xl bg-[rgb(var(--accent-rgb)/0.14)] text-[var(--accent-hover)]">
                    <Gamepad2 className="h-4 w-4" />
                  </div>
                  <div>
                    <h2 className="text-sm font-bold text-white">
                      {t.settings.gameDetectionTitle}
                    </h2>
                    <p className="text-[11px] text-slate-400">
                      {t.settings.gameDetectionDesc}
                    </p>
                  </div>
                </div>
                <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
                  <ToggleSwitch
                    checked={draft.capture.games.auto_start}
                    onChange={(v) =>
                      setDraft((p) => ({
                        ...p,
                        capture: {
                          ...p.capture,
                          games: { ...p.capture.games, auto_start: v },
                        },
                      }))
                    }
                    label={t.settings.autoStartGames}
                    tooltip={t.settings.tips.autoGame}
                  />
                  <ToggleSwitch
                    checked={draft.capture.games.use_builtin_game_list}
                    onChange={(v) =>
                      setDraft((p) => ({
                        ...p,
                        capture: {
                          ...p.capture,
                          games: {
                            ...p.capture.games,
                            use_builtin_game_list: v,
                          },
                        },
                      }))
                    }
                    label={t.settings.builtinGameList}
                    tooltip={t.settings.tips.builtinGames}
                  />
                </div>

                <AdvancedAccordion
                  title={t.settings.advancedSettings}
                  hint={`${t.settings.altTabDelay}, ${t.settings.whitelistTitle}`}
                >
                  <div className="max-w-sm">
                    <AccentSlider
                      label={t.settings.altTabDelay}
                      valueBadge={`${draft.capture.games.auto_pause_delay_secs} ${t.secWord}`}
                      min={0}
                      max={60}
                      step={1}
                      value={draft.capture.games.auto_pause_delay_secs}
                      onChange={(val) =>
                        setDraft((p) => ({
                          ...p,
                          capture: {
                            ...p.capture,
                            games: {
                              ...p.capture.games,
                              auto_pause_delay_secs: val,
                            },
                          },
                        }))
                      }
                    />
                  </div>

                  <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
                    <ExeListEditor
                      title={t.settings.whitelistTitle}
                      hint={t.settings.whitelistHint}
                      placeholder="valorant-win64-shipping.exe"
                      addLabel={t.settings.exeAddBtn}
                      emptyLabel={t.settings.exeListEmpty}
                      items={draft.capture.games.custom_whitelist_exes}
                      onChange={(next) =>
                        setDraft((p) => ({
                          ...p,
                          capture: {
                            ...p.capture,
                            games: {
                              ...p.capture.games,
                              custom_whitelist_exes: next,
                            },
                          },
                        }))
                      }
                    />
                    <ExeListEditor
                      title={t.settings.blacklistTitle}
                      hint={t.settings.blacklistHint}
                      placeholder="discord.exe"
                      addLabel={t.settings.exeAddBtn}
                      emptyLabel={t.settings.exeListEmpty}
                      items={draft.capture.games.blacklist_exes}
                      onChange={(next) =>
                        setDraft((p) => ({
                          ...p,
                          capture: {
                            ...p.capture,
                            games: { ...p.capture.games, blacklist_exes: next },
                          },
                        }))
                      }
                    />
                  </div>
                </AdvancedAccordion>
              </section>
            </>
            );
          })()}

          {tab === "audio" && (
            <>
              <section className={sectionCls}>
                <div className="flex items-center justify-between">
                  <h2 className="text-sm font-bold text-white">
                    {t.settings.audioDevicesTitle}
                  </h2>
                  <button
                    type="button"
                    onClick={() => void loadAudioDevices()}
                    className={actionChipBtn}
                  >
                    <RefreshCw className="h-3 w-3" />
                    {t.settings.refreshDevices}
                  </button>
                </div>

                <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
                  <div>
                    <div className={`${labelTitleCls} flex items-center gap-1.5`}>
                      <span>{t.settings.gameAudioLabel}</span>
                      <InfoTooltip text={t.settings.tips.gameAudio} />
                    </div>
                    <CustomSelect
                      value={
                        draft.audio.audio_output_mode.mode === "specific_device"
                          ? `spec:${draft.audio.audio_output_mode.device_id}`
                          : draft.audio.audio_output_mode.mode
                      }
                      options={gameAudioOptions}
                      onChange={(val) => {
                        const nextMode =
                          val === "default_device"
                            ? ({ mode: "default_device" } as const)
                            : val === "game_process_only"
                              ? ({ mode: "game_process_only" } as const)
                              : ({
                                  mode: "specific_device",
                                  device_id: val.slice(5),
                                } as const);
                        setDraft((p) => ({
                          ...p,
                          audio: { ...p.audio, audio_output_mode: nextMode },
                        }));
                      }}
                      ariaLabel={t.settings.gameAudioLabel}
                    />
                  </div>

                  <div>
                    <div className={`${labelTitleCls} flex items-center gap-1.5`}>
                      <span>{t.settings.secondaryAudioLabel}</span>
                      <InfoTooltip text={t.settings.tips.secondaryAudio} />
                    </div>
                    <CustomSelect
                      value={draft.audio.secondary_output_device ?? ""}
                      options={secondaryAudioOptions}
                      onChange={(val) => {
                        const v = val || null;
                        setDraft((p) => ({
                          ...p,
                          audio: { ...p.audio, secondary_output_device: v },
                        }));
                      }}
                      ariaLabel={t.settings.secondaryAudioLabel}
                    />
                  </div>

                  <div>
                    <div className={`${labelTitleCls} flex items-center gap-1.5`}>
                      <span>{t.settings.micLabel}</span>
                      <InfoTooltip text={t.settings.tips.micDevice} />
                    </div>
                    <CustomSelect
                      value={
                        draft.audio.mic_device.mode === "specific_device"
                          ? `spec:${draft.audio.mic_device.device_id}`
                          : draft.audio.mic_device.mode
                      }
                      options={micDeviceOptions}
                      onChange={(val) => {
                        const nextMic =
                          val === "none"
                            ? ({ mode: "none" } as const)
                            : val === "default"
                              ? ({ mode: "default" } as const)
                              : ({
                                  mode: "specific_device",
                                  device_id: val.slice(5),
                                } as const);
                        setDraft((p) => ({
                          ...p,
                          audio: {
                            ...p.audio,
                            mic_device: nextMic,
                            mic_enabled: nextMic.mode !== "none",
                          },
                        }));
                      }}
                      ariaLabel={t.settings.micLabel}
                    />
                  </div>
                </div>

                {/* Live Microphone Test & Level Meter */}
                <div className="flex flex-wrap items-center justify-between gap-4 rounded-xl bg-white/[0.025] p-3.5 ring-1 ring-white/[0.06]">
                  <div className="flex items-center gap-2.5">
                    <Activity
                      className={`h-4 w-4 ${
                        micTesting ? "text-emerald-400" : "text-slate-400"
                      }`}
                    />
                    <div>
                      <div className="text-xs font-semibold text-white">
                        {t.settings.micTesterTitle}
                      </div>
                      <div className="text-[11px] text-slate-400">
                        {t.settings.micTesterDesc}
                      </div>
                    </div>
                  </div>
                  <div className="flex max-w-xs flex-1 items-center gap-3">
                    <div className="h-2 flex-1 overflow-hidden rounded-full bg-white/[0.08]">
                      <div
                        className="h-full rounded-full bg-gradient-to-r from-emerald-500 via-lime-400 to-amber-400 transition-all duration-75"
                        style={{ width: `${micLevelPct}%` }}
                      />
                    </div>
                    <button
                      type="button"
                      onClick={() => void toggleMicTester()}
                      className={`rounded-xl px-3 py-1.5 text-xs font-semibold transition ${
                        micTesting
                          ? "bg-rose-600 text-white hover:bg-rose-500"
                          : "bg-[var(--accent)] text-white hover:bg-[var(--accent-hover)]"
                      }`}
                    >
                      {micTesting
                        ? t.settings.micTestStop
                        : t.settings.micTestStart}
                    </button>
                  </div>
                </div>

                <div className="grid grid-cols-1 gap-5 border-t border-white/[0.06] pt-4 md:grid-cols-3">
                  <AccentSlider
                    label={t.settings.gameVolume}
                    valueBadge={`${Math.round(draft.audio.game_volume * 100)}%`}
                    min={0}
                    max={5}
                    step={0.05}
                    value={draft.audio.game_volume}
                    onChange={(v) => {
                      setDraft((p) => ({
                        ...p,
                        audio: { ...p.audio, game_volume: v },
                      }));
                      applyLiveVolumes(
                        v,
                        draft.audio.mic_volume,
                        draft.audio.secondary_volume
                      );
                    }}
                  />

                  <AccentSlider
                    label={t.settings.micVolume}
                    valueBadge={`${Math.round(draft.audio.mic_volume * 100)}%`}
                    min={0}
                    max={5}
                    step={0.05}
                    value={draft.audio.mic_volume}
                    onChange={(v) => {
                      setDraft((p) => ({
                        ...p,
                        audio: { ...p.audio, mic_volume: v },
                      }));
                      applyLiveVolumes(
                        draft.audio.game_volume,
                        v,
                        draft.audio.secondary_volume
                      );
                    }}
                  />

                  <AccentSlider
                    label={t.settings.secondaryVolume}
                    valueBadge={`${Math.round(
                      draft.audio.secondary_volume * 100
                    )}%`}
                    min={0}
                    max={5}
                    step={0.05}
                    value={draft.audio.secondary_volume}
                    onChange={(v) => {
                      setDraft((p) => ({
                        ...p,
                        audio: { ...p.audio, secondary_volume: v },
                      }));
                      applyLiveVolumes(
                        draft.audio.game_volume,
                        draft.audio.mic_volume,
                        v
                      );
                    }}
                  />
                </div>

              </section>

              <section className={sectionCls}>
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <div className="flex items-center gap-1.5">
                    <h2 className="text-sm font-bold text-white">
                      {t.settings.appAudioTitle}
                    </h2>
                    <InfoTooltip text={t.settings.tips.appAudio} />
                  </div>
                  <div className="flex flex-wrap items-center gap-2">
                    <button
                      type="button"
                      onClick={() => void loadActiveAudioApps()}
                      disabled={loadingAudioApps}
                      className={actionChipBtn}
                    >
                      <RefreshCw
                        className={`h-3 w-3 ${loadingAudioApps ? "animate-spin" : ""}`}
                      />
                      {t.settings.appAudioPickRunning}
                    </button>
                    <button
                      type="button"
                      onClick={() => {
                        const nextSlot: AppAudioSlotConfig = {
                          id: `app-${Date.now()}`,
                          label: "",
                          process_names: [],
                          volume: 1.0,
                          muted: false,
                          enabled: true,
                        };
                        setDraft((p) => ({
                          ...p,
                          audio: {
                            ...p.audio,
                            app_audio_slots: [
                              ...(p.audio.app_audio_slots ?? []),
                              nextSlot,
                            ],
                          },
                        }));
                      }}
                      className={actionChipBtn}
                    >
                      <Plus className="h-3 w-3" />
                      {t.settings.appAudioAddChannel}
                    </button>
                  </div>
                </div>

                {appPickerOpen && (
                  <div className="rounded-xl bg-white/[0.03] p-3 ring-1 ring-white/[0.08]">
                    <div className="mb-2 flex items-center justify-between">
                      <span className="text-xs font-semibold text-slate-300">
                        {t.settings.appAudioPickRunning}
                      </span>
                      <button
                        type="button"
                        onClick={() => setAppPickerOpen(false)}
                        className="rounded-lg p-1 text-slate-400 hover:bg-white/[0.06] hover:text-white"
                      >
                        <X className="h-3.5 w-3.5" />
                      </button>
                    </div>
                    {activeAudioApps.length === 0 ? (
                      <div className="text-xs text-slate-400">
                        {t.settings.appAudioNoRunning}
                      </div>
                    ) : (
                      <div className="flex flex-wrap gap-1.5">
                        {activeAudioApps.map((app) => (
                          <button
                            key={`${app.pid}-${app.exe_name}`}
                            type="button"
                            onClick={() => {
                              const cleanLabel = app.exe_name.replace(
                                /\.exe$/i,
                                ""
                              );
                              const nextSlot: AppAudioSlotConfig = {
                                id: `app-${Date.now()}-${app.pid}`,
                                label: cleanLabel,
                                process_names: [app.exe_name],
                                volume: 1.0,
                                muted: false,
                                enabled: true,
                              };
                              setDraft((p) => ({
                                ...p,
                                audio: {
                                  ...p.audio,
                                  app_audio_slots: [
                                    ...(p.audio.app_audio_slots ?? []),
                                    nextSlot,
                                  ],
                                },
                              }));
                              setAppPickerOpen(false);
                            }}
                            className="flex items-center gap-1.5 rounded-lg bg-white/[0.05] px-2.5 py-1.5 text-xs text-slate-200 ring-1 ring-white/[0.08] transition hover:bg-white/[0.1]"
                          >
                            <span
                              className={`h-1.5 w-1.5 rounded-full ${
                                app.peak > 0.0001
                                  ? "bg-emerald-400"
                                  : "bg-slate-400"
                              }`}
                            />
                            <span>{app.exe_name}</span>
                            <span className="text-[10px] text-slate-400">
                              PID {app.pid}
                            </span>
                          </button>
                        ))}
                      </div>
                    )}
                  </div>
                )}

                {(draft.audio.app_audio_slots ?? []).length === 0 ? (
                  <div className="rounded-xl bg-white/[0.02] p-3.5 text-xs text-slate-400 ring-1 ring-white/[0.05]">
                    {t.settings.appAudioEmpty}
                  </div>
                ) : (
                  <div className="space-y-3">
                    {(draft.audio.app_audio_slots ?? []).map((slot, idx) => (
                      <div
                        key={slot.id || idx}
                        className="rounded-xl bg-white/[0.025] p-3.5 ring-1 ring-white/[0.06]"
                      >
                        <div className="grid grid-cols-1 gap-3 md:grid-cols-12 md:items-center">
                          <div className="md:col-span-3">
                            <input
                              type="text"
                              value={slot.label}
                              placeholder={t.settings.appAudioSlotNamePlaceholder}
                              onChange={(e) => {
                                const val = e.target.value;
                                setDraft((p) => ({
                                  ...p,
                                  audio: {
                                    ...p.audio,
                                    app_audio_slots: (
                                      p.audio.app_audio_slots ?? []
                                    ).map((s, i) =>
                                      i === idx ? { ...s, label: val } : s
                                    ),
                                  },
                                }));
                              }}
                              className="w-full rounded-xl bg-black/35 px-3 py-1.5 text-xs text-white placeholder-slate-500 ring-1 ring-white/[0.08] outline-none focus:ring-[var(--accent)]"
                            />
                          </div>
                          <div className="md:col-span-4">
                            <input
                              type="text"
                              value={slot.process_names.join(", ")}
                              placeholder={
                                t.settings.appAudioProcessesPlaceholder
                              }
                              onChange={(e) => {
                                const procs = e.target.value
                                  .split(",")
                                  .map((s) => s.trim())
                                  .filter(Boolean);
                                setDraft((p) => ({
                                  ...p,
                                  audio: {
                                    ...p.audio,
                                    app_audio_slots: (
                                      p.audio.app_audio_slots ?? []
                                    ).map((s, i) =>
                                      i === idx
                                        ? { ...s, process_names: procs }
                                        : s
                                    ),
                                  },
                                }));
                              }}
                              className="w-full rounded-xl bg-black/35 px-3 py-1.5 text-xs text-white placeholder-slate-500 ring-1 ring-white/[0.08] outline-none focus:ring-[var(--accent)]"
                            />
                          </div>
                          <div className="md:col-span-3">
                            <AccentSlider
                              label=""
                              valueBadge={`${Math.round(slot.volume * 100)}%`}
                              min={0}
                              max={5}
                              step={0.05}
                              value={slot.volume}
                              onChange={(v) => {
                                setDraft((p) => ({
                                  ...p,
                                  audio: {
                                    ...p.audio,
                                    app_audio_slots: (
                                      p.audio.app_audio_slots ?? []
                                    ).map((s, i) =>
                                      i === idx ? { ...s, volume: v } : s
                                    ),
                                  },
                                }));
                              }}
                            />
                          </div>
                          <div className="flex items-center justify-end gap-2 md:col-span-2">
                            <button
                              type="button"
                              onClick={() => {
                                setDraft((p) => ({
                                  ...p,
                                  audio: {
                                    ...p.audio,
                                    app_audio_slots: (
                                      p.audio.app_audio_slots ?? []
                                    ).map((s, i) =>
                                      i === idx ? { ...s, muted: !s.muted } : s
                                    ),
                                  },
                                }));
                              }}
                              className={`rounded-lg p-1.5 transition ${
                                slot.muted
                                  ? "bg-rose-500/20 text-rose-400"
                                  : "bg-white/[0.05] text-slate-300 hover:text-white"
                              }`}
                            >
                              {slot.muted ? (
                                <VolumeX className="h-3.5 w-3.5" />
                              ) : (
                                <Volume2 className="h-3.5 w-3.5" />
                              )}
                            </button>
                            <button
                              type="button"
                              onClick={() => {
                                setDraft((p) => ({
                                  ...p,
                                  audio: {
                                    ...p.audio,
                                    app_audio_slots: (
                                      p.audio.app_audio_slots ?? []
                                    ).map((s, i) =>
                                      i === idx
                                        ? { ...s, enabled: !s.enabled }
                                        : s
                                    ),
                                  },
                                }));
                              }}
                              className={`rounded-lg px-2 py-1 text-[11px] font-semibold transition ${
                                slot.enabled
                                  ? "bg-emerald-500/20 text-emerald-300"
                                  : "bg-white/[0.05] text-slate-400"
                              }`}
                            >
                              {slot.enabled ? "ON" : "OFF"}
                            </button>
                            <button
                              type="button"
                              title={t.settings.appAudioRemove}
                              onClick={() => {
                                setDraft((p) => ({
                                  ...p,
                                  audio: {
                                    ...p.audio,
                                    app_audio_slots: (
                                      p.audio.app_audio_slots ?? []
                                    ).filter((_, i) => i !== idx),
                                  },
                                }));
                              }}
                              className="rounded-lg p-1.5 text-slate-400 transition hover:bg-rose-500/20 hover:text-rose-400"
                            >
                              <Trash2 className="h-3.5 w-3.5" />
                            </button>
                          </div>
                        </div>
                      </div>
                    ))}
                  </div>
                )}
              </section>

              <section className={sectionCls}>
                <div className="flex items-center gap-3">
                  <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-xl bg-[rgb(var(--accent-rgb)/0.14)] text-[var(--accent-hover)]">
                    <Mic className="h-4 w-4" />
                  </div>
                  <div>
                    <h2 className="text-sm font-bold text-white">
                      {t.settings.micProcessingTitle}
                    </h2>
                    <p className="text-[11px] text-slate-400">
                      {t.settings.micProcessingDesc}
                    </p>
                  </div>
                </div>

                <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
                  <ToggleSwitch
                    checked={draft.audio.mic_noise_suppression}
                    onChange={(en) => {
                      setDraft((p) => ({
                        ...p,
                        audio: { ...p.audio, mic_noise_suppression: en },
                      }));
                      applyLiveRnnoise(
                        en,
                        draft.audio.mic_noise_suppression_strength,
                        draft.audio.mic_noise_gate_db
                      );
                    }}
                    label={t.settings.rnnoiseLabel}
                    hint={t.settings.rnnoiseHint}
                    tooltip={t.settings.tips.rnnoise}
                  />

                  <div className="flex flex-col justify-center">
                    <AccentSlider
                      label={t.settings.rnnoiseStrength}
                      valueBadge={`${Math.round(
                        draft.audio.mic_noise_suppression_strength * 100
                      )}%`}
                      min={0}
                      max={1}
                      step={0.05}
                      disabled={!draft.audio.mic_noise_suppression}
                      value={draft.audio.mic_noise_suppression_strength}
                      onChange={(val) => {
                        setDraft((p) => ({
                          ...p,
                          audio: {
                            ...p.audio,
                            mic_noise_suppression_strength: val,
                          },
                        }));
                        applyLiveRnnoise(
                          draft.audio.mic_noise_suppression,
                          val,
                          draft.audio.mic_noise_gate_db
                        );
                      }}
                    />
                  </div>
                </div>

                <div className="border-t border-white/[0.06] pt-4">
                  <div className="grid grid-cols-1 gap-4 md:grid-cols-3">
                    <div>
                      <div className={`${labelTitleCls} flex items-center gap-1.5`}>
                        <span>{t.settings.micActivationMode}</span>
                        <InfoTooltip text={t.settings.tips.micActivation} />
                      </div>
                      <CustomSelect
                        value={draft.audio.mic_activation_mode.mode}
                        options={micModeOptions}
                        onChange={(mode) => {
                          const prevKey =
                            draft.audio.mic_activation_mode.mode === "always_on"
                              ? "V"
                              : draft.audio.mic_activation_mode.key;
                          const prevDelay =
                            draft.audio.mic_activation_mode.mode === "always_on"
                              ? 150
                              : draft.audio.mic_activation_mode.release_delay_ms;
                          setDraft((p) => ({
                            ...p,
                            audio: {
                              ...p.audio,
                              mic_activation_mode:
                                mode === "always_on"
                                  ? { mode: "always_on" }
                                  : mode === "push_to_talk"
                                    ? {
                                        mode: "push_to_talk",
                                        key: prevKey,
                                        release_delay_ms: prevDelay,
                                      }
                                    : {
                                        mode: "push_to_mute",
                                        key: prevKey,
                                        release_delay_ms: prevDelay,
                                      },
                            },
                          }));
                        }}
                        ariaLabel={t.settings.micActivationMode}
                      />
                    </div>

                    {draft.audio.mic_activation_mode.mode !== "always_on" && (
                      <>
                        <label className="block text-xs text-slate-300">
                          {t.settings.pttKeyLabel}
                          <input
                            type="text"
                            value={draft.audio.mic_activation_mode.key}
                            onKeyDown={(e) => {
                              const combo = keyEventToComboString(e);
                              if (!combo) return;
                              e.preventDefault();
                              setDraft((p) => {
                                if (
                                  p.audio.mic_activation_mode.mode ===
                                  "always_on"
                                )
                                  return p;
                                return {
                                  ...p,
                                  audio: {
                                    ...p.audio,
                                    mic_activation_mode: {
                                      ...p.audio.mic_activation_mode,
                                      key: combo,
                                    },
                                  },
                                };
                              });
                            }}
                            onChange={(e) => {
                              const v = e.target.value;
                              setDraft((p) => {
                                if (
                                  p.audio.mic_activation_mode.mode ===
                                  "always_on"
                                )
                                  return p;
                                return {
                                  ...p,
                                  audio: {
                                    ...p.audio,
                                    mic_activation_mode: {
                                      ...p.audio.mic_activation_mode,
                                      key: v,
                                    },
                                  },
                                };
                              });
                            }}
                            className={`${fieldCls} font-mono`}
                          />
                        </label>

                        <div className="flex flex-col justify-center">
                          <AccentSlider
                            label={t.settings.releaseDelayLabel}
                            valueBadge={`${draft.audio.mic_activation_mode.release_delay_ms} ${msUnit}`}
                            min={0}
                            max={1000}
                            step={25}
                            value={
                              draft.audio.mic_activation_mode.release_delay_ms
                            }
                            onChange={(ms) => {
                              setDraft((p) => {
                                if (
                                  p.audio.mic_activation_mode.mode ===
                                  "always_on"
                                )
                                  return p;
                                return {
                                  ...p,
                                  audio: {
                                    ...p.audio,
                                    mic_activation_mode: {
                                      ...p.audio.mic_activation_mode,
                                      release_delay_ms: ms,
                                    },
                                  },
                                };
                              });
                            }}
                          />
                        </div>
                      </>
                    )}
                  </div>
                </div>

                <AdvancedAccordion
                  title={t.settings.advancedSettings}
                  hint={t.settings.advancedAudioHint}
                >
                  <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
                    <ToggleSwitch
                      checked={draft.audio.separate_tracks}
                      onChange={(v) =>
                        setDraft((p) => ({
                          ...p,
                          audio: { ...p.audio, separate_tracks: v },
                        }))
                      }
                      label={t.settings.separateTracks}
                      tooltip={t.settings.tips.separateTracks}
                    />
                    <ToggleSwitch
                      checked={draft.audio.apply_volumes_to_editor ?? true}
                      onChange={(v) =>
                        setDraft((p) => ({
                          ...p,
                          audio: { ...p.audio, apply_volumes_to_editor: v },
                        }))
                      }
                      label={t.settings.applyVolumesToEditor}
                      tooltip={t.settings.tips.applyVolumesToEditor}
                    />
                  </div>

                  <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
                    <ToggleSwitch
                      checked={draft.audio.mic_soft_limiter}
                      onChange={(v) =>
                        setDraft((p) => ({
                          ...p,
                          audio: { ...p.audio, mic_soft_limiter: v },
                        }))
                      }
                      label={t.settings.softLimiterLabel}
                      hint={t.settings.softLimiterHint}
                    />

                    <div className="space-y-2.5">
                      <ToggleSwitch
                        checked={draft.audio.mic_noise_gate_db !== null}
                        onChange={(en) => {
                          const gate = en ? -50 : null;
                          setDraft((p) => ({
                            ...p,
                            audio: { ...p.audio, mic_noise_gate_db: gate },
                          }));
                          applyLiveRnnoise(
                            draft.audio.mic_noise_suppression,
                            draft.audio.mic_noise_suppression_strength,
                            gate
                          );
                        }}
                        label={t.settings.noiseGateTitle(
                          draft.audio.mic_noise_gate_db !== null
                            ? `${draft.audio.mic_noise_gate_db} dBFS`
                            : t.settings.disabledOption
                        )}
                        tooltip={t.settings.tips.noiseGate}
                      />
                      {draft.audio.mic_noise_gate_db !== null && (
                        <AccentSlider
                          valueBadge={`${draft.audio.mic_noise_gate_db} dBFS`}
                          min={-80}
                          max={-20}
                          step={1}
                          value={draft.audio.mic_noise_gate_db}
                          onChange={(g) => {
                            setDraft((p) => ({
                              ...p,
                              audio: { ...p.audio, mic_noise_gate_db: g },
                            }));
                            applyLiveRnnoise(
                              draft.audio.mic_noise_suppression,
                              draft.audio.mic_noise_suppression_strength,
                              g
                            );
                          }}
                        />
                      )}
                    </div>
                  </div>
                </AdvancedAccordion>
              </section>
            </>
          )}

          {tab === "notifications" && (
            <>
              <section className={sectionCls}>
                <h2 className="text-sm font-bold text-white">
                  {t.settings.overlayAndSoundTitle}
                </h2>
                <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
                  <ToggleSwitch
                    checked={draft.notifications.overlay_toast_enabled}
                    onChange={(v) =>
                      setDraft((p) => ({
                        ...p,
                        notifications: {
                          ...p.notifications,
                          overlay_toast_enabled: v,
                        },
                      }))
                    }
                    label={t.settings.toastOverlayLabel}
                    hint={t.settings.toastOverlayHint}
                    tooltip={t.settings.tips.toastOverlay}
                  />

                  <ToggleSwitch
                    checked={draft.notifications.sound_enabled}
                    onChange={(v) =>
                      setDraft((p) => ({
                        ...p,
                        notifications: {
                          ...p.notifications,
                          sound_enabled: v,
                        },
                      }))
                    }
                    label={t.settings.rodioSoundLabel}
                    hint={t.settings.rodioSoundHint}
                    tooltip={t.settings.tips.soundMaster}
                  />
                </div>

                <div className="grid grid-cols-1 gap-5 md:grid-cols-2">
                  <AccentSlider
                    label={t.settings.masterNotifVolume}
                    valueBadge={`${Math.round(
                      draft.notifications.sound_volume * 100
                    )}%`}
                    min={0}
                    max={1}
                    step={0.05}
                    value={draft.notifications.sound_volume}
                    onChange={(val) =>
                      setDraft((p) => ({
                        ...p,
                        notifications: {
                          ...p.notifications,
                          sound_volume: val,
                        },
                      }))
                    }
                  />

                  <AccentSlider
                    label={t.settings.toastDurationLabel}
                    valueBadge={`${(
                      draft.notifications.overlay_duration_ms / 1000
                    ).toFixed(1)} ${t.secWord}`}
                    min={1000}
                    max={8000}
                    step={250}
                    value={draft.notifications.overlay_duration_ms}
                    onChange={(val) =>
                      setDraft((p) => ({
                        ...p,
                        notifications: {
                          ...p.notifications,
                          overlay_duration_ms: val,
                        },
                      }))
                    }
                  />
                </div>
              </section>

              <section className={sectionCls}>
                <div className="flex items-center gap-1.5">
                  <h2 className="text-sm font-bold text-white">
                    {t.settings.eventSoundsTitle}
                  </h2>
                  <InfoTooltip text={t.settings.tips.soundEvents} />
                </div>
                <div className="space-y-2.5">
                  {(
                    [
                      ["clip_saved", t.settings.evClipSaved],
                      ["recording_started", t.settings.evRecStarted],
                      ["recording_stopped", t.settings.evRecStopped],
                      ["marker_added", t.settings.evMarkerAdded],
                    ] as [SoundNotificationKind, string][]
                  ).map(([kind, label]) => {
                    const entry = draft.notifications.sounds[kind];
                    const presetOpts = soundPresetOptionsFor(kind);
                    const normalizedPreset =
                      entry.source.kind === "preset"
                        ? normalizePresetValueForKind(kind, entry.source.value)
                        : defaultPresetForKind(kind);
                    return (
                      <div
                        key={kind}
                        className="rounded-xl bg-white/[0.025] p-3 ring-1 ring-white/[0.06]"
                      >
                        <div className="flex flex-wrap items-center justify-between gap-3">
                          <span className="text-xs font-semibold text-white">
                            {label}
                          </span>
                          <div className="flex flex-wrap items-center gap-2.5">
                            <CustomSelect
                              value={entry.source.kind}
                              options={soundKindOptions}
                              onChange={(k) =>
                                updateSoundEntry(kind, (prev) => ({
                                  ...prev,
                                  source:
                                    k === "preset"
                                      ? {
                                          kind: "preset",
                                          value: defaultPresetForKind(kind),
                                        }
                                      : k === "custom_file"
                                        ? { kind: "custom_file", value: "" }
                                        : { kind: "silent" },
                                }))
                              }
                              className="w-44"
                              menuMinWidth={175}
                              ariaLabel={label}
                            />

                            {entry.source.kind === "preset" && (
                              <CustomSelect
                                value={normalizedPreset}
                                options={presetOpts}
                                onChange={(val) =>
                                  updateSoundEntry(kind, (prev) => ({
                                    ...prev,
                                    source: { kind: "preset", value: val },
                                  }))
                                }
                                className="w-56"
                                menuMinWidth={220}
                                ariaLabel={label}
                              />
                            )}

                            {entry.source.kind === "custom_file" && (
                              <div className="flex items-center gap-1.5">
                                <input
                                  type="text"
                                  value={entry.source.value}
                                  onChange={(e) => {
                                    const val = e.target.value;
                                    updateSoundEntry(kind, (prev) => ({
                                      ...prev,
                                      source: {
                                        kind: "custom_file",
                                        value: val,
                                      },
                                    }));
                                  }}
                                  placeholder="C:\Sounds\notify.wav"
                                  className="w-44 rounded-xl bg-black/35 px-2.5 py-1.5 font-mono text-xs text-white ring-1 ring-white/[0.08] outline-none focus:ring-[var(--accent)]"
                                />
                                <button
                                  type="button"
                                  onClick={() => {
                                    void api
                                      .pickAudioFile()
                                      .then((picked) => {
                                        if (picked) {
                                          updateSoundEntry(kind, (prev) => ({
                                            ...prev,
                                            source: {
                                              kind: "custom_file",
                                              value: picked,
                                            },
                                          }));
                                        }
                                      })
                                      .catch(() => {});
                                  }}
                                  className={actionChipBtn}
                                >
                                  <FolderOpen className="h-3.5 w-3.5" />
                                  {t.settings.browseSoundFileBtn}
                                </button>
                              </div>
                            )}

                            <AccentSlider
                              className="w-32"
                              min={0}
                              max={1}
                              step={0.05}
                              value={entry.volume}
                              valueBadge={`${Math.round(entry.volume * 100)}%`}
                              onChange={(vol) =>
                                updateSoundEntry(kind, (prev) => ({
                                  ...prev,
                                  volume: vol,
                                }))
                              }
                            />

                            <button
                              type="button"
                              onClick={() => void handleTestSound(kind)}
                              className="inline-flex items-center gap-1 rounded-xl bg-[rgb(var(--accent-rgb)/0.2)] px-3 py-1.5 text-xs font-semibold text-[var(--accent-hover)] transition hover:bg-[var(--accent)] hover:text-white"
                            >
                              <Volume2 className="h-3.5 w-3.5" />
                              {t.settings.testSoundBtn}
                            </button>
                          </div>
                        </div>
                      </div>
                    );
                  })}
                </div>
              </section>
            </>
          )}

          {tab === "appearance" && (
            <>
              {/* Interface Language Switcher */}
              <section className={sectionCls}>
                <div className="flex flex-wrap items-center justify-between gap-4">
                  <div className="flex items-start gap-2.5">
                    <Globe className="mt-0.5 h-4 w-4 text-[var(--accent-hover)]" />
                    <div>
                      <h2 className="text-sm font-bold text-white">
                        {t.settings.languageTitle}
                      </h2>
                      <p className="mt-0.5 text-xs text-slate-400">
                        {t.settings.languageHint}
                      </p>
                    </div>
                  </div>

                  <div className="flex items-center gap-2">
                    {(
                      [
                        ["ru", t.settings.languageRu],
                        ["en", t.settings.languageEn],
                      ] as [AppLanguage, string][]
                    ).map(([code, label]) => {
                      const active = lang === code;
                      return (
                        <button
                          key={code}
                          type="button"
                          onClick={() => handleSelectLanguage(code)}
                          className={`inline-flex items-center gap-2 rounded-xl px-4 py-2 text-xs font-semibold transition ${
                            active
                              ? "bg-[var(--accent)] text-white shadow"
                              : "bg-white/[0.05] text-slate-300 ring-1 ring-white/[0.08] hover:bg-white/[0.1] hover:text-white"
                          }`}
                        >
                          {active && <Check className="h-3.5 w-3.5" />}
                          {label}
                        </button>
                      );
                    })}
                  </div>
                </div>

                <AdvancedAccordion
                  title={t.settings.advancedSettings}
                  hint={t.settings.advancedAudioHint}
                >
                  <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
                    <ToggleSwitch
                      checked={draft.audio.separate_tracks}
                      onChange={(v) =>
                        setDraft((p) => ({
                          ...p,
                          audio: { ...p.audio, separate_tracks: v },
                        }))
                      }
                      label={t.settings.separateTracks}
                      tooltip={t.settings.tips.separateTracks}
                    />
                    <ToggleSwitch
                      checked={draft.audio.apply_volumes_to_editor ?? true}
                      onChange={(v) =>
                        setDraft((p) => ({
                          ...p,
                          audio: { ...p.audio, apply_volumes_to_editor: v },
                        }))
                      }
                      label={t.settings.applyVolumesToEditor}
                      tooltip={t.settings.tips.applyVolumesToEditor}
                    />
                  </div>

                  <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
                    <ToggleSwitch
                      checked={draft.audio.mic_soft_limiter}
                      onChange={(v) =>
                        setDraft((p) => ({
                          ...p,
                          audio: { ...p.audio, mic_soft_limiter: v },
                        }))
                      }
                      label={t.settings.softLimiterLabel}
                      hint={t.settings.softLimiterHint}
                    />

                    <div className="space-y-2.5">
                      <ToggleSwitch
                        checked={draft.audio.mic_noise_gate_db !== null}
                        onChange={(en) => {
                          const gate = en ? -50 : null;
                          setDraft((p) => ({
                            ...p,
                            audio: { ...p.audio, mic_noise_gate_db: gate },
                          }));
                          applyLiveRnnoise(
                            draft.audio.mic_noise_suppression,
                            draft.audio.mic_noise_suppression_strength,
                            gate
                          );
                        }}
                        label={t.settings.noiseGateTitle(
                          draft.audio.mic_noise_gate_db !== null
                            ? `${draft.audio.mic_noise_gate_db} dBFS`
                            : t.settings.disabledOption
                        )}
                        tooltip={t.settings.tips.noiseGate}
                      />
                      {draft.audio.mic_noise_gate_db !== null && (
                        <AccentSlider
                          valueBadge={`${draft.audio.mic_noise_gate_db} dBFS`}
                          min={-80}
                          max={-20}
                          step={1}
                          value={draft.audio.mic_noise_gate_db}
                          onChange={(g) => {
                            setDraft((p) => ({
                              ...p,
                              audio: { ...p.audio, mic_noise_gate_db: g },
                            }));
                            applyLiveRnnoise(
                              draft.audio.mic_noise_suppression,
                              draft.audio.mic_noise_suppression_strength,
                              g
                            );
                          }}
                        />
                      )}
                    </div>
                  </div>
                </AdvancedAccordion>
              </section>

              {/* 8 Theme Presets + Custom Theme Gallery & Builder */}
              <section className={sectionCls}>
                <div>
                  <h2 className="text-sm font-bold text-white">
                    {t.settings.themeTitle}
                  </h2>
                  <p className="mt-0.5 text-xs text-slate-400">
                    {t.settings.themeSubtitle}
                  </p>
                </div>

                <div className="grid grid-cols-1 gap-3.5 sm:grid-cols-2 lg:grid-cols-3">
                  {THEME_PRESETS.map((preset) => {
                    const selected = draft.theme === preset.id;
                    return (
                      <button
                        key={preset.id}
                        type="button"
                        onClick={() => {
                          setDraft((p) => ({ ...p, theme: preset.id }));
                          onPreviewCustomTheme(null);
                          onPreviewTheme(preset.id);
                        }}
                        className={`group flex flex-col gap-2.5 rounded-2xl p-3 text-left transition ${
                          selected
                            ? "bg-[rgb(var(--accent-rgb)/0.14)] ring-2 ring-[var(--accent)]"
                            : "bg-white/[0.025] ring-1 ring-white/[0.07] hover:bg-white/[0.05]"
                        }`}
                      >
                        <div
                          className="relative flex h-16 w-full overflow-hidden rounded-xl ring-1 ring-white/10"
                          style={{ backgroundColor: preset.bgApp }}
                        >
                          <div
                            className="flex w-12 flex-col gap-1 border-r border-white/10 p-1.5"
                            style={{ backgroundColor: preset.bgPanel }}
                          >
                            <div
                              className="h-2 w-full rounded"
                              style={{ backgroundColor: preset.accent }}
                            />
                            <div className="h-1.5 w-4/5 rounded bg-white/15" />
                            <div className="h-1.5 w-3/5 rounded bg-white/10" />
                          </div>
                          <div className="flex flex-1 flex-col gap-1.5 p-2">
                            <div className="flex items-center justify-between">
                              <div className="h-1.5 w-14 rounded bg-white/25" />
                              <div
                                className="h-2.5 w-6 rounded-full"
                                style={{ backgroundColor: preset.accent }}
                              />
                            </div>
                            <div className="grid flex-1 grid-cols-2 gap-1.5">
                              <div
                                className="rounded-md ring-1 ring-white/10"
                                style={{ backgroundColor: preset.bgCard }}
                              />
                              <div
                                className="rounded-md ring-1 ring-white/10"
                                style={{ backgroundColor: preset.bgCard }}
                              />
                            </div>
                          </div>
                          {selected && (
                            <span
                              className="absolute right-2 top-2 flex h-5 w-5 items-center justify-center rounded-full text-white shadow"
                              style={{ backgroundColor: preset.accent }}
                            >
                              <Check className="h-3 w-3" />
                            </span>
                          )}
                        </div>

                        <div className="flex items-center justify-between gap-2">
                          <div className="min-w-0">
                            <div className="truncate text-xs font-bold text-white">
                              {preset.name}
                            </div>
                            <div className="truncate text-[11px] text-slate-400">
                              {lang === "en" ? preset.descEn : preset.descRu}
                            </div>
                          </div>
                          <span
                            className="h-3.5 w-3.5 shrink-0 rounded-full ring-2 ring-white/20"
                            style={{ backgroundColor: preset.accent }}
                          />
                        </div>
                      </button>
                    );
                  })}

                  {/* 9th Card: Custom Theme */}
                  {(() => {
                    const selected = draft.theme === "custom";
                    return (
                      <button
                        type="button"
                        onClick={() => {
                          const nextCustom = normalizeCustomColors(
                            draft.custom_theme
                          );
                          setDraft((p) => ({
                            ...p,
                            theme: "custom",
                            custom_theme: nextCustom,
                          }));
                          onPreviewTheme("custom");
                          onPreviewCustomTheme(nextCustom);
                        }}
                        className={`group flex flex-col gap-2.5 rounded-2xl p-3 text-left transition ${
                          selected
                            ? "bg-[rgb(var(--accent-rgb)/0.14)] ring-2 ring-[var(--accent)]"
                            : "bg-white/[0.025] ring-1 ring-white/[0.07] hover:bg-white/[0.05]"
                        }`}
                      >
                        <div
                          className="relative flex h-16 w-full overflow-hidden rounded-xl ring-1 ring-white/10"
                          style={{ backgroundColor: customPreviewColors.bg_app }}
                        >
                          <div
                            className="flex w-12 flex-col gap-1 border-r border-white/10 p-1.5"
                            style={{
                              backgroundColor: customPreviewColors.bg_panel,
                            }}
                          >
                            <div
                              className="h-2 w-full rounded"
                              style={{
                                backgroundColor: customPreviewColors.accent,
                              }}
                            />
                            <div className="h-1.5 w-4/5 rounded bg-white/15" />
                            <div className="h-1.5 w-3/5 rounded bg-white/10" />
                          </div>
                          <div className="flex flex-1 flex-col gap-1.5 p-2">
                            <div className="flex items-center justify-between">
                              <div
                                className="h-1.5 w-14 rounded opacity-70"
                                style={{
                                  backgroundColor:
                                    customPreviewColors.text_primary,
                                }}
                              />
                              <div
                                className="h-2.5 w-6 rounded-full"
                                style={{
                                  backgroundColor: customPreviewColors.accent,
                                }}
                              />
                            </div>
                            <div className="grid flex-1 grid-cols-2 gap-1.5">
                              <div
                                className="rounded-md ring-1 ring-white/10"
                                style={{
                                  backgroundColor: customPreviewColors.bg_panel,
                                }}
                              />
                              <div
                                className="rounded-md ring-1 ring-white/10"
                                style={{
                                  backgroundColor: customPreviewColors.bg_panel,
                                }}
                              />
                            </div>
                          </div>
                          {selected && (
                            <span
                              className="absolute right-2 top-2 flex h-5 w-5 items-center justify-center rounded-full text-white shadow"
                              style={{
                                backgroundColor: customPreviewColors.accent,
                              }}
                            >
                              <Check className="h-3 w-3" />
                            </span>
                          )}
                        </div>

                        <div className="flex items-center justify-between gap-2">
                          <div className="min-w-0">
                            <div className="truncate text-xs font-bold text-white">
                              {t.settings.customThemeTitle}
                            </div>
                            <div className="truncate text-[11px] text-slate-400">
                              {t.settings.customThemeDesc}
                            </div>
                          </div>
                          <span
                            className="h-3.5 w-3.5 shrink-0 rounded-full ring-2 ring-white/20"
                            style={{
                              backgroundColor: customPreviewColors.accent,
                            }}
                          />
                        </div>
                      </button>
                    );
                  })()}
                </div>

                {/* Live Custom Theme Builder */}
                <div className="rounded-2xl bg-black/25 p-4 ring-1 ring-white/[0.06]">
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <div>
                      <h3 className="text-xs font-bold text-white">
                        {t.settings.customThemeTitle}
                      </h3>
                      <p className="text-[11px] text-slate-400">
                        {t.settings.customThemeDesc}
                      </p>
                    </div>
                    <button
                      type="button"
                      onClick={handleResetCustomTheme}
                      className={actionChipBtn}
                    >
                      <RotateCcw className="h-3 w-3" />
                      {t.settings.customThemeReset}
                    </button>
                  </div>

                  <div className="mt-3.5 grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4">
                    {customColorFields.map(({ key, label }) => {
                      const pickerVal = customPreviewColors[key];
                      const textVal = hexInputs[key];
                      const valid = isValidHex(textVal);
                      return (
                        <div
                          key={key}
                          className="rounded-xl bg-white/[0.03] p-2.5 ring-1 ring-white/[0.07]"
                        >
                          <div className="mb-2 text-[11px] font-medium text-slate-300">
                            {label}
                          </div>
                          <div className="flex items-center gap-2">
                            <input
                              type="color"
                              value={pickerVal}
                              onChange={(e) =>
                                handleCustomColorInput(key, e.target.value)
                              }
                              aria-label={label}
                              className="h-8 w-10 shrink-0 cursor-pointer rounded-lg border-0 bg-transparent p-0"
                            />
                            <input
                              type="text"
                              maxLength={7}
                              value={textVal}
                              onChange={(e) =>
                                handleCustomColorInput(key, e.target.value)
                              }
                              onBlur={() =>
                                setHexInputs(
                                  normalizeCustomColors(draft.custom_theme)
                                )
                              }
                              className={`min-w-0 flex-1 rounded-lg bg-black/40 px-2.5 py-1.5 font-mono text-xs text-white ring-1 outline-none ${
                                valid
                                  ? "ring-white/[0.1] focus:ring-[var(--accent)]"
                                  : "ring-rose-500/70"
                              }`}
                            />
                          </div>
                        </div>
                      );
                    })}
                  </div>
                </div>

                <AdvancedAccordion
                  title={t.settings.advancedSettings}
                  hint={t.settings.advancedAudioHint}
                >
                  <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
                    <ToggleSwitch
                      checked={draft.audio.separate_tracks}
                      onChange={(v) =>
                        setDraft((p) => ({
                          ...p,
                          audio: { ...p.audio, separate_tracks: v },
                        }))
                      }
                      label={t.settings.separateTracks}
                      tooltip={t.settings.tips.separateTracks}
                    />
                    <ToggleSwitch
                      checked={draft.audio.apply_volumes_to_editor ?? true}
                      onChange={(v) =>
                        setDraft((p) => ({
                          ...p,
                          audio: { ...p.audio, apply_volumes_to_editor: v },
                        }))
                      }
                      label={t.settings.applyVolumesToEditor}
                      tooltip={t.settings.tips.applyVolumesToEditor}
                    />
                  </div>

                  <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
                    <ToggleSwitch
                      checked={draft.audio.mic_soft_limiter}
                      onChange={(v) =>
                        setDraft((p) => ({
                          ...p,
                          audio: { ...p.audio, mic_soft_limiter: v },
                        }))
                      }
                      label={t.settings.softLimiterLabel}
                      hint={t.settings.softLimiterHint}
                    />

                    <div className="space-y-2.5">
                      <ToggleSwitch
                        checked={draft.audio.mic_noise_gate_db !== null}
                        onChange={(en) => {
                          const gate = en ? -50 : null;
                          setDraft((p) => ({
                            ...p,
                            audio: { ...p.audio, mic_noise_gate_db: gate },
                          }));
                          applyLiveRnnoise(
                            draft.audio.mic_noise_suppression,
                            draft.audio.mic_noise_suppression_strength,
                            gate
                          );
                        }}
                        label={t.settings.noiseGateTitle(
                          draft.audio.mic_noise_gate_db !== null
                            ? `${draft.audio.mic_noise_gate_db} dBFS`
                            : t.settings.disabledOption
                        )}
                        tooltip={t.settings.tips.noiseGate}
                      />
                      {draft.audio.mic_noise_gate_db !== null && (
                        <AccentSlider
                          valueBadge={`${draft.audio.mic_noise_gate_db} dBFS`}
                          min={-80}
                          max={-20}
                          step={1}
                          value={draft.audio.mic_noise_gate_db}
                          onChange={(g) => {
                            setDraft((p) => ({
                              ...p,
                              audio: { ...p.audio, mic_noise_gate_db: g },
                            }));
                            applyLiveRnnoise(
                              draft.audio.mic_noise_suppression,
                              draft.audio.mic_noise_suppression_strength,
                              g
                            );
                          }}
                        />
                      )}
                    </div>
                  </div>
                </AdvancedAccordion>
              </section>

              <section className={sectionCls}>
                <h2 className="text-sm font-bold text-white">
                  {t.settings.uiCustomTitle}
                </h2>
                <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
                  <div>
                    <div className={labelTitleCls}>
                      {t.settings.cornerRadiusLabel}
                    </div>
                    <div className="grid grid-cols-3 gap-1.5 rounded-xl bg-white/[0.04] p-1">
                      {(
                        [
                          ["sharp", t.settings.radiusSharp],
                          ["medium", t.settings.radiusMedium],
                          ["soft", t.settings.radiusSoft],
                        ] as [UiCornerRadius, string][]
                      ).map(([val, lbl]) => (
                        <button
                          key={val}
                          type="button"
                          onClick={() => updateUiSetting({ corner_radius: val })}
                          className={`rounded-lg py-1.5 text-xs font-semibold transition ${
                            currentUi.corner_radius === val
                              ? "bg-[var(--accent)] text-white shadow-sm"
                              : "text-slate-400 hover:text-white"
                          }`}
                        >
                          {lbl}
                        </button>
                      ))}
                    </div>
                  </div>

                  <div>
                    <div className={labelTitleCls}>
                      {t.settings.densityLabel}
                    </div>
                    <div className="grid grid-cols-2 gap-1.5 rounded-xl bg-white/[0.04] p-1">
                      {(
                        [
                          ["compact", t.settings.densityCompact],
                          ["normal", t.settings.densityNormal],
                        ] as [UiDensity, string][]
                      ).map(([val, lbl]) => (
                        <button
                          key={val}
                          type="button"
                          onClick={() => updateUiSetting({ density: val })}
                          className={`rounded-lg py-1.5 text-xs font-semibold transition ${
                            currentUi.density === val
                              ? "bg-[var(--accent)] text-white shadow-sm"
                              : "text-slate-400 hover:text-white"
                          }`}
                        >
                          {lbl}
                        </button>
                      ))}
                    </div>
                  </div>

                  <ToggleSwitch
                    checked={currentUi.glass_panels}
                    onChange={(v) => updateUiSetting({ glass_panels: v })}
                    label={t.settings.glassPanelsLabel}
                    hint={t.settings.glassPanelsHint}
                  />

                  <ToggleSwitch
                    checked={currentUi.show_card_specs}
                    onChange={(v) => updateUiSetting({ show_card_specs: v })}
                    label={t.settings.showCardSpecsLabel}
                    hint={t.settings.showCardSpecsHint}
                  />
                </div>
              </section>

              <section className={sectionCls}>
                <h2 className="text-sm font-bold text-white">
                  {t.settings.systemTitle}
                </h2>
                <ToggleSwitch
                  checked={draft.launch_at_startup}
                  onChange={(v) =>
                    setDraft((p) => ({
                      ...p,
                      launch_at_startup: v,
                    }))
                  }
                  label={t.settings.autostartLabel}
                  hint={t.settings.autostartHint}
                  tooltip={t.settings.tips.autostart}
                />
              </section>

              <section className={sectionCls}>
                <div className="flex items-center justify-between gap-2">
                  <div className="flex items-center gap-2">
                    <Download className="h-4 w-4 text-[var(--accent-hover)]" />
                    <h2 className="text-sm font-bold text-white">
                      {t.settings.updatesTitle}
                    </h2>
                    <InfoTooltip text={t.settings.tips.updates} />
                  </div>
                  <span className="rounded-full bg-white/[0.06] px-2.5 py-0.5 font-mono text-[11px] font-semibold text-slate-300">
                    {t.settings.currentVersionLabel}: v
                    {updateInfo?.current_version || __IKOCH_VERSION__}
                  </span>
                </div>

                <ToggleSwitch
                  checked={draft.check_updates_on_startup}
                  onChange={(v) =>
                    setDraft((p) => ({
                      ...p,
                      check_updates_on_startup: v,
                    }))
                  }
                  label={t.settings.autoCheckUpdates}
                />

                <div className="flex flex-wrap items-center gap-2.5">
                  <button
                    type="button"
                    disabled={checkingUpdates}
                    onClick={() => void handleCheckUpdates()}
                    className="inline-flex items-center gap-1.5 rounded-lg bg-white/[0.06] px-3 py-1.5 text-xs font-semibold text-slate-200 ring-1 ring-white/[0.1] hover:bg-white/[0.1] disabled:opacity-50"
                  >
                    <RefreshCw
                      className={`h-3.5 w-3.5 ${checkingUpdates ? "animate-spin" : ""}`}
                    />
                    {checkingUpdates
                      ? t.settings.checkingUpdates
                      : t.settings.checkUpdatesBtn}
                  </button>

                  {updateInfo && !updateInfo.has_update && (
                    <span className="text-xs font-medium text-emerald-300">
                      {t.settings.upToDate} (v{updateInfo.latest_version})
                    </span>
                  )}

                  {updateInfo?.has_update && (
                    <button
                      type="button"
                      disabled={installingUpdate || !updateInfo.download_url}
                      onClick={() => void handleInstallUpdate()}
                      className="inline-flex items-center gap-1.5 rounded-lg bg-[var(--accent)] px-3 py-1.5 text-xs font-semibold text-white hover:bg-[var(--accent-hover)] disabled:opacity-50"
                    >
                      <Download className="h-3.5 w-3.5" />
                      {installingUpdate
                        ? t.settings.installingUpdate
                        : `${t.settings.downloadInstallBtn} (v${updateInfo.latest_version})`}
                    </button>
                  )}
                </div>

                {updateInfo?.has_update && updateInfo.release_notes && (
                  <div className="space-y-1 rounded-xl bg-white/[0.025] p-3 ring-1 ring-white/[0.06]">
                    <div className="text-[11px] font-semibold uppercase tracking-wide text-slate-400">
                      {t.settings.releaseNotesLabel} — v
                      {updateInfo.latest_version}
                    </div>
                    <pre className="max-h-40 overflow-y-auto whitespace-pre-wrap font-sans text-xs text-slate-200">
                      {updateInfo.release_notes}
                    </pre>
                  </div>
                )}
              </section>
            </>
          )}

          {tab === "hotkeys_storage" && (
            <>
              <section className={sectionCls}>
                <div className="flex items-center gap-2">
                  <Keyboard className="h-4 w-4 text-[var(--accent-hover)]" />
                  <h2 className="text-sm font-bold text-white">
                    {t.settings.hotkeysTitle}
                  </h2>
                  <InfoTooltip text={t.settings.tips.hotkeys} />
                </div>
                <div className="space-y-2">
                  {renderHotkeyInput(
                    "save_clip",
                    t.settings.hkSaveClip,
                    draft.hotkeys.save_clip,
                    (val) =>
                      setDraft((p) => ({
                        ...p,
                        hotkeys: { ...p.hotkeys, save_clip: val },
                      }))
                  )}
                  {renderHotkeyInput(
                    "toggle_session",
                    t.settings.hkToggleSession,
                    draft.hotkeys.toggle_session,
                    (val) =>
                      setDraft((p) => ({
                        ...p,
                        hotkeys: { ...p.hotkeys, toggle_session: val },
                      }))
                  )}
                  {renderHotkeyInput(
                    "add_bookmark",
                    t.settings.hkAddBookmark,
                    draft.hotkeys.add_bookmark,
                    (val) =>
                      setDraft((p) => ({
                        ...p,
                        hotkeys: { ...p.hotkeys, add_bookmark: val },
                      }))
                  )}
                  {renderHotkeyInput(
                    "toggle_mic_mute",
                    t.settings.hkMicMute,
                    draft.hotkeys.toggle_mic_mute,
                    (val) =>
                      setDraft((p) => ({
                        ...p,
                        hotkeys: { ...p.hotkeys, toggle_mic_mute: val },
                      }))
                  )}
                  {renderHotkeyInput(
                    "toggle_privacy_mode",
                    t.settings.hkPrivacy,
                    draft.hotkeys.toggle_privacy_mode,
                    (val) =>
                      setDraft((p) => ({
                        ...p,
                        hotkeys: { ...p.hotkeys, toggle_privacy_mode: val },
                      }))
                  )}
                </div>
              </section>

              {/* Editor Heavy Cache Control Section */}
              <section className={sectionCls}>
                <div className="flex flex-wrap items-center justify-between gap-3">
                  <div className="flex items-center gap-2">
                    <Database className="h-4 w-4 text-[var(--accent-hover)]" />
                    <div>
                      <div className="flex items-center gap-1.5">
                        <h2 className="text-sm font-bold text-white">
                          {t.settings.editorCacheTitle}
                        </h2>
                        <InfoTooltip text={t.settings.tips.editorCache} />
                      </div>
                      <p className="text-[11px] text-slate-400">
                        {t.settings.editorCacheSubtitle}
                      </p>
                    </div>
                  </div>
                  <button
                    type="button"
                    onClick={() => void refreshCacheStats()}
                    className={actionChipBtn}
                  >
                    <RefreshCw className="h-3 w-3" />
                    {t.settings.recalculateBtn}
                  </button>
                </div>

                {/* Live Cache Size Breakdown */}
                <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
                  <div className="rounded-xl bg-black/25 p-3.5 ring-1 ring-white/[0.06]">
                    <div className="text-[11px] font-medium text-slate-400">
                      {t.settings.tempCacheCardLabel}
                    </div>
                    <div className="mt-1 flex items-baseline justify-between">
                      <span className="font-mono text-base font-bold text-white">
                        {formatBytes(cacheStats.temp_bytes, lang)}
                      </span>
                      <span className="text-[11px] text-slate-400">
                        {t.filesCount(cacheStats.temp_file_count)}
                      </span>
                    </div>
                  </div>

                  <div className="rounded-xl bg-black/25 p-3.5 ring-1 ring-white/[0.06]">
                    <div className="text-[11px] font-medium text-slate-400">
                      {t.settings.metadataCardLabel}
                    </div>
                    <div className="mt-1 flex items-baseline justify-between">
                      <span className="font-mono text-base font-bold text-emerald-300">
                        {formatBytes(cacheStats.metadata_bytes, lang)}
                      </span>
                      <span className="text-[11px] text-slate-400">
                        {t.filesCount(cacheStats.metadata_file_count)}
                      </span>
                    </div>
                  </div>
                </div>

                {/* Cleanup Mode Radio Cards */}
                <div className="grid grid-cols-1 gap-2.5 md:grid-cols-3">
                  {(
                    [
                      [
                        "on_close",
                        t.settings.cacheModeOnClose,
                        t.settings.cacheModeOnCloseDesc,
                      ],
                      [
                        "timer_and_limit",
                        t.settings.cacheModeTimer,
                        t.settings.cacheModeTimerDesc,
                      ],
                      [
                        "manual",
                        t.settings.cacheModeManual,
                        t.settings.cacheModeManualDesc,
                      ],
                    ] as [CacheCleanupMode, string, string][]
                  ).map(([modeId, title, desc]) => {
                    const active = draft.storage.editor_cache.mode === modeId;
                    return (
                      <button
                        key={modeId}
                        type="button"
                        onClick={() =>
                          setDraft((p) => ({
                            ...p,
                            storage: {
                              ...p.storage,
                              editor_cache: {
                                ...p.storage.editor_cache,
                                mode: modeId,
                              },
                            },
                          }))
                        }
                        className={`rounded-xl p-3 text-left transition ${
                          active
                            ? "bg-[rgb(var(--accent-rgb)/0.16)] ring-2 ring-[var(--accent)]"
                            : "bg-white/[0.025] ring-1 ring-white/[0.06] hover:bg-white/[0.05]"
                        }`}
                      >
                        <div className="text-xs font-semibold text-white">
                          {title}
                        </div>
                        <div className="mt-1 text-[11px] leading-snug text-slate-400">
                          {desc}
                        </div>
                      </button>
                    );
                  })}
                </div>

                {draft.storage.editor_cache.mode === "timer_and_limit" && (
                  <div className="grid grid-cols-1 gap-4 rounded-xl bg-black/25 p-4 ring-1 ring-white/[0.06] md:grid-cols-2">
                    <div>
                      <div className="flex items-center justify-between text-xs text-slate-300">
                        <span>{t.settings.checkIntervalLabel}</span>
                        <span className="rounded-lg bg-[rgb(var(--accent-rgb)/0.16)] px-2 py-0.5 font-mono text-[11px] font-bold text-[var(--accent-hover)] ring-1 ring-[rgb(var(--accent-rgb)/0.32)]">
                          {draft.storage.editor_cache.interval_minutes}{" "}
                          {t.minWord}
                        </span>
                      </div>
                      <div className="mt-2.5 flex items-center gap-1.5">
                        {[15, 30, 60].map((mins) => (
                          <button
                            key={mins}
                            type="button"
                            onClick={() =>
                              setDraft((p) => ({
                                ...p,
                                storage: {
                                  ...p.storage,
                                  editor_cache: {
                                    ...p.storage.editor_cache,
                                    interval_minutes: mins,
                                  },
                                },
                              }))
                            }
                            className={`rounded-lg px-2.5 py-1 text-xs font-semibold transition ${
                              draft.storage.editor_cache.interval_minutes ===
                              mins
                                ? "bg-[var(--accent)] text-white"
                                : "bg-white/[0.06] text-slate-300 hover:bg-white/[0.1]"
                            }`}
                          >
                            {mins}
                            {minShortUnit}
                          </button>
                        ))}
                        <input
                          type="number"
                          min={5}
                          max={1440}
                          value={draft.storage.editor_cache.interval_minutes}
                          onChange={(e) => {
                            const val = Math.max(
                              5,
                              Math.min(1440, Number(e.target.value) || 30)
                            );
                            setDraft((p) => ({
                              ...p,
                              storage: {
                                ...p.storage,
                                editor_cache: {
                                  ...p.storage.editor_cache,
                                  interval_minutes: val,
                                },
                              },
                            }));
                          }}
                          className="ml-auto w-20 rounded-lg bg-black/40 px-2.5 py-1 text-xs text-white ring-1 ring-white/[0.08] outline-none"
                        />
                      </div>
                    </div>

                    <div className="space-y-2.5">
                      <AccentSlider
                        label={t.settings.cacheLimitLabel}
                        valueBadge={`${draft.storage.editor_cache.max_size_mb} ${t.mbWord}`}
                        min={50}
                        max={2048}
                        step={50}
                        value={draft.storage.editor_cache.max_size_mb}
                        onChange={(val) =>
                          setDraft((p) => ({
                            ...p,
                            storage: {
                              ...p.storage,
                              editor_cache: {
                                ...p.storage.editor_cache,
                                max_size_mb: val,
                              },
                            },
                          }))
                        }
                      />
                      <div className="flex items-center gap-1.5">
                        {[100, 250, 500].map((mb) => (
                          <button
                            key={mb}
                            type="button"
                            onClick={() =>
                              setDraft((p) => ({
                                ...p,
                                storage: {
                                  ...p.storage,
                                  editor_cache: {
                                    ...p.storage.editor_cache,
                                    max_size_mb: mb,
                                  },
                                },
                              }))
                            }
                            className={`rounded-lg px-2.5 py-1 text-xs font-semibold transition ${
                              draft.storage.editor_cache.max_size_mb === mb
                                ? "bg-[var(--accent)] text-white"
                                : "bg-white/[0.06] text-slate-300 hover:bg-white/[0.1]"
                            }`}
                          >
                            {mb} {t.mbWord}
                          </button>
                        ))}
                      </div>
                    </div>
                  </div>
                )}

                <div className="flex flex-wrap items-center justify-between gap-3 border-t border-white/[0.06] pt-3">
                  <span className="truncate font-mono text-[11px] text-slate-400">
                    {draft.storage.temp_dir}
                  </span>
                  <button
                    type="button"
                    disabled={clearingCache}
                    onClick={() => void handleClearEditorCacheNow()}
                    className="inline-flex items-center gap-1.5 rounded-xl bg-[rgb(var(--accent-rgb)/0.18)] px-3.5 py-2 text-xs font-semibold text-[var(--accent-hover)] ring-1 ring-[rgb(var(--accent-rgb)/0.4)] transition hover:bg-[var(--accent)] hover:text-white disabled:opacity-50"
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                    {clearingCache
                      ? t.settings.clearingCache
                      : t.settings.clearCacheNowBtn}
                  </button>
                </div>
              </section>

              <section className={sectionCls}>
                <div className="flex items-center gap-1.5">
                  <h2 className="text-sm font-bold text-white">
                    {t.settings.diskAndQuotaTitle}
                  </h2>
                  <InfoTooltip text={t.settings.tips.storagePaths} />
                </div>

                <div>
                  <label className="block text-xs text-slate-300">
                    {t.settings.outputDirLabel}
                    <div className="mt-1.5 flex gap-2">
                      <input
                        type="text"
                        value={draft.storage.output_dir}
                        onChange={(e) =>
                          setDraft((p) => ({
                            ...p,
                            storage: {
                              ...p.storage,
                              output_dir: e.target.value,
                            },
                          }))
                        }
                        className="flex-1 rounded-xl bg-black/35 px-3 py-2 font-mono text-xs text-white ring-1 ring-white/[0.08] outline-none focus:ring-[var(--accent)]"
                      />
                      <button
                        type="button"
                        onClick={onOpenFolder}
                        className="rounded-xl bg-white/[0.06] px-3.5 text-xs font-medium text-slate-200 transition hover:bg-white/[0.12] hover:text-white"
                      >
                        {t.settings.openBtn}
                      </button>
                    </div>
                  </label>
                </div>

                <AdvancedAccordion
                  title={t.settings.advancedSettings}
                  hint={t.settings.advancedStorageHint}
                >
                  <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
                    <label className="block text-xs text-slate-300">
                      {t.settings.tempDirLabel}
                      <input
                        type="text"
                        value={draft.storage.temp_dir}
                        onChange={(e) =>
                          setDraft((p) => ({
                            ...p,
                            storage: { ...p.storage, temp_dir: e.target.value },
                          }))
                        }
                        className={`${fieldCls} font-mono`}
                      />
                    </label>

                    <label className="block text-xs text-slate-300">
                      {t.settings.maxStorageLabel}
                      <input
                        type="number"
                        min={1}
                        placeholder={t.settings.noLimitPlaceholder}
                        value={draft.storage.max_storage_gb ?? ""}
                        onChange={(e) => {
                          const val = e.target.value
                            ? Math.max(1, Number(e.target.value))
                            : null;
                          setDraft((p) => ({
                            ...p,
                            storage: { ...p.storage, max_storage_gb: val },
                          }));
                        }}
                        className={fieldCls}
                      />
                    </label>

                    <label className="block text-xs text-slate-300">
                      {t.settings.maxAgeLabel}
                      <input
                        type="number"
                        min={1}
                        placeholder={t.settings.noAgeLimitPlaceholder}
                        value={draft.storage.max_age_days ?? ""}
                        onChange={(e) => {
                          const val = e.target.value
                            ? Math.max(1, Number(e.target.value))
                            : null;
                          setDraft((p) => ({
                            ...p,
                            storage: { ...p.storage, max_age_days: val },
                          }));
                        }}
                        className={fieldCls}
                      />
                    </label>
                  </div>

                  <div className="flex flex-wrap items-center justify-between gap-4 border-t border-white/[0.06] pt-3">
                    <div className="min-w-[260px] flex-1">
                      <ToggleSwitch
                        checked={draft.storage.auto_remux_fmp4}
                        onChange={(v) =>
                          setDraft((p) => ({
                            ...p,
                            storage: { ...p.storage, auto_remux_fmp4: v },
                          }))
                        }
                        label={t.settings.autoRemuxLabel}
                      />
                    </div>

                    <button
                      type="button"
                      onClick={() => void handleCleanupNow()}
                      className="inline-flex items-center gap-1.5 rounded-xl bg-amber-500/15 px-3.5 py-2 text-xs font-semibold text-amber-300 ring-1 ring-amber-500/35 transition hover:bg-amber-500 hover:text-slate-950"
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                      {t.settings.cleanupDiskBtn}
                    </button>
                  </div>
                </AdvancedAccordion>
              </section>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
