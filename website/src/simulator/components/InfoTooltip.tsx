import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { HelpCircle } from "lucide-react";

interface InfoTooltipProps {
  text: string;
}

export function InfoTooltip({ text }: InfoTooltipProps) {
  const [open, setOpen] = useState(false);
  const [coords, setCoords] = useState<{
    top: number;
    left: number;
    showAbove: boolean;
  } | null>(null);
  const btnRef = useRef<HTMLButtonElement | null>(null);
  const closeTimerRef = useRef<number | null>(null);

  const updatePosition = useCallback(() => {
    const el = btnRef.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    const width = Math.min(280, Math.max(200, window.innerWidth - 24));
    const margin = 12;
    const left = Math.min(
      Math.max(margin, rect.left + rect.width / 2 - width / 2),
      window.innerWidth - width - margin
    );
    const estimatedHeight = 140;
    const showAbove =
      rect.bottom + estimatedHeight + margin > window.innerHeight &&
      rect.top - estimatedHeight - margin > 0;
    const rawTop = showAbove ? rect.top - 8 : rect.bottom + 8;
    const top = Math.min(
      Math.max(margin, rawTop),
      window.innerHeight - margin
    );
    setCoords({ top, left, showAbove });
  }, []);

  const handleOpen = () => {
    if (closeTimerRef.current) {
      window.clearTimeout(closeTimerRef.current);
      closeTimerRef.current = null;
    }
    updatePosition();
    setOpen(true);
  };

  const handleCloseDelayed = () => {
    closeTimerRef.current = window.setTimeout(() => {
      setOpen(false);
    }, 120);
  };

  useEffect(() => {
    if (!open) return;
    const onDocClick = (e: MouseEvent) => {
      if (btnRef.current && !btnRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    const onScrollOrResize = () => updatePosition();
    window.addEventListener("mousedown", onDocClick);
    window.addEventListener("keydown", onKey);
    window.addEventListener("scroll", onScrollOrResize, true);
    window.addEventListener("resize", onScrollOrResize);
    return () => {
      window.removeEventListener("mousedown", onDocClick);
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("scroll", onScrollOrResize, true);
      window.removeEventListener("resize", onScrollOrResize);
    };
  }, [open, updatePosition]);

  return (
    <span className="relative inline-flex items-center">
      <button
        ref={btnRef}
        type="button"
        onClick={(e) => {
          e.stopPropagation();
          if (open) {
            setOpen(false);
          } else {
            handleOpen();
          }
        }}
        onMouseEnter={handleOpen}
        onMouseLeave={handleCloseDelayed}
        aria-label={text}
        className="inline-flex h-4 w-4 items-center justify-center rounded-full text-slate-400 transition hover:bg-[rgb(var(--accent-rgb)/0.16)] hover:text-[var(--accent-hover)]"
      >
        <HelpCircle className="h-3.5 w-3.5" />
      </button>

      {open &&
        coords &&
        createPortal(
          <div
            onMouseEnter={() => {
              if (closeTimerRef.current) {
                window.clearTimeout(closeTimerRef.current);
                closeTimerRef.current = null;
              }
            }}
            onMouseLeave={handleCloseDelayed}
            style={{
              position: "fixed",
              top: coords.top,
              left: coords.left,
              width: Math.min(280, Math.max(200, window.innerWidth - 24)),
              transform: coords.showAbove ? "translateY(-100%)" : undefined,
            }}
            className="z-[9999] rounded-xl border border-white/15 bg-slate-950/95 px-3 py-2 text-[11px] leading-relaxed text-slate-200 shadow-2xl backdrop-blur-md"
          >
            {text}
          </div>,
          document.body
        )}
    </span>
  );
}
