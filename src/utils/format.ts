import type { AppLanguage, CaptureTarget, TrackKind } from "../types";

export function formatDuration(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "0:00";
  const total = Math.floor(seconds);
  const mins = Math.floor(total / 60);
  const secs = total % 60;
  return `${mins}:${secs.toString().padStart(2, "0")}`;
}

export function formatTimePrecise(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "00:00.0";
  const mins = Math.floor(seconds / 60);
  const secs = Math.floor(seconds % 60);
  const tenths = Math.floor((seconds % 1) * 10);
  return `${mins.toString().padStart(2, "0")}:${secs
    .toString()
    .padStart(2, "0")}.${tenths}`;
}

export function formatBytes(bytes: number, lang: AppLanguage = "ru"): string {
  const units =
    lang === "en" ? ["B", "KB", "MB", "GB", "TB"] : ["Б", "КБ", "МБ", "ГБ", "ТБ"];
  if (!Number.isFinite(bytes) || bytes <= 0) return `0 ${units[2]}`;
  let val = bytes;
  let idx = 0;
  while (val >= 1024 && idx < units.length - 1) {
    val /= 1024;
    idx += 1;
  }
  const precision = idx >= 2 ? 1 : 0;
  return `${val.toFixed(precision)} ${units[idx]}`;
}

export function formatDate(iso: string, lang: AppLanguage = "ru"): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return date.toLocaleString(lang === "en" ? "en-US" : "ru-RU", {
    day: "2-digit",
    month: "short",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export function formatDateRu(iso: string): string {
  return formatDate(iso, "ru");
}

export function captureTargetKey(target: CaptureTarget): string {
  switch (target.kind) {
    case "auto_game":
      return "auto_game";
    case "monitor":
      return `monitor:${target.monitor_id}`;
    case "specific_window":
      return `window:${target.hwnd}`;
  }
}

export function captureTargetLabel(
  target: CaptureTarget,
  lang: AppLanguage = "ru"
): string {
  switch (target.kind) {
    case "auto_game":
      return lang === "en" ? "Auto (Game)" : "Авто (Игра)";
    case "monitor":
      return target.name;
    case "specific_window":
      return `[${target.process_name}] ${target.title || (lang === "en" ? "Window" : "Окно")}`;
  }
}

export function trackDisplayName(
  kind: TrackKind,
  fallback: string,
  lang: AppLanguage = "ru"
): string {
  if (lang === "en") {
    switch (kind) {
      case "audio_game":
        return "Game / System";
      case "audio_mic":
        return "Microphone";
      case "audio_secondary":
        return "Secondary (Discord/VM)";
      case "audio_mixed":
        return "Mixed Audio";
      default:
        return fallback;
    }
  }
  switch (kind) {
    case "audio_game":
      return "Игра / Система";
    case "audio_mic":
      return "Микрофон";
    case "audio_secondary":
      return "2-й канал (Discord/VM)";
    case "audio_mixed":
      return "Общий микс";
    default:
      return fallback;
  }
}

export function cleanExportPrefix(
  sourcePath: string,
  gameName?: string
): string {
  const rawGame = (gameName ?? "").trim();
  const gameLower = rawGame.toLowerCase();
  if (
    rawGame &&
    (gameLower === "screen" ||
      gameLower === "desktop" ||
      gameLower.startsWith("monitor") ||
      gameLower.startsWith("экран"))
  ) {
    return "Screen";
  }
  if (rawGame) {
    const cleaned = rawGame
      .replace(/\([^)]*\)/g, " ")
      .replace(/\.exe$/i, "")
      .replace(/[<>:"/\\|?*]+/g, " ")
      .replace(/\s+/g, " ")
      .trim()
      .slice(0, 32)
      .trim();
    if (cleaned) return cleaned;
  }

  const slashIdx = Math.max(
    sourcePath.lastIndexOf("\\"),
    sourcePath.lastIndexOf("/")
  );
  const fileName = slashIdx >= 0 ? sourcePath.slice(slashIdx + 1) : sourcePath;
  const stem = fileName.replace(/\.[^.]+$/, "");
  if (
    /^monitor[_\s]/i.test(stem) ||
    /^screen([_\s]|$)/i.test(stem) ||
    /^desktop/i.test(stem)
  ) {
    return "Screen";
  }
  const stripped = stem
    .replace(
      /(_\d{4}-\d{2}-\d{2}_\d{2}-\d{2}(-\d{2})?|_\d{8}_\d{6}_\d+).*$/,
      ""
    )
    .trim();
  return stripped || "Screen";
}

export function deriveExportPath(
  sourcePath: string,
  suffix: string,
  ext: "mp4" | "gif" | "png",
  gameName?: string,
  outputDir?: string
): string {
  const prefix = cleanExportPrefix(sourcePath, gameName);
  const now = new Date();
  const pad = (n: number) => n.toString().padStart(2, "0");
  const stamp = `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(
    now.getDate()
  )}_${pad(now.getHours())}-${pad(now.getMinutes())}-${pad(now.getSeconds())}`;

  const cleanOut = outputDir
    ?.trim()
    .replace(/\//g, "\\")
    .replace(/\\+$/, "");
  if (cleanOut) {
    return `${cleanOut}\\Edited\\${prefix}_${stamp}_${suffix}.${ext}`;
  }

  const slashIdx = Math.max(
    sourcePath.lastIndexOf("\\"),
    sourcePath.lastIndexOf("/")
  );
  const dir = slashIdx >= 0 ? sourcePath.slice(0, slashIdx + 1) : "";
  if (/[\\/]Edited[\\/]$/i.test(dir)) {
    return `${dir}${prefix}_${stamp}_${suffix}.${ext}`;
  }
  return `${dir}Edited\\${prefix}_${stamp}_${suffix}.${ext}`;
}
