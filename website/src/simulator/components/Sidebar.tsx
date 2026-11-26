import { Film, Scissors, Settings } from "lucide-react";
import { useI18n } from "../i18n";
import type { ClipMetadata, DaemonStateSnapshot } from "../types";
import { formatBytes } from "../utils/format";

export type AppTab = "library" | "editor" | "settings";

interface SidebarProps {
  activeTab: AppTab;
  onSelectTab: (tab: AppTab) => void;
  daemon: DaemonStateSnapshot;
  selectedClip: ClipMetadata | null;
  clipCount: number;
}

export function Sidebar({
  activeTab,
  onSelectTab,
  daemon,
  selectedClip,
  clipCount,
}: SidebarProps) {
  const { lang, t } = useI18n();

  const fillPct =
    daemon.buffer_capacity_sec > 0
      ? Math.min(
          100,
          Math.max(
            0,
            (daemon.buffer_fill_sec / daemon.buffer_capacity_sec) * 100
          )
        )
      : 0;

  const navItems: {
    id: AppTab;
    label: string;
    icon: typeof Film;
    badge?: string;
  }[] = [
    {
      id: "library",
      label: t.sidebar.library,
      icon: Film,
      badge: clipCount > 0 ? String(clipCount) : undefined,
    },
    {
      id: "editor",
      label: t.sidebar.editor,
      icon: Scissors,
      badge: selectedClip ? t.sidebar.editorActive : undefined,
    },
    {
      id: "settings",
      label: t.sidebar.settings,
      icon: Settings,
    },
  ];

  return (
    <aside className="flex w-52 shrink-0 flex-col justify-between border-r border-[var(--border-subtle,rgba(255,255,255,0.06))] bg-[var(--bg-sidebar,var(--bg-panel))] p-3">
      {/* Navigation links */}
      <nav className="space-y-1 pt-1">
        {navItems.map((item) => {
          const Icon = item.icon;
          const isActive = activeTab === item.id;
          return (
            <button
              key={item.id}
              type="button"
              onClick={() => onSelectTab(item.id)}
              className={`flex w-full items-center justify-between rounded-xl px-3 py-2.5 text-left text-sm font-medium transition ${
                isActive
                  ? "bg-[rgb(var(--accent-rgb)/0.16)] text-[var(--accent-hover)] ring-1 ring-[rgb(var(--accent-rgb)/0.38)]"
                  : "text-slate-300 hover:bg-white/[0.05] hover:text-white"
              }`}
            >
              <span className="flex items-center gap-2.5">
                <Icon
                  className={`h-4 w-4 ${
                    isActive ? "text-[var(--accent-hover)]" : "text-slate-400"
                  }`}
                />
                {item.label}
              </span>
              {item.badge && (
                <span
                  className={`rounded-md px-1.5 py-0.5 text-[10px] font-semibold ${
                    isActive
                      ? "bg-[rgb(var(--accent-rgb)/0.28)] text-white"
                      : "bg-white/[0.06] text-slate-400"
                  }`}
                >
                  {item.badge}
                </span>
              )}
            </button>
          );
        })}
      </nav>

      {/* Live Replay Buffer Telemetry Card */}
      <div className="rounded-xl bg-white/[0.03] p-3 ring-1 ring-white/[0.06]">
        <div className="flex items-center justify-between text-[11px] font-semibold text-slate-300">
          <span>{t.sidebar.replayBuffer}</span>
          <span className="font-mono text-slate-400">
            {daemon.buffer_fill_sec.toFixed(0)}
            {t.secShort} / {daemon.buffer_capacity_sec}
            {t.secShort}
          </span>
        </div>
        <div className="mt-2 h-1.5 w-full overflow-hidden rounded-full bg-white/[0.08]">
          <div
            className={`h-full rounded-full transition-all duration-300 ${
              daemon.privacy_mode
                ? "bg-amber-500"
                : daemon.tray_state === "recording"
                  ? "bg-rose-500"
                  : "bg-emerald-500"
            }`}
            style={{ width: `${fillPct}%` }}
          />
        </div>
        <div className="mt-2.5 flex items-center justify-between text-[11px] text-slate-400">
          <span>{t.sidebar.bufferRam}</span>
          <span className="font-mono text-slate-200">
            {formatBytes(daemon.buffer_bytes, lang)}
          </span>
        </div>
        <div className="mt-1 flex items-center justify-between text-[11px] text-slate-400">
          <span>{t.sidebar.noiseSuppression}</span>
          <span
            className={
              daemon.mic_noise_suppression
                ? "text-emerald-400"
                : "text-slate-500"
            }
          >
            {daemon.mic_noise_suppression
              ? t.sidebar.rnnoiseOn
              : t.sidebar.rnnoiseOff}
          </span>
        </div>
      </div>
    </aside>
  );
}
