import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Check, ChevronDown } from "lucide-react";

export interface SelectOption {
  value: string;
  label: string;
  description?: string;
}

interface CustomSelectProps {
  value: string;
  options: SelectOption[];
  onChange: (value: string) => void;
  placeholder?: string;
  className?: string;
  triggerClassName?: string;
  menuMinWidth?: number;
  disabled?: boolean;
  title?: string;
  ariaLabel?: string;
  variant?: "default" | "embedded";
}

export function CustomSelect({
  value,
  options,
  onChange,
  placeholder = "—",
  className = "",
  triggerClassName = "",
  menuMinWidth = 200,
  disabled = false,
  title,
  ariaLabel,
  variant = "default",
}: CustomSelectProps) {
  const [open, setOpen] = useState(false);
  const [coords, setCoords] = useState<{
    top: number;
    left: number;
    width: number;
    openUpward: boolean;
  }>({ top: 0, left: 0, width: 200, openUpward: false });

  const triggerRef = useRef<HTMLButtonElement | null>(null);
  const menuRef = useRef<HTMLDivElement | null>(null);

  const selected = options.find((o) => o.value === value);

  const updatePosition = () => {
    const el = triggerRef.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    const estimatedMenuHeight = Math.min(264, options.length * 36 + 16);
    const spaceBelow = window.innerHeight - rect.bottom - 12;
    const spaceAbove = rect.top - 12;
    const openUpward =
      spaceBelow < estimatedMenuHeight && spaceAbove > spaceBelow;
    const width = Math.max(rect.width, menuMinWidth);
    const left = Math.max(
      8,
      Math.min(rect.left, window.innerWidth - width - 8)
    );
    const top = openUpward ? rect.top - 6 : rect.bottom + 6;
    setCoords({ top, left, width, openUpward });
  };

  useEffect(() => {
    if (!open) return;
    updatePosition();

    const onMouseDown = (e: MouseEvent) => {
      const t = e.target as Node | null;
      if (triggerRef.current?.contains(t) || menuRef.current?.contains(t)) {
        return;
      }
      setOpen(false);
    };

    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        setOpen(false);
        triggerRef.current?.focus();
      } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        if (options.length === 0) return;
        e.preventDefault();
        const curIdx = options.findIndex((o) => o.value === value);
        const delta = e.key === "ArrowDown" ? 1 : -1;
        const nextIdx =
          curIdx === -1
            ? 0
            : (curIdx + delta + options.length) % options.length;
        onChange(options[nextIdx].value);
      } else if (e.key === "Enter") {
        setOpen(false);
      }
    };

    const onScroll = (e: Event) => {
      if (menuRef.current && menuRef.current.contains(e.target as Node)) {
        return;
      }
      setOpen(false);
    };

    window.addEventListener("mousedown", onMouseDown);
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("resize", updatePosition);
    window.addEventListener("scroll", onScroll, true);
    window.addEventListener("blur", () => setOpen(false), { once: true });

    return () => {
      window.removeEventListener("mousedown", onMouseDown);
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("resize", updatePosition);
      window.removeEventListener("scroll", onScroll, true);
    };
  }, [open, options, value]);

  const baseTrigger =
    variant === "embedded"
      ? "flex w-full items-center justify-between gap-2 bg-transparent px-2.5 py-1 text-left text-xs font-medium text-slate-200 outline-none transition hover:text-white disabled:cursor-not-allowed disabled:opacity-50"
      : "flex w-full items-center justify-between gap-2 rounded-xl bg-black/35 px-3 py-2 text-left text-xs font-medium text-slate-100 ring-1 ring-white/[0.08] outline-none transition hover:bg-white/[0.06] hover:ring-white/[0.15] focus:ring-[var(--accent)] disabled:cursor-not-allowed disabled:opacity-50";

  return (
    <div className={`relative ${className}`}>
      <button
        ref={triggerRef}
        type="button"
        disabled={disabled}
        title={title ?? selected?.label}
        aria-label={ariaLabel ?? title ?? selected?.label}
        aria-expanded={open}
        aria-haspopup="listbox"
        onClick={() => {
          if (!disabled) setOpen((v) => !v);
        }}
        className={`${baseTrigger} ${triggerClassName}`}
      >
        <span className="min-w-0 flex-1 truncate">
          {selected?.label ?? placeholder}
        </span>
        <ChevronDown
          className={`h-3.5 w-3.5 shrink-0 text-slate-400 transition-transform duration-150 ${
            open ? "rotate-180 text-[var(--accent-hover)]" : ""
          }`}
        />
      </button>

      {open &&
        createPortal(
          <div
            ref={menuRef}
            role="listbox"
            style={{
              position: "fixed",
              left: coords.left,
              top: coords.top,
              width: coords.width,
              transform: coords.openUpward ? "translateY(-100%)" : undefined,
              zIndex: 9999,
            }}
            className="max-h-64 overflow-y-auto rounded-xl border border-white/[0.1] bg-[var(--bg-panel)]/95 p-1.5 shadow-[0_20px_50px_rgba(0,0,0,0.75)] backdrop-blur-xl"
          >
            {options.length === 0 ? (
              <div className="px-3 py-2 text-xs text-slate-500">
                {placeholder}
              </div>
            ) : (
              options.map((opt) => {
                const isSelected = opt.value === value;
                return (
                  <button
                    key={opt.value}
                    type="button"
                    role="option"
                    aria-selected={isSelected}
                    onClick={() => {
                      onChange(opt.value);
                      setOpen(false);
                    }}
                    className={`flex w-full items-center justify-between gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs transition ${
                      isSelected
                        ? "bg-[rgb(var(--accent-rgb)/0.22)] font-semibold text-white ring-1 ring-[rgb(var(--accent-rgb)/0.4)]"
                        : "text-slate-300 hover:bg-white/[0.08] hover:text-white"
                    }`}
                  >
                    <div className="min-w-0 flex-1">
                      <div className="truncate">{opt.label}</div>
                      {opt.description && (
                        <div className="truncate text-[10px] text-slate-400">
                          {opt.description}
                        </div>
                      )}
                    </div>
                    {isSelected && (
                      <Check className="h-3.5 w-3.5 shrink-0 text-[var(--accent-hover)]" />
                    )}
                  </button>
                );
              })
            )}
          </div>,
          document.body
        )}
    </div>
  );
}
