interface AccentSliderProps {
  value: number;
  min: number;
  max: number;
  step?: number;
  disabled?: boolean;
  onChange: (value: number) => void;
  label?: string;
  valueBadge?: string;
  className?: string;
  sliderClassName?: string;
  title?: string;
}

export function sliderFillStyle(
  value: number,
  min: number,
  max: number
): React.CSSProperties {
  const span = max - min;
  const pct =
    span > 0
      ? Math.max(0, Math.min(100, ((value - min) / span) * 100))
      : 0;
  return {
    background: `linear-gradient(to right, var(--accent) 0%, var(--accent) ${pct}%, rgba(255, 255, 255, 0.14) ${pct}%, rgba(255, 255, 255, 0.14) 100%)`,
  };
}

export function AccentSlider({
  value,
  min,
  max,
  step = 1,
  disabled = false,
  onChange,
  label,
  valueBadge,
  className = "",
  sliderClassName = "",
  title,
}: AccentSliderProps) {
  return (
    <div className={className}>
      {(label || valueBadge !== undefined) && (
        <div className="mb-2 flex items-center justify-between gap-2">
          {label && (
            <span className="text-xs font-medium text-slate-300">{label}</span>
          )}
          {valueBadge !== undefined && (
            <span className="shrink-0 rounded-lg bg-[rgb(var(--accent-rgb)/0.16)] px-2 py-0.5 font-mono text-[11px] font-bold text-[var(--accent-hover)] ring-1 ring-[rgb(var(--accent-rgb)/0.32)]">
              {valueBadge}
            </span>
          )}
        </div>
      )}
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        disabled={disabled}
        value={value}
        title={title}
        onChange={(e) => onChange(Number(e.target.value))}
        style={sliderFillStyle(value, min, max)}
        className={`ikoch-slider w-full ${sliderClassName}`}
      />
    </div>
  );
}
