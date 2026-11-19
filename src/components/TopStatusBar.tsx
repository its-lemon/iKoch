import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  Bookmark,
  Copy,
  Disc,
  Mic,
  MicOff,
  Minus,
  Monitor,
  RefreshCw,
  Scissors,
  Shield,
  ShieldAlert,
  Square,
  X,
} from "lucide-react";
import { useI18n } from "../i18n";
import type {
  AppSettings,
  CaptureTarget,
  DaemonStateSnapshot,
  MonitorInfo,
  WindowInfo,
} from "../types";
import { captureTargetKey, formatDuration } from "../utils/format";
import { CustomSelect } from "./CustomSelect";

interface TopStatusBarProps {
  daemon: DaemonStateSnapshot;
  settings: AppSettings | null;
  monitors: MonitorInfo[];
  windows: WindowInfo[];
  refreshingSources: boolean;
  onRefreshSources: () => void;
  onSelectTarget: (target: CaptureTarget) => void;
  onSaveClip: () => void;
  onToggleSession: () => void;
  onAddBookmark: () => void;
  onToggleMicMute: () => void;
  onTogglePrivacy: () => void;
}

export function TopStatusBar({
  daemon,
  settings,
  monitors,
  windows,
  refreshingSources,
  onRefreshSources,
  onSelectTarget,
  onSaveClip,
  onToggleSession,
  onAddBookmark,
  onToggleMicMute,
  onTogglePrivacy,
}: TopStatusBarProps) {
  const { t } = useI18n();
  const [isMaximized, setIsMaximized] = useState(false);

  useEffect(() => {
    const win = getCurrentWindow();
    let active = true;
    win
      .isMaximized()
      .then((val) => {
        if (active) setIsMaximized(val);
      })
      .catch(() => {});

    let unlisten: (() => void) | undefined;
    win
      .onResized(() => {
        win
          .isMaximized()
          .then((val) => {
            if (active) setIsMaximized(val);
          })
          .catch(() => {});
      })
      .then((fn) => {
        if (!active) fn();
        else unlisten = fn;
      })
      .catch(() => {});

    return () => {
      active = false;
      if (unlisten) unlisten();
    };
  }, []);

  const hotkeys = settings?.hotkeys ?? {
    save_clip: "F9",
    toggle_session: "F10",
    add_bookmark: "F8",
    toggle_mic_mute: "Ctrl+Shift+M",
    toggle_privacy_mode: "Ctrl+Shift+P",
  };

  const isRecording = Boolean(daemon.active_session);
  const canSaveClip = !daemon.privacy_mode && daemon.buffer_fill_sec >= 0.5;
  const currentKey = captureTargetKey(daemon.capture_target);

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

  if (!targetOptions.some((o) => o.key === currentKey)) {
    if (daemon.capture_target.kind === "specific_window") {
      targetOptions.push({
        key: currentKey,
        label: t.topBar.windowPrefix(
          daemon.capture_target.process_name,
          daemon.capture_target.title
        ),
        target: daemon.capture_target,
      });
    } else if (daemon.capture_target.kind === "monitor") {
      targetOptions.push({
        key: currentKey,
        label: t.topBar.monitorPrefix(daemon.capture_target.name),
        target: daemon.capture_target,
      });
    }
  }

  let statusBadge = (
    <span
      className="pointer-events-none inline-flex items-center gap-2 rounded-full bg-white/[0.05] px-2.5 py-1 text-[11px] font-medium text-slate-300 ring-1 ring-white/[0.06]"
      title={t.topBar.idleHint}
    >
      <span className="h-1.5 w-1.5 rounded-full bg-slate-400" />
      {t.topBar.idle}
    </span>
  );

  if (daemon.privacy_mode) {
    statusBadge = (
      <span
        className="pointer-events-none inline-flex items-center gap-1.5 rounded-full bg-amber-500/15 px-2.5 py-1 text-[11px] font-semibold text-amber-300 ring-1 ring-amber-500/35"
        title={t.topBar.privacyHint}
      >
        <ShieldAlert className="h-3.5 w-3.5 text-amber-400" />
        {t.topBar.privacyMode}
      </span>
    );
  } else if (isRecording && daemon.active_session) {
    statusBadge = (
      <span
        className="pointer-events-none inline-flex items-center gap-2 rounded-full bg-rose-500/20 px-2.5 py-1 text-[11px] font-bold text-rose-200 ring-1 ring-rose-500/45"
        title={t.topBar.recordingHint}
      >
        <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-rose-500" />
        {t.topBar.recording(formatDuration(daemon.active_session.elapsed_sec))}
      </span>
    );
  } else if (daemon.tray_state === "buffering") {
    statusBadge = (
      <span
        className="pointer-events-none inline-flex items-center gap-2 rounded-full bg-emerald-500/15 px-2.5 py-1 text-[11px] font-semibold text-emerald-300 ring-1 ring-emerald-500/35"
        title={t.topBar.bufferingHint}
      >
        <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-emerald-400" />
        {t.topBar.buffering(
          daemon.buffer_fill_sec.toFixed(0),
          daemon.buffer_capacity_sec
        )}
      </span>
    );
  } else if (daemon.tray_state === "error") {
    statusBadge = (
      <span
        className="pointer-events-none inline-flex items-center gap-2 rounded-full bg-rose-500/15 px-2.5 py-1 text-[11px] font-semibold text-rose-300 ring-1 ring-rose-500/35"
        title={t.topBar.errorHint}
      >
        <span className="h-1.5 w-1.5 rounded-full bg-rose-500" />
        {t.topBar.errorState}
      </span>
    );
  }

  const handleToggleMax = () => {
    void getCurrentWindow().toggleMaximize();
  };

  return (
    <header
      data-tauri-drag-region
      className="flex h-12 shrink-0 select-none items-center justify-between gap-2 border-b border-white/[0.06] bg-[var(--bg-panel)] pl-3.5 pr-1.5"
    >
      {/* Left minimalist iKoch brand emblem + Live Status (draggable) */}
      <div
        data-tauri-drag-region
        onDoubleClick={handleToggleMax}
        className="flex shrink-0 items-center gap-2.5"
      >
        <div
          data-tauri-drag-region
          className="pointer-events-none flex items-center gap-2 pr-1"
        >
          <svg
            viewBox="0 0 24 24"
            className="h-5 w-5 shrink-0"
            fill="none"
            aria-hidden="true"
          >
            <rect
              x="1.5"
              y="1.5"
              width="21"
              height="21"
              rx="6"
              fill="var(--bg-card)"
              stroke="rgba(255,255,255,0.14)"
              strokeWidth="1.2"
            />
            <circle cx="7.4" cy="7.3" r="1.45" fill="var(--accent)" />
            <rect x="6.2" y="10.1" width="2.4" height="7.4" rx="1.1" fill="#F8FAFC" />
            <path
              d="M10.6 6.5H13V11.1L16.4 6.5H19.1L15.1 11.7L19.4 17.5H16.6L13 12.6V17.5H10.6V6.5Z"
              fill="#F8FAFC"
            />
          </svg>
          <span className="text-[13px] font-bold tracking-tight text-[var(--text-primary)]">
            iKoch
          </span>
        </div>

        {statusBadge}

        {daemon.active_game && !daemon.privacy_mode && (
          <span
            className="pointer-events-none hidden items-center gap-1.5 rounded-lg bg-[rgb(var(--accent-rgb)/0.16)] px-2.5 py-1 text-[11px] font-medium text-[var(--accent-hover)] md:inline-flex"
            title={t.topBar.activeGameTitle(daemon.active_game.executable_name)}
          >
            <span className="h-1.5 w-1.5 rounded-full bg-[var(--accent)]" />
            {daemon.active_game.display_name}
          </span>
        )}
      </div>

      {/* Dedicated drag region spacer */}
      <div
        data-tauri-drag-region
        onDoubleClick={handleToggleMax}
        className="h-full min-w-[16px] flex-1"
      />

      {/* Quick actions + Native-feeling frameless window controls */}
      <div className="flex shrink-0 items-center gap-1.5">
        <div className="flex items-center rounded-xl bg-white/[0.04] ring-1 ring-white/[0.06] transition focus-within:ring-[rgb(var(--accent-rgb)/0.6)]">
          <div
            className="flex items-center pl-2.5 text-slate-400"
            title={t.topBar.captureSourceTooltip}
          >
            <Monitor className="h-3.5 w-3.5" />
          </div>
          <CustomSelect
            variant="embedded"
            value={currentKey}
            options={targetOptions.map((o) => ({
              value: o.key,
              label: o.label,
            }))}
            onChange={(val) => {
              const found = targetOptions.find((o) => o.key === val);
              if (found) onSelectTarget(found.target);
            }}
            className="w-[180px]"
            menuMinWidth={260}
            title={t.topBar.captureSourceSelectTitle}
          />
          <button
            type="button"
            onClick={onRefreshSources}
            disabled={refreshingSources}
            title={t.topBar.refreshSources}
            className="border-l border-white/[0.06] px-2 py-1 text-slate-400 transition hover:bg-white/[0.08] hover:text-white disabled:opacity-50"
          >
            <RefreshCw
              className={`h-3.5 w-3.5 ${refreshingSources ? "animate-spin" : ""}`}
            />
          </button>
        </div>

        <button
          type="button"
          onClick={onSaveClip}
          disabled={daemon.privacy_mode}
          title={
            daemon.privacy_mode
              ? t.topBar.saveClipHintPrivacy
              : canSaveClip
                ? t.topBar.saveClipHintReady(
                    daemon.buffer_fill_sec.toFixed(0),
                    hotkeys.save_clip
                  )
                : t.topBar.saveClipHintEmpty
          }
          className={`inline-flex items-center gap-1.5 rounded-xl px-2.5 py-1 text-xs font-semibold transition active:scale-[0.98] disabled:cursor-not-allowed disabled:opacity-40 ${
            canSaveClip
              ? "bg-[var(--accent)] text-white shadow-sm hover:bg-[var(--accent-hover)]"
              : "bg-white/[0.04] text-slate-400 hover:bg-white/[0.08] hover:text-slate-200"
          }`}
        >
          <Scissors className="h-3.5 w-3.5" />
          <span className="hidden lg:inline">
            {t.topBar.saveClip(hotkeys.save_clip)}
          </span>
          <span className="lg:hidden">{hotkeys.save_clip}</span>
        </button>

        <button
          type="button"
          onClick={onToggleSession}
          disabled={daemon.privacy_mode}
          title={
            isRecording
              ? t.topBar.stopSessionHint(hotkeys.toggle_session)
              : t.topBar.startSessionHint(hotkeys.toggle_session)
          }
          className={`inline-flex items-center gap-1.5 rounded-xl px-2.5 py-1 text-xs font-semibold transition active:scale-[0.98] disabled:cursor-not-allowed disabled:opacity-40 ${
            isRecording
              ? "bg-rose-600 text-white shadow-sm hover:bg-rose-500"
              : "bg-white/[0.05] text-slate-200 hover:bg-rose-500/20 hover:text-white"
          }`}
        >
          {isRecording ? (
            <>
              <Square className="h-3 w-3 fill-current" />
              <span className="hidden lg:inline">{t.topBar.stopSession}</span>
            </>
          ) : (
            <>
              <Disc className="h-3.5 w-3.5 text-rose-400" />
              <span className="hidden lg:inline">
                {t.topBar.startSession(hotkeys.toggle_session)}
              </span>
            </>
          )}
        </button>

        <button
          type="button"
          onClick={onAddBookmark}
          className="inline-flex items-center gap-1 rounded-xl bg-white/[0.05] px-2 py-1 text-xs font-medium text-amber-300 transition hover:bg-amber-500/15"
          title={t.topBar.addBookmarkHint(hotkeys.add_bookmark)}
        >
          <Bookmark className="h-3.5 w-3.5" />
          <span className="hidden xl:inline">{hotkeys.add_bookmark}</span>
        </button>

        <button
          type="button"
          onClick={onToggleMicMute}
          className={`inline-flex items-center gap-1 rounded-xl px-2 py-1 text-xs font-medium transition ${
            daemon.mic_muted
              ? "bg-rose-500/15 text-rose-300 hover:bg-rose-500/25"
              : "bg-white/[0.05] text-slate-200 hover:bg-emerald-500/15"
          }`}
          title={
            daemon.mic_muted
              ? t.topBar.micUnmuteHint(hotkeys.toggle_mic_mute)
              : t.topBar.micMuteHint(hotkeys.toggle_mic_mute)
          }
        >
          {daemon.mic_muted ? (
            <MicOff className="h-3.5 w-3.5 text-rose-400" />
          ) : (
            <Mic className="h-3.5 w-3.5 text-emerald-400" />
          )}
        </button>

        <button
          type="button"
          onClick={onTogglePrivacy}
          className={`inline-flex items-center gap-1 rounded-xl px-2 py-1 text-xs font-medium transition ${
            daemon.privacy_mode
              ? "bg-amber-500/20 text-amber-200 ring-1 ring-amber-500/50"
              : "bg-white/[0.05] text-slate-400 hover:bg-white/[0.1] hover:text-slate-200"
          }`}
          title={
            daemon.privacy_mode
              ? t.topBar.privacyOffHint(hotkeys.toggle_privacy_mode)
              : t.topBar.privacyOnHint(hotkeys.toggle_privacy_mode)
          }
        >
          <Shield className="h-3.5 w-3.5" />
        </button>

        {/* Window Caption Controls */}
        <div className="ml-1 flex items-center gap-0.5 border-l border-white/[0.08] pl-1.5">
          <button
            type="button"
            onClick={() => void getCurrentWindow().minimize()}
            title={t.topBar.minimize}
            aria-label={t.topBar.minimize}
            className="flex h-8 w-8 items-center justify-center rounded-lg text-slate-400 transition hover:bg-white/[0.08] hover:text-white"
          >
            <Minus className="h-3.5 w-3.5" />
          </button>
          <button
            type="button"
            onClick={handleToggleMax}
            title={isMaximized ? t.topBar.restore : t.topBar.maximize}
            aria-label={isMaximized ? t.topBar.restore : t.topBar.maximize}
            className="flex h-8 w-8 items-center justify-center rounded-lg text-slate-400 transition hover:bg-white/[0.08] hover:text-white"
          >
            {isMaximized ? (
              <Copy className="h-3 w-3 rotate-90" />
            ) : (
              <Square className="h-3 w-3" />
            )}
          </button>
          <button
            type="button"
            onClick={() => void getCurrentWindow().close()}
            title={t.topBar.closeToTray}
            aria-label={t.topBar.closeToTray}
            className="flex h-8 w-8 items-center justify-center rounded-lg text-slate-400 transition hover:bg-rose-600 hover:text-white"
          >
            <X className="h-4 w-4" />
          </button>
        </div>
      </div>
    </header>
  );
}
