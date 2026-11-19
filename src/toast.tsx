import React, { useEffect } from "react";
import ReactDOM from "react-dom/client";
import {
  AlertTriangle,
  Bookmark,
  CheckCircle2,
  Clapperboard,
  Info,
  ShieldAlert,
  Video,
} from "lucide-react";
import { getTranslations } from "./i18n";
import {
  applyThemeToDocument,
  loadStoredLanguage,
  loadStoredTheme,
  normalizeCustomColors,
  normalizeThemeSlug,
} from "./utils/theme";
import "./index.css";

function ToastOverlay() {
  const params = new URLSearchParams(window.location.search);
  const rawLang = params.get("lang");
  const lang = rawLang === "en" || rawLang === "ru" ? rawLang : loadStoredLanguage();
  const dict = getTranslations(lang);

  const title = params.get("title") || dict.toast.defaultTitle;
  const message = params.get("message") || dict.toast.defaultMessage;

  const stored = loadStoredTheme();
  const theme = params.has("theme")
    ? normalizeThemeSlug(params.get("theme"))
    : stored.theme;
  const customColors =
    theme === "custom" &&
    (params.has("cbg") ||
      params.has("cpanel") ||
      params.has("caccent") ||
      params.has("ctext"))
      ? normalizeCustomColors({
          bg_app: params.get("cbg") ?? undefined,
          bg_panel: params.get("cpanel") ?? undefined,
          accent: params.get("caccent") ?? undefined,
          text_primary: params.get("ctext") ?? undefined,
        })
      : stored.custom;

  useEffect(() => {
    applyThemeToDocument(theme, customColors);
  }, [theme, customColors]);

  const lower = `${title} ${message}`.toLowerCase();

  let badgeStyle =
    "bg-[rgb(var(--accent-rgb)/0.2)] text-[var(--accent-hover)] border-[rgb(var(--accent-rgb)/0.45)]";
  let cardGlow = "border-[rgb(var(--accent-rgb)/0.4)]";
  let accentBar = "from-[var(--accent)] to-[var(--accent-hover)]";
  let Icon = Info;

  if (lower.includes("приват") || lower.includes("privacy")) {
    badgeStyle = "bg-amber-500/20 text-amber-300 border-amber-500/40";
    cardGlow = "border-amber-500/35";
    accentBar = "from-amber-500 to-orange-400";
    Icon = ShieldAlert;
  } else if (
    lower.includes("пуст") ||
    lower.includes("наполняется") ||
    lower.includes("не выбран") ||
    lower.includes("не удалось") ||
    lower.includes("empty") ||
    lower.includes("priming") ||
    lower.includes("no capture") ||
    lower.includes("could not")
  ) {
    badgeStyle = "bg-amber-500/20 text-amber-300 border-amber-500/40";
    cardGlow = "border-amber-500/35";
    accentBar = "from-amber-500 to-yellow-400";
    Icon = AlertTriangle;
  } else if (
    lower.includes("клип сохранён") ||
    lower.includes("clip saved")
  ) {
    badgeStyle = "bg-emerald-500/20 text-emerald-300 border-emerald-500/40";
    cardGlow = "border-emerald-500/35";
    accentBar = "from-emerald-500 to-teal-400";
    Icon = CheckCircle2;
  } else if (
    lower.includes("запись нача") ||
    lower.includes("recording started") ||
    lower.includes("started")
  ) {
    badgeStyle = "bg-rose-500/20 text-rose-300 border-rose-500/40";
    cardGlow = "border-rose-500/35";
    accentBar = "from-rose-500 to-orange-400";
    Icon = Video;
  } else if (
    lower.includes("запись сохран") ||
    lower.includes("recording saved") ||
    lower.includes("stopped")
  ) {
    badgeStyle = "bg-violet-500/20 text-violet-300 border-violet-500/40";
    cardGlow = "border-violet-500/35";
    accentBar = "from-violet-500 to-indigo-400";
    Icon = Clapperboard;
  } else if (lower.includes("маркер") || lower.includes("bookmark")) {
    badgeStyle = "bg-amber-500/20 text-amber-300 border-amber-500/40";
    cardGlow = "border-amber-500/35";
    accentBar = "from-amber-500 to-yellow-400";
    Icon = Bookmark;
  }

  return (
    <div className="h-[98px] w-[370px] p-2">
      <div
        className={`animate-toast-enter relative flex h-full w-full items-center gap-3 overflow-hidden rounded-2xl border bg-[var(--bg-panel)]/95 px-3.5 py-2.5 shadow-[0_10px_28px_rgba(0,0,0,0.72)] backdrop-blur-xl ${cardGlow}`}
      >
        <div
          className={`flex h-10 w-10 shrink-0 items-center justify-center rounded-xl border ${badgeStyle}`}
        >
          <Icon className="h-4 w-4" />
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-1.5">
            <span className="text-[10px] font-bold uppercase tracking-wider text-slate-400">
              iKoch
            </span>
            <span className="h-1 w-1 shrink-0 rounded-full bg-slate-600" />
            <span className="text-xs font-bold leading-tight text-[var(--text-primary)]">
              {title}
            </span>
          </div>
          <p className="mt-0.5 line-clamp-2 break-words text-[11px] leading-snug text-slate-300">
            {message}
          </p>
        </div>
        <div
          className={`animate-toast-progress absolute bottom-0 left-0 h-[2.5px] bg-gradient-to-r ${accentBar}`}
        />
      </div>
    </div>
  );
}

ReactDOM.createRoot(document.getElementById("toast-root")!).render(
  <React.StrictMode>
    <ToastOverlay />
  </React.StrictMode>
);
