import { useEffect, useMemo, useRef, useState } from "react";
import {
  Bookmark,
  Plus,
  Redo2,
  Scissors,
  Undo2,
  X,
  ZoomIn,
  ZoomOut,
} from "lucide-react";
import { useI18n } from "../i18n";
import type {
  SegmentRange,
  SessionMarker,
  TrackKind,
  TrackWaveform,
} from "../types";
import { formatTimePrecise, trackDisplayName } from "../utils/format";
import { sliderFillStyle } from "./AccentSlider";

interface TimelinePanelProps {
  durationSec: number;
  currentTime: number;
  markers: SessionMarker[];
  waveforms: TrackWaveform[];
  segments: SegmentRange[];
  selectedSegmentIdx: number;
  loadingMedia: boolean;
  canUndo?: boolean;
  canRedo?: boolean;
  onUndo?: () => void;
  onRedo?: () => void;
  onInteractionStart?: () => void;
  onSeek: (sec: number) => void;
  onSelectSegment: (idx: number) => void;
  onChangeSegments: (next: SegmentRange[]) => void;
  onSplitAtPlayhead: () => void;
}

function trackBarColor(kind: TrackKind): string {
  switch (kind) {
    case "audio_game":
      return "bg-gradient-to-t from-blue-600/85 via-sky-400/90 to-cyan-300";
    case "audio_mic":
      return "bg-gradient-to-t from-emerald-600/85 via-emerald-400/90 to-teal-300";
    case "audio_secondary":
      return "bg-gradient-to-t from-violet-600/85 via-fuchsia-400/90 to-pink-300";
    default:
      return "bg-gradient-to-t from-cyan-600/85 via-cyan-400/90 to-sky-300";
  }
}

type DragAction =
  | { type: "scrub" }
  | { type: "edge_start"; idx: number }
  | { type: "edge_end"; idx: number };

export function TimelinePanel({
  durationSec,
  currentTime,
  markers,
  waveforms,
  segments,
  selectedSegmentIdx,
  loadingMedia,
  canUndo = false,
  canRedo = false,
  onUndo,
  onRedo,
  onInteractionStart,
  onSeek,
  onSelectSegment,
  onChangeSegments,
  onSplitAtPlayhead,
}: TimelinePanelProps) {
  const { lang, t } = useI18n();
  const [zoom, setZoom] = useState(1);
  const trackAreaRef = useRef<HTMLDivElement | null>(null);
  const dragRef = useRef<DragAction | null>(null);

  const safeDur = Math.max(durationSec, 0.1);

  const displayWaveforms = useMemo(() => {
    const stems = waveforms.filter((w) => w.kind !== "audio_mixed");
    return stems.length > 0 ? stems : waveforms;
  }, [waveforms]);

  const timeFromClientX = (clientX: number): number => {
    const el = trackAreaRef.current;
    if (!el) return 0;
    const rect = el.getBoundingClientRect();
    const frac = Math.min(1, Math.max(0, (clientX - rect.left) / rect.width));
    return Number((frac * safeDur).toFixed(2));
  };

  // Global mouse/pointer drag tracking for scrubbing and draggable trim handles.
  useEffect(() => {
    const handleMove = (e: PointerEvent) => {
      const action = dragRef.current;
      if (!action) return;
      const t = timeFromClientX(e.clientX);
      if (action.type === "scrub") {
        onSeek(t);
      } else if (action.type === "edge_start") {
        const seg = segments[action.idx];
        if (!seg) return;
        const clamped = Math.min(Math.max(0, t), seg.end_sec - 0.15);
        onChangeSegments(
          segments.map((s, i) =>
            i === action.idx
              ? { ...s, start_sec: Number(clamped.toFixed(2)) }
              : s
          )
        );
        onSeek(clamped);
      } else if (action.type === "edge_end") {
        const seg = segments[action.idx];
        if (!seg) return;
        const clamped = Math.max(
          seg.start_sec + 0.15,
          Math.min(safeDur, t)
        );
        onChangeSegments(
          segments.map((s, i) =>
            i === action.idx
              ? { ...s, end_sec: Number(clamped.toFixed(2)) }
              : s
          )
        );
        onSeek(clamped);
      }
    };

    const handleUp = () => {
      dragRef.current = null;
    };

    window.addEventListener("pointermove", handleMove);
    window.addEventListener("pointerup", handleUp);
    window.addEventListener("pointercancel", handleUp);
    return () => {
      window.removeEventListener("pointermove", handleMove);
      window.removeEventListener("pointerup", handleUp);
      window.removeEventListener("pointercancel", handleUp);
    };
  }, [onChangeSegments, onSeek, safeDur, segments]);

  const setSegmentInAtCursor = () => {
    const seg = segments[selectedSegmentIdx];
    if (!seg) return;
    onInteractionStart?.();
    const clamped = Math.min(Math.max(0, currentTime), seg.end_sec - 0.15);
    const next = segments.map((s, idx) =>
      idx === selectedSegmentIdx
        ? { ...s, start_sec: Number(clamped.toFixed(2)) }
        : s
    );
    onChangeSegments(next);
  };

  const setSegmentOutAtCursor = () => {
    const seg = segments[selectedSegmentIdx];
    if (!seg) return;
    onInteractionStart?.();
    const clamped = Math.max(
      seg.start_sec + 0.15,
      Math.min(safeDur, currentTime)
    );
    const next = segments.map((s, idx) =>
      idx === selectedSegmentIdx
        ? { ...s, end_sec: Number(clamped.toFixed(2)) }
        : s
    );
    onChangeSegments(next);
  };

  const removeSegment = (idx: number) => {
    if (segments.length <= 1) return;
    onInteractionStart?.();
    const next = segments.filter((_, i) => i !== idx);
    onChangeSegments(next);
    onSelectSegment(Math.max(0, idx - 1));
  };

  const addSegmentAfterCursor = () => {
    onInteractionStart?.();
    const start = Math.min(
      Math.max(0, currentTime),
      Math.max(0, safeDur - 0.5)
    );
    const end = Math.min(safeDur, start + Math.min(5, safeDur - start));
    const next = [
      ...segments,
      {
        start_sec: Number(start.toFixed(2)),
        end_sec: Number(end.toFixed(2)),
      },
    ].sort((a, b) => a.start_sec - b.start_sec);
    onChangeSegments(next);
    onSelectSegment(next.length - 1);
  };

  const snapAroundMarker = (marker: SessionMarker) => {
    onInteractionStart?.();
    const s = Math.max(0, marker.timestamp_sec - 10);
    const e = Math.min(safeDur, marker.timestamp_sec + 5);
    onChangeSegments([
      { start_sec: Number(s.toFixed(2)), end_sec: Number(e.toFixed(2)) },
    ]);
    onSelectSegment(0);
    onSeek(marker.timestamp_sec);
  };

  const playheadPct = Math.min(
    100,
    Math.max(0, (currentTime / safeDur) * 100)
  );

  return (
    <div className="shrink-0 rounded-2xl bg-white/[0.03] px-3 py-2.5 ring-1 ring-white/[0.06]">
      {/* Timeline toolbar */}
      <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
        <div className="flex flex-wrap items-center gap-1.5">
          <button
            type="button"
            onClick={onSplitAtPlayhead}
            className="inline-flex items-center gap-1.5 rounded-lg bg-[rgb(var(--accent-rgb)/0.2)] px-2.5 py-1 text-xs font-semibold text-[var(--accent-hover)] transition hover:bg-[var(--accent)] hover:text-white"
            title={t.timeline.splitHint}
          >
            <Scissors className="h-3.5 w-3.5" />
            {t.timeline.split}
          </button>

          <button
            type="button"
            onClick={setSegmentInAtCursor}
            className="rounded-lg bg-white/[0.05] px-2.5 py-1 text-xs font-medium text-slate-200 transition hover:bg-white/[0.1]"
            title={t.timeline.setInHint}
          >
            {t.timeline.setIn}
          </button>

          <button
            type="button"
            onClick={setSegmentOutAtCursor}
            className="rounded-lg bg-white/[0.05] px-2.5 py-1 text-xs font-medium text-slate-200 transition hover:bg-white/[0.1]"
            title={t.timeline.setOutHint}
          >
            {t.timeline.setOut}
          </button>

          <button
            type="button"
            onClick={addSegmentAfterCursor}
            className="inline-flex items-center gap-1 rounded-lg bg-white/[0.05] px-2.5 py-1 text-xs font-medium text-slate-300 transition hover:bg-white/[0.1] hover:text-white"
            title={t.timeline.addSegmentHint}
          >
            <Plus className="h-3.5 w-3.5" />
            {t.timeline.addSegment.replace(/^\+\s*/, "")}
          </button>

          {onUndo && (
            <button
              type="button"
              disabled={!canUndo}
              onClick={onUndo}
              title={t.timeline.undoHint}
              className="rounded-lg bg-white/[0.05] p-1.5 text-slate-300 transition hover:bg-white/[0.1] hover:text-white disabled:opacity-30"
            >
              <Undo2 className="h-3.5 w-3.5" />
            </button>
          )}
          {onRedo && (
            <button
              type="button"
              disabled={!canRedo}
              onClick={onRedo}
              title={t.timeline.redoHint}
              className="rounded-lg bg-white/[0.05] p-1.5 text-slate-300 transition hover:bg-white/[0.1] hover:text-white disabled:opacity-30"
            >
              <Redo2 className="h-3.5 w-3.5" />
            </button>
          )}
        </div>

        {/* Zoom controls */}
        <div className="flex items-center gap-1.5 text-xs text-slate-400">
          <button
            type="button"
            onClick={() =>
              setZoom((z) => Math.max(1, Number((z - 0.5).toFixed(1))))
            }
            className="rounded p-1 hover:bg-white/[0.07] hover:text-white"
            title="Zoom -"
          >
            <ZoomOut className="h-3.5 w-3.5" />
          </button>
          <input
            type="range"
            min={1}
            max={8}
            step={0.5}
            value={zoom}
            onChange={(e) => setZoom(Number(e.target.value))}
            style={sliderFillStyle(zoom, 1, 8)}
            className="ikoch-slider w-20"
          />
          <button
            type="button"
            onClick={() =>
              setZoom((z) => Math.min(8, Number((z + 0.5).toFixed(1))))
            }
            className="rounded p-1 hover:bg-white/[0.07] hover:text-white"
            title="Zoom +"
          >
            <ZoomIn className="h-3.5 w-3.5" />
          </button>
          <span className="w-7 font-mono text-[11px] text-slate-300">
            {zoom}x
          </span>
        </div>
      </div>

      {/* Clickable Bookmark Chips (F8 markers) */}
      {markers.length > 0 && (
        <div className="mb-2 flex flex-wrap items-center gap-1.5">
          <span className="text-[11px] font-medium text-amber-400/90">
            {t.timeline.markersRowLabel}
          </span>
          {markers.map((m) => (
            <div
              key={m.id}
              className="inline-flex items-center overflow-hidden rounded-lg bg-amber-500/10 text-[11px] ring-1 ring-amber-500/25"
            >
              <button
                type="button"
                onClick={() => onSeek(m.timestamp_sec)}
                className="inline-flex items-center gap-1 px-2 py-0.5 font-medium text-amber-200 hover:bg-amber-500/20"
                title={formatTimePrecise(m.timestamp_sec)}
              >
                <Bookmark className="h-3 w-3 fill-amber-400 text-amber-400" />
                {m.label} ({formatTimePrecise(m.timestamp_sec)})
              </button>
              <button
                type="button"
                onClick={() => snapAroundMarker(m)}
                className="border-l border-amber-500/25 px-1.5 py-0.5 text-[10px] font-semibold text-amber-300 hover:bg-amber-500/25"
                title={t.timeline.extractSubclipTitle}
              >
                -10/+5{t.secShort}
              </button>
            </div>
          ))}
        </div>
      )}

      {/* Horizontal Zoomable Timeline Surface with Scrubbing & Inline Segment Ranges */}
      <div className="overflow-x-auto rounded-xl bg-[var(--bg-app)] p-2">
        <div
          ref={trackAreaRef}
          onPointerDown={(e) => {
            dragRef.current = { type: "scrub" };
            onSeek(timeFromClientX(e.clientX));
          }}
          style={{ width: `${zoom * 100}%`, minWidth: "100%" }}
          className="relative cursor-pointer select-none touch-none"
        >
          {/* Time Ruler + Bookmark Pins */}
          <div className="relative mb-1.5 h-5 border-b border-white/[0.06] text-[10px] font-mono text-slate-500">
            {[0, 0.25, 0.5, 0.75, 1].map((frac) => (
              <span
                key={frac}
                style={{
                  left: `${frac * 100}%`,
                  transform:
                    frac === 1
                      ? "translateX(-100%)"
                      : frac > 0
                        ? "translateX(-50%)"
                        : undefined,
                }}
                className="absolute top-0"
              >
                {formatTimePrecise(frac * safeDur)}
              </span>
            ))}

            {markers.map((m) => {
              const leftPct = Math.min(
                100,
                Math.max(0, (m.timestamp_sec / safeDur) * 100)
              );
              return (
                <button
                  key={m.id}
                  type="button"
                  onPointerDown={(e) => e.stopPropagation()}
                  onClick={(e) => {
                    e.stopPropagation();
                    onSeek(m.timestamp_sec);
                  }}
                  style={{ left: `${leftPct}%` }}
                  title={`${m.label} (${formatTimePrecise(m.timestamp_sec)})`}
                  className="absolute top-0 z-20 -translate-x-1/2 rounded bg-amber-500 px-1 py-0.5 text-[9px] font-bold text-slate-950 shadow hover:bg-amber-400"
                >
                  ★
                </button>
              );
            })}
          </div>

          {/* Kept Segments Track with Embedded Exact Times, Drag Handles & Inline Delete */}
          <div className="relative mb-2 h-8 overflow-hidden rounded-lg bg-white/[0.03]">
            {segments.map((seg, idx) => {
              const leftPct = Math.max(0, (seg.start_sec / safeDur) * 100);
              const widthPct = Math.max(
                2,
                ((seg.end_sec - seg.start_sec) / safeDur) * 100
              );
              const isSelected = idx === selectedSegmentIdx;
              const segDur = (seg.end_sec - seg.start_sec).toFixed(1);
              return (
                <div
                  key={`${idx}`}
                  onPointerDown={(e) => {
                    e.stopPropagation();
                    onSelectSegment(idx);
                    onSeek(timeFromClientX(e.clientX));
                  }}
                  style={{ left: `${leftPct}%`, width: `${widthPct}%` }}
                  className={`absolute inset-y-0.5 flex items-center justify-between overflow-hidden rounded-md transition ${
                    isSelected
                      ? "bg-[rgb(var(--accent-rgb)/0.34)] text-white ring-1 ring-[var(--accent-hover)]"
                      : "bg-emerald-500/20 text-emerald-200 ring-1 ring-emerald-400/35 hover:bg-emerald-500/30"
                  }`}
                >
                  {/* Left trim drag handle */}
                  <div
                    onPointerDown={(e) => {
                      e.stopPropagation();
                      onInteractionStart?.();
                      onSelectSegment(idx);
                      dragRef.current = { type: "edge_start", idx };
                    }}
                    title={t.timeline.resizeStartHint}
                    className="flex h-full w-2.5 shrink-0 cursor-ew-resize items-center justify-center bg-white/15 hover:bg-[var(--accent)]"
                  >
                    <div className="h-3 w-[2px] rounded-full bg-white" />
                  </div>

                  <div className="flex min-w-0 flex-1 items-center justify-center gap-1.5 px-1.5 font-mono text-[10px] font-semibold">
                    <span className="truncate">
                      #{idx + 1} • {formatTimePrecise(seg.start_sec)}–
                      {formatTimePrecise(seg.end_sec)} ({segDur}
                      {t.secShort})
                    </span>
                    {segments.length > 1 && (
                      <button
                        type="button"
                        onPointerDown={(e) => e.stopPropagation()}
                        onClick={(e) => {
                          e.stopPropagation();
                          removeSegment(idx);
                        }}
                        title={t.timeline.deleteSegmentHint}
                        className="shrink-0 rounded p-0.5 text-white/70 hover:bg-rose-500 hover:text-white"
                      >
                        <X className="h-3 w-3" />
                      </button>
                    )}
                  </div>

                  {/* Right trim drag handle */}
                  <div
                    onPointerDown={(e) => {
                      e.stopPropagation();
                      onInteractionStart?.();
                      onSelectSegment(idx);
                      dragRef.current = { type: "edge_end", idx };
                    }}
                    title={t.timeline.resizeEndHint}
                    className="flex h-full w-2.5 shrink-0 cursor-ew-resize items-center justify-center bg-white/15 hover:bg-[var(--accent)]"
                  >
                    <div className="h-3 w-[2px] rounded-full bg-white" />
                  </div>
                </div>
              );
            })}
          </div>

          {/* Multi-track Smooth Gradient Waveforms */}
          <div className="space-y-1">
            {displayWaveforms.length === 0 ? (
              <div className="flex h-12 items-center justify-center rounded-lg bg-white/[0.025] text-xs text-slate-500">
                {loadingMedia
                  ? t.timeline.loadingWaveforms
                  : t.timeline.noAudioNotice}
              </div>
            ) : (
              displayWaveforms.map((wf) => {
                const trackMax = wf.peaks.reduce((m, v) => (v > m ? v : m), 0);
                return (
                  <div
                    key={`${wf.stream_ordinal}-${JSON.stringify(wf.kind)}`}
                    className="rounded-lg bg-white/[0.025] px-2 py-1"
                  >
                    <div className="mb-0.5 flex items-center justify-between text-[10px] font-medium text-slate-400">
                      <span>{trackDisplayName(wf.kind, wf.label, lang)}</span>
                      <span className="font-mono text-[9px] text-slate-500">
                        Track {wf.stream_ordinal + 1}
                      </span>
                    </div>
                    <div className="flex h-6 items-center gap-[1.5px] overflow-hidden">
                      {wf.peaks.map((amp, barIdx) => {
                        const norm =
                          trackMax > 0.0005
                            ? Math.pow(Math.max(0, amp) / trackMax, 0.55)
                            : 0;
                        const pctHeight = Math.max(8, Math.min(100, norm * 100));
                        return (
                          <div
                            key={barIdx}
                            style={{ height: `${pctHeight}%` }}
                            className={`min-w-[1px] flex-1 rounded-full ${trackBarColor(
                              wf.kind
                            )}`}
                          />
                        );
                      })}
                    </div>
                  </div>
                );
              })
            )}
          </div>

          {/* Playhead Scrubber */}
          <div
            style={{ left: `${playheadPct}%` }}
            className="pointer-events-none absolute inset-y-0 z-30 w-[2px] -translate-x-1/2 bg-rose-500 shadow-[0_0_8px_rgba(244,63,94,0.9)]"
          >
            <div className="h-3 w-3 -translate-x-[5px] rounded-full bg-rose-500 ring-2 ring-white/80" />
          </div>
        </div>
      </div>
    </div>
  );
}
