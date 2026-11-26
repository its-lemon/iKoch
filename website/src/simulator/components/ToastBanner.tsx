import { AlertCircle, CheckCircle2, Info, X } from "lucide-react";
import { useI18n } from "../i18n";

export interface UiToast {
  id: string;
  title: string;
  message?: string;
  variant: "success" | "error" | "info";
}

interface ToastBannerProps {
  toasts: UiToast[];
  onDismiss: (id: string) => void;
}

export function ToastBanner({ toasts, onDismiss }: ToastBannerProps) {
  const { t } = useI18n();
  if (toasts.length === 0) return null;

  return (
    <div className="pointer-events-none fixed bottom-4 right-4 z-50 flex w-96 max-w-[calc(100vw-2rem)] flex-col gap-2">
      {toasts.map((item) => {
        const isErr = item.variant === "error";
        const isOk = item.variant === "success";
        const Icon = isErr ? AlertCircle : isOk ? CheckCircle2 : Info;
        const borderCls = isErr
          ? "border-rose-500/40 bg-[#1A1218]/95"
          : isOk
            ? "border-emerald-500/40 bg-[#101B18]/95"
            : "border-[rgb(var(--accent-rgb)/0.45)] bg-[var(--bg-panel)]/95";
        const iconCls = isErr
          ? "text-rose-400"
          : isOk
            ? "text-emerald-400"
            : "text-[var(--accent-hover)]";

        return (
          <div
            key={item.id}
            className={`pointer-events-auto flex items-start gap-3 rounded-xl border px-3.5 py-2.5 shadow-2xl backdrop-blur-md transition-all ${borderCls}`}
          >
            <Icon className={`mt-0.5 h-4 w-4 shrink-0 ${iconCls}`} />
            <div className="min-w-0 flex-1">
              <div className="text-xs font-semibold text-[var(--text-primary)]">
                {item.title}
              </div>
              {item.message && (
                <div className="mt-0.5 break-words text-[11px] leading-snug text-slate-300">
                  {item.message}
                </div>
              )}
            </div>
            <button
              type="button"
              onClick={() => onDismiss(item.id)}
              className="rounded p-0.5 text-slate-400 hover:bg-white/10 hover:text-white"
              aria-label={t.app.dismissToast}
            >
              <X className="h-3.5 w-3.5" />
            </button>
          </div>
        );
      })}
    </div>
  );
}
