import { formatTimePrecise } from "../utils/format";

interface DemoGamePreviewProps {
  gameName: string;
  title?: string;
  isPlaying?: boolean;
  currentTime?: number;
  durationSec?: number;
  compact?: boolean;
}

export function DemoGamePreview({
  gameName,
  isPlaying = false,
  currentTime = 0,
  compact = false,
}: DemoGamePreviewProps) {
  const g = gameName.toLowerCase();
  const pulseStyle = isPlaying ? "animate-pulse" : "";

  if (g.includes("counter-strike") || g.includes("cs2")) {
    return (
      <div className="relative h-full w-full select-none overflow-hidden bg-gradient-to-br from-[#1b2232] via-[#121722] to-[#0a0d14]">
        <div
          className="absolute inset-0 opacity-20"
          style={{
            backgroundImage:
              "linear-gradient(rgba(255,255,255,0.06) 1px, transparent 1px), linear-gradient(90deg, rgba(255,255,255,0.06) 1px, transparent 1px)",
            backgroundSize: "24px 24px",
          }}
        />
        {/* Dust/Mirage warm horizon glow */}
        <div className="absolute inset-x-0 bottom-0 h-1/2 bg-gradient-to-t from-amber-500/10 to-transparent" />
        {/* Radar */}
        <div className="absolute left-3 top-3 h-11 w-11 rounded-full border border-emerald-400/35 bg-black/55 p-1.5 backdrop-blur-sm">
          <div className="h-full w-full rounded-full border border-emerald-400/20" />
          <div className="absolute left-1/2 top-1/2 h-1.5 w-1.5 -translate-x-1/2 -translate-y-1/2 rounded-full bg-emerald-400" />
          <div className="absolute left-3 top-3.5 h-1 w-1 rounded-full bg-rose-400" />
          <div className="absolute right-2.5 top-5 h-1 w-1 rounded-full bg-rose-400" />
        </div>
        {/* Killfeed */}
        <div className="absolute right-3 top-3 flex flex-col items-end gap-1 text-[9px] font-bold">
          <div className="rounded bg-black/70 px-2 py-0.5 text-amber-300 ring-1 ring-rose-500/40">
            iKoch <span className="text-rose-400">[AK-47 HS]</span> Enemy_01
          </div>
          {!compact && (
            <div className="rounded bg-black/70 px-2 py-0.5 text-amber-300 ring-1 ring-rose-500/40">
              iKoch <span className="text-rose-400">[AK-47 HS]</span> Enemy_02
            </div>
          )}
        </div>
        {/* Center Crosshair */}
        <div className="absolute left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2">
          <div className={`relative h-6 w-6 ${pulseStyle}`}>
            <span className="absolute left-1/2 top-0 h-2 w-0.5 -translate-x-1/2 bg-emerald-400" />
            <span className="absolute bottom-0 left-1/2 h-2 w-0.5 -translate-x-1/2 bg-emerald-400" />
            <span className="absolute left-0 top-1/2 h-0.5 w-2 -translate-y-1/2 bg-emerald-400" />
            <span className="absolute right-0 top-1/2 h-0.5 w-2 -translate-y-1/2 bg-emerald-400" />
          </div>
        </div>
        {/* Bottom HP / Ammo HUD */}
        <div className="absolute bottom-3 left-3 flex items-center gap-2 rounded bg-black/65 px-2.5 py-1 text-[10px] font-bold text-emerald-300">
          <span>+ 100 HP</span>
          <span className="text-slate-500">|</span>
          <span className="text-amber-300">24 / 90</span>
        </div>
        {!compact && (
          <div className="absolute bottom-3 right-3 rounded bg-black/65 px-2 py-0.5 font-mono text-[10px] text-slate-300">
            {formatTimePrecise(currentTime)}
          </div>
        )}
      </div>
    );
  }

  if (g.includes("valorant")) {
    return (
      <div className="relative h-full w-full select-none overflow-hidden bg-gradient-to-br from-[#2b1420] via-[#171222] to-[#0a0b12]">
        <div className="absolute inset-0 bg-[radial-gradient(circle_at_50%_42%,rgba(244,63,94,0.22),transparent_62%)]" />
        {/* Top Score + Spike */}
        <div className="absolute left-1/2 top-3 flex -translate-x-1/2 items-center gap-2 rounded bg-black/65 px-2.5 py-0.5 text-[9px] font-bold">
          <span className="text-emerald-300">11</span>
          <span className={`rounded px-1.5 py-0.5 text-rose-300 ring-1 ring-rose-500/40 ${pulseStyle}`}>
            SPIKE
          </span>
          <span className="text-rose-300">10</span>
        </div>
        {/* Crosshair */}
        <div className="absolute left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2">
          <div className="h-2 w-2 rounded-full border border-cyan-300 bg-cyan-300/60 shadow-[0_0_10px_rgba(34,211,238,0.8)]" />
        </div>
        {/* Abilities */}
        <div className="absolute bottom-3 left-1/2 flex -translate-x-1/2 items-center gap-1.5">
          {["C", "Q", "E", "X"].map((k) => (
            <span
              key={k}
              className="flex h-5 w-5 items-center justify-center rounded border border-white/15 bg-black/65 text-[9px] font-bold text-slate-200"
            >
              {k}
            </span>
          ))}
        </div>
        {!compact && (
          <div className="absolute bottom-3 right-3 rounded bg-black/65 px-2 py-0.5 font-mono text-[10px] text-slate-300">
            {formatTimePrecise(currentTime)}
          </div>
        )}
      </div>
    );
  }

  if (g.includes("apex")) {
    return (
      <div className="relative h-full w-full select-none overflow-hidden bg-gradient-to-br from-[#1f1916] via-[#151821] to-[#0a0c12]">
        <div className="absolute inset-0 bg-[radial-gradient(circle_at_55%_45%,rgba(249,115,22,0.2),transparent_60%)]" />
        {/* Top-right Squad / Kills */}
        <div className="absolute right-3 top-3 flex items-center gap-1.5 rounded bg-black/70 px-2 py-0.5 text-[9px] font-bold text-orange-300">
          <span>3 SQUADS</span>
          <span className="text-slate-500">•</span>
          <span className="text-white">9 KILLS</span>
        </div>
        {/* Evo Shield + HP */}
        <div className="absolute bottom-3 left-3 w-32 rounded bg-black/70 p-1.5">
          <div className="flex gap-0.5">
            {[1, 2, 3, 4].map((bar) => (
              <div key={bar} className="h-1.5 flex-1 rounded-sm bg-rose-500" />
            ))}
          </div>
          <div className="mt-1 h-1.5 w-full rounded-sm bg-white/85" />
        </div>
        {!compact && (
          <div className="absolute bottom-3 right-3 rounded bg-black/65 px-2 py-0.5 font-mono text-[10px] text-slate-300">
            {formatTimePrecise(currentTime)}
          </div>
        )}
      </div>
    );
  }

  // Elden Ring / default fantasy boss HUD
  return (
    <div className="relative h-full w-full select-none overflow-hidden bg-gradient-to-br from-[#1c1a13] via-[#121318] to-[#08090d]">
      <div className="absolute inset-0 bg-[radial-gradient(circle_at_50%_40%,rgba(234,179,8,0.18),transparent_62%)]" />
      <div className="absolute left-3 top-3 flex flex-col gap-1">
        <div className="h-1.5 w-24 rounded-full bg-rose-700/85" />
        <div className="h-1 w-16 rounded-full bg-sky-600/80" />
        <div className="h-1 w-20 rounded-full bg-emerald-600/80" />
      </div>
      <div className="absolute inset-x-6 bottom-4">
        <div className="mb-1 text-[9px] font-semibold tracking-wide text-amber-200/90">
          Malenia, Blade of Miquella
        </div>
        <div className="h-1.5 w-full overflow-hidden rounded-full bg-black/70 ring-1 ring-amber-400/25">
          <div
            className="h-full bg-gradient-to-r from-rose-700 to-rose-500 transition-all"
            style={{ width: isPlaying ? "18%" : "28%" }}
          />
        </div>
      </div>
      {!compact && (
        <div className="absolute right-3 top-3 rounded bg-black/65 px-2 py-0.5 font-mono text-[10px] text-slate-300">
          {formatTimePrecise(currentTime)}
        </div>
      )}
    </div>
  );
}
