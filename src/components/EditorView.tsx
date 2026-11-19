import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ArrowLeft,
  Camera,
  ClipboardCopy,
  Download,
  Film,
  Loader2,
  Maximize2,
  Minimize2,
  Pause,
  Play,
  Scissors,
  Smartphone,
  Square,
  Volume2,
  VolumeX,
} from "lucide-react";
import { api, toAssetUrl } from "../api";
import type {
  AppLanguage,
  AppSettings,
  AspectRatioPreset,
  ClipMetadata,
  DiscordPreset,
  PreparedEditorMedia,
  SegmentRange,
  TrackKind,
  TrackMixSpec,
  TrackWaveform,
  TrimAudioExportMode,
} from "../types";
import { useI18n } from "../i18n";
import {
  deriveExportPath,
  formatBytes,
  formatTimePrecise,
  trackDisplayName,
} from "../utils/format";
import { sliderFillStyle } from "./AccentSlider";
import { CustomSelect, type SelectOption } from "./CustomSelect";
import { TimelinePanel } from "./TimelinePanel";

interface EditorViewProps {
  clip: ClipMetadata | null;
  settings: AppSettings;
  onBackToLibrary: () => void;
  onClipCreatedOrUpdated: () => Promise<void>;
  onOpenFolder: () => void;
  onNotify: (
    title: string,
    message: string,
    variant?: "success" | "error" | "info"
  ) => void;
}

interface TrackMixState {
  kind: TrackKind;
  label: string;
  volume: number; // 0.0 .. 2.0
  muted: boolean;
}

interface EditHistoryState {
  segments: SegmentRange[];
  playbackSpeed: number;
  fadeInSec: number;
  fadeOutSec: number;
  aspectRatio: AspectRatioPreset;
}

function defaultMixList(
  waveforms: TrackWaveform[],
  lang: AppLanguage
): TrackMixState[] {
  const stems = waveforms.filter((w) => w.kind !== "audio_mixed");
  const active = stems.length > 0 ? stems : waveforms;
  return active.map((w) => ({
    kind: w.kind,
    label: trackDisplayName(w.kind, w.label, lang),
    volume: 1.0,
    muted: false,
  }));
}

export function EditorView({
  clip,
  settings,
  onBackToLibrary,
  onClipCreatedOrUpdated,
  onOpenFolder,
  onNotify,
}: EditorViewProps) {
  const { t, lang } = useI18n();
  const tRef = useRef(t);
  tRef.current = t;
  const langRef = useRef(lang);
  langRef.current = lang;
  const playerWrapperRef = useRef<HTMLDivElement | null>(null);
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const audioCtxRef = useRef<AudioContext | null>(null);
  const stemNodesRef = useRef<
    Map<
      string,
      {
        audio: HTMLAudioElement;
        gain: GainNode;
        source: MediaElementAudioSourceNode;
      }
    >
  >(new Map());

  const [prepared, setPrepared] = useState<PreparedEditorMedia | null>(null);
  const [loadingMedia, setLoadingMedia] = useState(false);
  const [stemsReady, setStemsReady] = useState(false);

  const [isPlaying, setIsPlaying] = useState(false);
  const [isFullscreen, setIsFullscreen] = useState(false);
  const [masterVolume, setMasterVolume] = useState(1);
  const [currentTime, setCurrentTime] = useState(0);
  const [durationSec, setDurationSec] = useState(1);

  const [segments, setSegments] = useState<SegmentRange[]>([]);
  const [selectedSegmentIdx, setSelectedSegmentIdx] = useState(0);

  const [playbackSpeed, setPlaybackSpeed] = useState(1);
  const [aspectRatio, setAspectRatio] =
    useState<AspectRatioPreset>("original");
  const [fadeInSec, setFadeInSec] = useState(0);
  const [fadeOutSec, setFadeOutSec] = useState(0);

  const [undoStack, setUndoStack] = useState<EditHistoryState[]>([]);
  const [redoStack, setRedoStack] = useState<EditHistoryState[]>([]);

  const [trackMix, setTrackMix] = useState<TrackMixState[]>([]);
  const [trimAudioMode, setTrimAudioMode] = useState<TrimAudioExportMode>(
    "keep_separate_tracks"
  );

  const [discordPresetKind, setDiscordPresetKind] = useState<
    "mb10" | "mb50" | "custom_mb"
  >("mb10");
  const [customDiscordMb, setCustomDiscordMb] = useState(25);

  const [busyTask, setBusyTask] = useState<string | null>(null);
  const [busyElapsedSec, setBusyElapsedSec] = useState(0);
  const [lastOutputPath, setLastOutputPath] = useState<string | null>(null);
  const [sidebarTab, setSidebarTab] = useState<"quick" | "export">("quick");

  useEffect(() => {
    const onFsChange = () => {
      setIsFullscreen(Boolean(document.fullscreenElement));
    };
    document.addEventListener("fullscreenchange", onFsChange);
    return () => document.removeEventListener("fullscreenchange", onFsChange);
  }, []);

  const toggleFullscreen = useCallback(async () => {
    const el = playerWrapperRef.current;
    if (!el) return;
    try {
      if (document.fullscreenElement) {
        await document.exitFullscreen();
      } else {
        await el.requestFullscreen();
      }
    } catch {
      // Ignore fullscreen denial
    }
  }, []);

  const currentSnapshot = useCallback(
    (): EditHistoryState => ({
      segments: segments.map((s) => ({ ...s })),
      playbackSpeed,
      fadeInSec,
      fadeOutSec,
      aspectRatio,
    }),
    [aspectRatio, fadeInSec, fadeOutSec, playbackSpeed, segments]
  );

  const pushUndoSnapshot = useCallback(() => {
    const snap = currentSnapshot();
    setUndoStack((prev) => [...prev.slice(-49), snap]);
    setRedoStack([]);
  }, [currentSnapshot]);

  const applyHistorySnapshot = useCallback((snap: EditHistoryState) => {
    setSegments(snap.segments.map((s) => ({ ...s })));
    setPlaybackSpeed(snap.playbackSpeed);
    setFadeInSec(snap.fadeInSec);
    setFadeOutSec(snap.fadeOutSec);
    setAspectRatio(snap.aspectRatio);
    setSelectedSegmentIdx((idx) =>
      Math.min(idx, Math.max(0, snap.segments.length - 1))
    );
  }, []);

  const handleUndo = useCallback(() => {
    if (undoStack.length === 0) return;
    const prev = undoStack[undoStack.length - 1];
    const cur = currentSnapshot();
    setUndoStack((s) => s.slice(0, -1));
    setRedoStack((s) => [...s.slice(-49), cur]);
    applyHistorySnapshot(prev);
  }, [applyHistorySnapshot, currentSnapshot, undoStack]);

  const handleRedo = useCallback(() => {
    if (redoStack.length === 0) return;
    const next = redoStack[redoStack.length - 1];
    const cur = currentSnapshot();
    setRedoStack((s) => s.slice(0, -1));
    setUndoStack((s) => [...s.slice(-49), cur]);
    applyHistorySnapshot(next);
  }, [applyHistorySnapshot, currentSnapshot, redoStack]);

  // Track elapsed time cleanly while FFmpeg runs an export.
  useEffect(() => {
    if (!busyTask) {
      setBusyElapsedSec(0);
      return;
    }
    const started = performance.now();
    const id = window.setInterval(() => {
      setBusyElapsedSec((performance.now() - started) / 1000);
    }, 100);
    return () => window.clearInterval(id);
  }, [busyTask]);

  const cleanupStemAudio = useCallback(() => {
    for (const node of stemNodesRef.current.values()) {
      node.audio.pause();
      node.audio.src = "";
      try {
        node.source.disconnect();
        node.gain.disconnect();
      } catch {
        // Ignore disconnected nodes
      }
    }
    stemNodesRef.current.clear();
    if (audioCtxRef.current) {
      void audioCtxRef.current.close().catch(() => {});
      audioCtxRef.current = null;
    }
    setStemsReady(false);
  }, []);

  // Load clip metadata & prepare FFmpeg stems + waveforms when clip switches.
  useEffect(() => {
    cleanupStemAudio();
    setPrepared(null);
    setIsPlaying(false);
    setCurrentTime(0);
    setLastOutputPath(null);
    setPlaybackSpeed(1);
    setAspectRatio("original");
    setFadeInSec(0);
    setFadeOutSec(0);
    setUndoStack([]);
    setRedoStack([]);
    setSelectedSegmentIdx(0);
    setTrackMix([]);

    if (!clip) return;

    const initialDur = Math.max(clip.duration_sec || 1, 0.2);
    setDurationSec(initialDur);
    setSegments([{ start_sec: 0, end_sec: Number(initialDur.toFixed(2)) }]);

    let cancelled = false;
    setLoadingMedia(true);

    api
      .prepareEditorMedia(clip.file_path, 180)
      .then(async (res) => {
        if (cancelled) return;
        setPrepared(res);
        const exactDur = Math.max(
          res.probed.duration_sec || clip.duration_sec || 1,
          0.2
        );
        setDurationSec(exactDur);
        setSegments([{ start_sec: 0, end_sec: Number(exactDur.toFixed(2)) }]);
        setTrackMix(defaultMixList(res.waveforms, langRef.current));

        const stems = res.waveforms.filter((w) => w.kind !== "audio_mixed");
        const playable = stems.length > 0 ? stems : res.waveforms;
        if (playable.length === 0) return;

        try {
          const Ctx =
            window.AudioContext ||
            (
              window as unknown as {
                webkitAudioContext: typeof AudioContext;
              }
            ).webkitAudioContext;
          const ctx = new Ctx();
          audioCtxRef.current = ctx;

          const readyPromises: Promise<void>[] = [];
          for (const wf of playable) {
            const audio = new Audio();
            audio.crossOrigin = "anonymous";
            audio.preload = "auto";

            const readyP = new Promise<void>((resolve) => {
              const done = () => {
                audio.removeEventListener("canplay", done);
                audio.removeEventListener("loadeddata", done);
                audio.removeEventListener("error", done);
                resolve();
              };
              audio.addEventListener("canplay", done);
              audio.addEventListener("loadeddata", done);
              audio.addEventListener("error", done);
              window.setTimeout(done, 1500);
            });
            readyPromises.push(readyP);

            audio.src = toAssetUrl(wf.stem_audio_path);
            const source = ctx.createMediaElementSource(audio);
            const gain = ctx.createGain();
            gain.gain.value = 1.0;
            source.connect(gain);
            gain.connect(ctx.destination);

            stemNodesRef.current.set(wf.kind, { audio, gain, source });
          }

          await Promise.all(readyPromises);
          if (cancelled) return;

          setStemsReady(true);
          const vid = videoRef.current;
          if (vid) {
            vid.muted = true;
            if (!vid.paused) {
              if (ctx.state === "suspended") {
                await ctx.resume().catch(() => {});
              }
              for (const node of stemNodesRef.current.values()) {
                node.audio.currentTime = vid.currentTime;
                node.audio.playbackRate = vid.playbackRate;
                void node.audio.play().catch(() => {});
              }
            }
          }
        } catch {
          setStemsReady(false);
        }
      })
      .catch((err) => {
        if (!cancelled) {
          onNotify(
            tRef.current.editor.mediaProbeWarning,
            String(err),
            "error"
          );
        }
      })
      .finally(() => {
        if (!cancelled) setLoadingMedia(false);
      });

    return () => {
      cancelled = true;
      cleanupStemAudio();
    };
  }, [cleanupStemAudio, clip?.file_path]);

  // Apply live track volume (0..200%) & mute + masterVolume directly to Web Audio GainNodes.
  useEffect(() => {
    for (const mix of trackMix) {
      const node = stemNodesRef.current.get(mix.kind);
      if (node) {
        node.gain.gain.value = mix.muted
          ? 0
          : Math.min(2, Math.max(0, mix.volume)) * masterVolume;
      }
    }
    if (videoRef.current && !stemsReady) {
      videoRef.current.volume = masterVolume;
    }
  }, [masterVolume, stemsReady, trackMix]);

  // Sync playbackRate across <video> and all stem <audio> elements.
  useEffect(() => {
    if (videoRef.current) {
      videoRef.current.playbackRate = playbackSpeed;
    }
    for (const node of stemNodesRef.current.values()) {
      node.audio.playbackRate = playbackSpeed;
    }
  }, [playbackSpeed]);

  const seekAll = useCallback(
    (sec: number) => {
      const clamped = Math.min(Math.max(0, sec), durationSec);
      setCurrentTime(clamped);
      if (videoRef.current) {
        videoRef.current.currentTime = clamped;
      }
      for (const node of stemNodesRef.current.values()) {
        node.audio.currentTime = clamped;
      }
    },
    [durationSec]
  );

  const pauseAllStems = useCallback(() => {
    for (const node of stemNodesRef.current.values()) {
      node.audio.pause();
    }
  }, []);

  const syncPlayAllStems = useCallback(
    async (timeSec: number, rate: number) => {
      if (!stemsReady) return;
      if (audioCtxRef.current && audioCtxRef.current.state === "suspended") {
        await audioCtxRef.current.resume().catch(() => {});
      }
      for (const node of stemNodesRef.current.values()) {
        if (Math.abs(node.audio.currentTime - timeSec) > 0.22) {
          node.audio.currentTime = timeSec;
        }
        node.audio.playbackRate = rate;
        if (node.audio.paused) {
          void node.audio.play().catch(() => {});
        }
      }
    },
    [stemsReady]
  );

  const togglePlay = useCallback(async () => {
    const vid = videoRef.current;
    if (!vid) return;

    if (vid.paused) {
      const sorted = [...segments].sort((a, b) => a.start_sec - b.start_sec);
      if (sorted.length > 0) {
        const insideAny = sorted.some(
          (s) =>
            vid.currentTime >= s.start_sec && vid.currentTime < s.end_sec - 0.04
        );
        if (!insideAny) {
          seekAll(sorted[0].start_sec);
        }
      }
      try {
        await vid.play();
        await syncPlayAllStems(vid.currentTime, playbackSpeed);
        setIsPlaying(true);
      } catch (err) {
        onNotify(t.editor.playErrorTitle, String(err), "error");
      }
    } else {
      vid.pause();
      pauseAllStems();
      setIsPlaying(false);
    }
  }, [
    onNotify,
    pauseAllStems,
    playbackSpeed,
    seekAll,
    segments,
    syncPlayAllStems,
    t.editor.playErrorTitle,
  ]);

  const handleSplitAtPlayhead = useCallback(() => {
    const tSec = Number(currentTime.toFixed(2));
    const targetIdx = segments.findIndex(
      (s) => tSec > s.start_sec + 0.15 && tSec < s.end_sec - 0.15
    );
    if (targetIdx === -1) {
      onNotify(t.editor.cannotSplitTitle, t.editor.cannotSplitMsg, "info");
      return;
    }
    pushUndoSnapshot();
    const seg = segments[targetIdx];
    const left: SegmentRange = { start_sec: seg.start_sec, end_sec: tSec };
    const right: SegmentRange = { start_sec: tSec, end_sec: seg.end_sec };
    const next = [
      ...segments.slice(0, targetIdx),
      left,
      right,
      ...segments.slice(targetIdx + 1),
    ];
    setSegments(next);
    setSelectedSegmentIdx(targetIdx + 1);
  }, [
    currentTime,
    onNotify,
    pushUndoSnapshot,
    segments,
    t.editor.cannotSplitMsg,
    t.editor.cannotSplitTitle,
  ]);

  const handleDeleteSelectedSegment = useCallback(() => {
    if (segments.length <= 1) {
      onNotify(
        t.editor.cannotDeleteLastTitle,
        t.editor.cannotDeleteLastMsg,
        "info"
      );
      return;
    }
    pushUndoSnapshot();
    const idxToDelete = Math.min(selectedSegmentIdx, segments.length - 1);
    const next = segments.filter((_, i) => i !== idxToDelete);
    setSegments(next);
    setSelectedSegmentIdx(Math.max(0, idxToDelete - 1));
  }, [
    onNotify,
    pushUndoSnapshot,
    segments,
    selectedSegmentIdx,
    t.editor.cannotDeleteLastMsg,
    t.editor.cannotDeleteLastTitle,
  ]);

  // Keyboard shortcuts: Space = Play/Pause, F = Fullscreen, S = Split, Del/Backspace = Remove segment, Ctrl+Z/Y = Undo/Redo, ←/→ = Seek
  useEffect(() => {
    if (!clip) return;
    const onKeyDown = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      const tag = target?.tagName;
      if (
        tag === "INPUT" ||
        tag === "TEXTAREA" ||
        tag === "SELECT" ||
        target?.isContentEditable
      ) {
        return;
      }

      if ((e.ctrlKey || e.metaKey) && !e.altKey) {
        if (e.code === "KeyZ" && !e.shiftKey) {
          e.preventDefault();
          handleUndo();
        } else if (
          e.code === "KeyY" ||
          (e.code === "KeyZ" && e.shiftKey)
        ) {
          e.preventDefault();
          handleRedo();
        }
        return;
      }

      if (e.altKey) return;

      if (e.code === "Space") {
        e.preventDefault();
        void togglePlay();
      } else if (e.code === "KeyF") {
        e.preventDefault();
        void toggleFullscreen();
      } else if (e.code === "KeyS") {
        e.preventDefault();
        handleSplitAtPlayhead();
      } else if (e.key === "Delete" || e.key === "Backspace") {
        e.preventDefault();
        handleDeleteSelectedSegment();
      } else if (e.code === "ArrowLeft") {
        e.preventDefault();
        const step = e.shiftKey ? 1 / 30 : 1;
        seekAll((videoRef.current?.currentTime ?? currentTime) - step);
      } else if (e.code === "ArrowRight") {
        e.preventDefault();
        const step = e.shiftKey ? 1 / 30 : 1;
        seekAll((videoRef.current?.currentTime ?? currentTime) + step);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [
    clip,
    currentTime,
    handleDeleteSelectedSegment,
    handleRedo,
    handleSplitAtPlayhead,
    handleUndo,
    seekAll,
    toggleFullscreen,
    togglePlay,
  ]);

  const handleVideoTimeUpdate = () => {
    const vid = videoRef.current;
    if (!vid) return;
    const t = vid.currentTime;
    setCurrentTime(t);

    if (stemsReady && !vid.paused) {
      for (const node of stemNodesRef.current.values()) {
        if (Math.abs(node.audio.currentTime - t) > 0.24) {
          node.audio.currentTime = t;
        }
      }
    }

    if (!vid.paused && segments.length > 0) {
      const sorted = [...segments].sort((a, b) => a.start_sec - b.start_sec);
      const currentSegIdx = sorted.findIndex(
        (s) => t >= s.start_sec - 0.05 && t <= s.end_sec
      );
      if (currentSegIdx === -1) {
        const nextSeg = sorted.find((s) => s.start_sec > t);
        if (nextSeg) {
          seekAll(nextSeg.start_sec);
        } else {
          vid.pause();
          pauseAllStems();
          setIsPlaying(false);
        }
      } else if (t >= sorted[currentSegIdx].end_sec - 0.03) {
        const nextSeg = sorted[currentSegIdx + 1];
        if (nextSeg) {
          seekAll(nextSeg.start_sec);
        } else {
          vid.pause();
          pauseAllStems();
          setIsPlaying(false);
        }
      }
    }
  };

  const totalKeptDurationSec = useMemo(() => {
    const raw = segments.reduce(
      (sum, s) => sum + Math.max(0, s.end_sec - s.start_sec),
      0
    );
    return Math.max(0.05, raw / Math.max(0.25, playbackSpeed));
  }, [segments, playbackSpeed]);

  const activeDiscordPreset: DiscordPreset = useMemo(() => {
    if (discordPresetKind === "mb10") return { preset: "mb10" };
    if (discordPresetKind === "mb50") return { preset: "mb50" };
    return { preset: "custom_mb", size_mb: Math.max(1, customDiscordMb) };
  }, [discordPresetKind, customDiscordMb]);

  const estimatedDiscordBitrateKbps = useMemo(() => {
    const mb =
      discordPresetKind === "mb10"
        ? 10
        : discordPresetKind === "mb50"
          ? 50
          : Math.max(1, customDiscordMb);
    const safeBytes = mb * 1024 * 1024 * 0.97;
    const totalKbps = (safeBytes * 8) / totalKeptDurationSec / 1000;
    return Math.max(100, Math.floor(totalKbps - 128));
  }, [discordPresetKind, customDiscordMb, totalKeptDurationSec]);

  const liveLevelForTrack = (kind: TrackKind, volume: number, muted: boolean) => {
    if (muted || !prepared) return 0;
    const wf = prepared.waveforms.find((w) => w.kind === kind);
    if (!wf || wf.peaks.length === 0) return 0;
    const frac = Math.min(0.999, Math.max(0, currentTime / Math.max(durationSec, 0.1)));
    const idx = Math.floor(frac * wf.peaks.length);
    const raw = wf.peaks[idx] ?? 0;
    return Math.min(100, Math.round(raw * volume * 100));
  };

  if (!clip) {
    return (
      <div className="flex h-full flex-col items-center justify-center p-8 text-center">
        <Film className="mb-3 h-12 w-12 text-slate-600" />
        <h2 className="text-base font-bold text-white">
          {t.editor.emptyTitle}
        </h2>
        <p className="mt-1 max-w-sm text-xs text-slate-400">
          {t.editor.emptyDesc}
        </p>
        <button
          type="button"
          onClick={onBackToLibrary}
          className="mt-4 inline-flex items-center gap-2 rounded-xl bg-[var(--accent)] px-4 py-2 text-xs font-semibold text-white hover:bg-[var(--accent-hover)]"
        >
          <ArrowLeft className="h-4 w-4" />
          {t.editor.goToLibrary}
        </button>
      </div>
    );
  }

  const gameTrack = trackMix.find((tr) => tr.kind === "audio_game");
  const micTrack = trackMix.find((tr) => tr.kind === "audio_mic");

  const toTrackMixSpec = (): TrackMixSpec[] =>
    trackMix.map((tr) => ({
      kind: tr.kind,
      volume: tr.volume,
      muted: tr.muted,
    }));

  const handleFastTrim = async () => {
    const seg = segments[selectedSegmentIdx] ?? segments[0];
    if (!seg) return;
    setBusyTask(t.editor.busyLosslessTrim);
    try {
      const outPath = await api.fastTrimClip({
        clip_id: clip.id,
        source_path: clip.file_path,
        output_path: deriveExportPath(
          clip.file_path,
          "trim",
          "mp4",
          clip.game_name,
          settings.storage.output_dir
        ),
        start_sec: seg.start_sec,
        end_sec: seg.end_sec,
        audio_mode: trimAudioMode,
        game_volume: gameTrack && !gameTrack.muted ? gameTrack.volume : 0,
        mic_volume: micTrack && !micTrack.muted ? micTrack.volume : 0,
      });
      setLastOutputPath(outPath);
      await onClipCreatedOrUpdated();
      onNotify(t.editor.trimDoneTitle, outPath, "success");
    } catch (err) {
      onNotify(t.editor.trimErrorTitle, String(err), "error");
    } finally {
      setBusyTask(null);
    }
  };

  const handleDiscordCompress = async () => {
    const sorted = [...segments].sort((a, b) => a.start_sec - b.start_sec);
    const first = sorted[0];
    const last = sorted[sorted.length - 1];
    const targetMbLabel =
      discordPresetKind === "mb10"
        ? t.editor.preset10Mb
        : discordPresetKind === "mb50"
          ? t.editor.preset50Mb
          : `${customDiscordMb} ${t.mbWord}`;
    setBusyTask(t.editor.busyDiscord(targetMbLabel));
    try {
      const outPath = await api.compressClipForDiscord(
        {
          clip_id: clip.id,
          source_path: clip.file_path,
          output_path: deriveExportPath(
            clip.file_path,
            "discord",
            "mp4",
            clip.game_name,
            settings.storage.output_dir
          ),
          preset: activeDiscordPreset,
          start_sec: first ? first.start_sec : 0,
          end_sec: last ? last.end_sec : durationSec,
          audio_bitrate_kbps: 128,
          mix_audio_tracks: true,
          game_volume: gameTrack && !gameTrack.muted ? gameTrack.volume : 0,
          mic_volume: micTrack && !micTrack.muted ? micTrack.volume : 0,
        },
        durationSec
      );
      setLastOutputPath(outPath);
      onNotify(t.editor.discordDoneTitle, outPath, "success");
      await onClipCreatedOrUpdated();
    } catch (err) {
      onNotify(t.editor.discordErrorTitle, String(err), "error");
    } finally {
      setBusyTask(null);
    }
  };

  const handleAdvancedExport = async (opts: {
    aspectOverride?: AspectRatioPreset;
    gif?: boolean;
    subclip?: boolean;
    withDiscordLimit?: boolean;
  }) => {
    const isGif = Boolean(opts.gif);
    const isSubclip = Boolean(opts.subclip);
    const aspect = opts.aspectOverride ?? aspectRatio;
    const suffix = isGif
      ? "gif"
      : isSubclip
        ? "subclip"
        : aspect === "vertical9x16_blurred"
          ? "shorts_9x16"
          : "edited";
    const ext = isGif ? "gif" : "mp4";

    setBusyTask(
      isGif
        ? t.editor.busyGif
        : isSubclip
          ? t.editor.busySubclip
          : t.editor.busyExport
    );

    try {
      const outPath = await api.exportAdvancedClip({
        clip_id: clip.id,
        source_path: clip.file_path,
        output_path: deriveExportPath(
          clip.file_path,
          suffix,
          ext,
          clip.game_name,
          settings.storage.output_dir
        ),
        segments,
        playback_speed: playbackSpeed,
        aspect_ratio: aspect,
        text_overlays: [],
        fade_in_sec: fadeInSec,
        fade_out_sec: fadeOutSec,
        track_mix: toTrackMixSpec(),
        discord_preset: opts.withDiscordLimit ? activeDiscordPreset : null,
        export_gif: isGif,
        subclip_from_session: isSubclip,
        discord_webhook_url: null,
      });
      setLastOutputPath(outPath);
      await onClipCreatedOrUpdated();
      onNotify(
        isGif
          ? t.editor.gifDoneTitle
          : isSubclip
            ? t.editor.subclipDoneTitle
            : t.editor.exportDoneTitle,
        outPath,
        "success"
      );
    } catch (err) {
      onNotify(t.editor.exportErrorTitle, String(err), "error");
    } finally {
      setBusyTask(null);
    }
  };

  const handleScreenshotPng = async () => {
    setBusyTask(t.editor.busyPng);
    try {
      const pngPath = await api.captureFramePng({
        clip_id: clip.id,
        source_path: clip.file_path,
        output_png_path: deriveExportPath(
          clip.file_path,
          "frame",
          "png",
          clip.game_name,
          settings.storage.output_dir
        ),
        timestamp_sec: currentTime,
      });
      setLastOutputPath(pngPath);
      onNotify(t.editor.pngDoneTitle, pngPath, "success");
    } catch (err) {
      onNotify(t.editor.pngErrorTitle, String(err), "error");
    } finally {
      setBusyTask(null);
    }
  };

  const handleCopyPathForDiscord = async (filePath: string) => {
    try {
      await api.copyClipToClipboard(filePath);
      onNotify(t.editor.copiedCtrlVTitle, filePath, "success");
    } catch (err) {
      onNotify(t.editor.copyErrorTitle, String(err), "error");
    }
  };

  const shortTrackLabel = (kind: TrackKind, fallback: string) => {
    switch (kind) {
      case "audio_game":
        return t.editor.gameTrackShort;
      case "audio_mic":
        return t.editor.micTrackShort;
      case "audio_secondary":
        return t.editor.secTrackShort;
      case "audio_mixed":
        return t.editor.mixTrackShort;
      default:
        return fallback;
    }
  };

  const trimAudioOptions: SelectOption[] = [
    { value: "keep_separate_tracks", label: t.editor.trimKeepSeparate },
    { value: "mix_to_single_track", label: t.editor.trimMixSingle },
    { value: "game_track_only", label: t.editor.trimGameOnly },
    { value: "mic_track_only", label: t.editor.trimMicOnly },
  ];

  return (
    <div className="flex h-full flex-col overflow-hidden bg-[var(--bg-app)]">
      {/* Slim Top Editor Header */}
      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-white/[0.06] bg-[var(--bg-panel)] px-4 py-2">
        <div className="flex items-center gap-3 min-w-0">
          <button
            type="button"
            onClick={onBackToLibrary}
            className="inline-flex shrink-0 items-center gap-1.5 rounded-xl bg-white/[0.05] px-3 py-1.5 text-xs font-medium text-slate-200 transition hover:bg-white/[0.1] hover:text-white"
          >
            <ArrowLeft className="h-3.5 w-3.5" />
            {t.editor.backToLibrary}
          </button>
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <h2 className="truncate text-sm font-bold text-white">
                {clip.title}
              </h2>
              <span className="shrink-0 rounded-md bg-[rgb(var(--accent-rgb)/0.16)] px-2 py-0.5 text-[10px] font-semibold text-[var(--accent-hover)]">
                {clip.game_name}
              </span>
            </div>
            <p className="truncate text-[11px] text-slate-400">
              {clip.width}x{clip.height} • {clip.fps} fps •{" "}
              {formatBytes(clip.file_size_bytes, lang)} •{" "}
              {stemsReady
                ? t.editor.stemsSynced
                : loadingMedia
                  ? t.editor.stemsLoading
                  : t.editor.singleAudio}
              {t.editor.shortcutsHint}
            </p>
          </div>
        </div>

        {/* Framing preset pills */}
        <div className="flex items-center gap-1 rounded-xl bg-white/[0.04] p-1">
          {(
            [
              ["original", t.editor.aspect16x9],
              ["vertical9x16_blurred", t.editor.aspect9x16],
              ["square1x1", t.editor.aspect1x1],
            ] as [AspectRatioPreset, string][]
          ).map(([preset, label]) => (
            <button
              key={preset}
              type="button"
              onClick={() => {
                if (aspectRatio !== preset) {
                  pushUndoSnapshot();
                  setAspectRatio(preset);
                }
              }}
              className={`inline-flex items-center gap-1.5 rounded-lg px-2.5 py-1 text-xs font-semibold transition ${
                aspectRatio === preset
                  ? "bg-[var(--accent)] text-white shadow-sm"
                  : "text-slate-400 hover:bg-white/[0.05] hover:text-slate-200"
              }`}
            >
              {preset === "vertical9x16_blurred" && (
                <Smartphone className="h-3.5 w-3.5" />
              )}
              {preset === "square1x1" && <Square className="h-3 w-3" />}
              {label}
            </button>
          ))}
        </div>
      </div>

      {/* Main Split Workspace */}
      <div className="flex flex-1 min-h-0 overflow-hidden">
        {/* Cinema Player + Sleek Timeline Column */}
        <div className="flex flex-1 min-w-0 flex-col gap-2.5 overflow-hidden p-3">
          {/* Borderless Cinema Player Wrapper + Single Unified Bottom HUD Bar */}
          <div
            ref={playerWrapperRef}
            className={`group relative flex min-h-0 flex-1 flex-col overflow-hidden bg-black ${
              isFullscreen
                ? "h-screen w-screen rounded-none"
                : "rounded-2xl shadow-[0_20px_60px_rgba(0,0,0,0.65)] ring-1 ring-white/[0.06]"
            }`}
          >
            <div className="relative flex min-h-0 flex-1 items-center justify-center overflow-hidden bg-black">
              <video
                ref={videoRef}
                src={toAssetUrl(clip.file_path)}
                muted={stemsReady}
                onPlay={() => {
                  setIsPlaying(true);
                  if (videoRef.current) {
                    void syncPlayAllStems(
                      videoRef.current.currentTime,
                      videoRef.current.playbackRate
                    );
                  }
                }}
                onPause={() => {
                  setIsPlaying(false);
                  pauseAllStems();
                }}
                onSeeked={() => {
                  if (videoRef.current) {
                    const tPos = videoRef.current.currentTime;
                    for (const node of stemNodesRef.current.values()) {
                      node.audio.currentTime = tPos;
                    }
                  }
                }}
                onRateChange={() => {
                  if (videoRef.current) {
                    const r = videoRef.current.playbackRate;
                    for (const node of stemNodesRef.current.values()) {
                      node.audio.playbackRate = r;
                    }
                  }
                }}
                onTimeUpdate={handleVideoTimeUpdate}
                onLoadedMetadata={(e) => {
                  const dur = e.currentTarget.duration;
                  if (Number.isFinite(dur) && dur > 0) {
                    setDurationSec((prev) => (prev <= 1 ? dur : prev));
                  }
                }}
                onEnded={() => {
                  setIsPlaying(false);
                  pauseAllStems();
                }}
                onClick={() => void togglePlay()}
                onDoubleClick={() => void toggleFullscreen()}
                className="h-full w-full cursor-pointer object-contain"
              />

              {aspectRatio === "vertical9x16_blurred" && (
                <div className="pointer-events-none absolute inset-0 flex items-center justify-center transition-all">
                  <div className="h-full flex-1 bg-black/65 backdrop-blur-sm" />
                  <div className="relative aspect-[9/16] h-full border-x-2 border-[var(--accent)] shadow-[0_0_24px_rgb(var(--accent-rgb)/0.35)]">
                    <span className="absolute left-2 top-2 rounded bg-[var(--accent)] px-1.5 py-0.5 text-[10px] font-bold text-white">
                      {t.editor.badge9x16}
                    </span>
                  </div>
                  <div className="h-full flex-1 bg-black/65 backdrop-blur-sm" />
                </div>
              )}

              {aspectRatio === "square1x1" && (
                <div className="pointer-events-none absolute inset-0 flex items-center justify-center transition-all">
                  <div className="h-full flex-1 bg-black/60 backdrop-blur-[2px]" />
                  <div className="relative aspect-square h-full border-x-2 border-violet-400">
                    <span className="absolute left-2 top-2 rounded bg-violet-600/85 px-1.5 py-0.5 text-[10px] font-bold text-white">
                      {t.editor.badge1x1}
                    </span>
                  </div>
                  <div className="h-full flex-1 bg-black/60 backdrop-blur-[2px]" />
                </div>
              )}
            </div>

            {/* Unified YouTube/Medal Bottom Control Bar */}
            <div
              onClick={(e) => e.stopPropagation()}
              className={`z-20 shrink-0 px-3.5 pb-2.5 pt-2 ${
                isFullscreen
                  ? "absolute inset-x-0 bottom-0 bg-gradient-to-t from-black/95 via-black/80 to-transparent pt-8"
                  : "border-t border-white/[0.06] bg-[var(--bg-panel)]/95 backdrop-blur-md"
              }`}
            >
              {/* Scrubber bar */}
              <input
                type="range"
                min={0}
                max={Math.max(durationSec, 0.1)}
                step={0.05}
                value={Math.min(currentTime, durationSec)}
                onChange={(e) => seekAll(Number(e.target.value))}
                style={sliderFillStyle(
                  Math.min(currentTime, durationSec),
                  0,
                  Math.max(durationSec, 0.1)
                )}
                className="ikoch-slider mb-2 w-full"
              />

              <div className="flex flex-wrap items-center justify-between gap-2">
                {/* Transport & Time */}
                <div className="flex items-center gap-2.5">
                  <button
                    type="button"
                    onClick={() => void togglePlay()}
                    title={t.editor.playPauseHint}
                    className="flex h-8 w-8 items-center justify-center rounded-lg bg-[var(--accent)] text-white shadow-sm transition hover:bg-[var(--accent-hover)]"
                  >
                    {isPlaying ? (
                      <Pause className="h-4 w-4 fill-current" />
                    ) : (
                      <Play className="ml-0.5 h-4 w-4 fill-current" />
                    )}
                  </button>

                  <div className="font-mono text-xs text-slate-200">
                    {formatTimePrecise(currentTime)}
                    <span className="text-slate-500">
                      {" "}
                      / {formatTimePrecise(durationSec)}
                    </span>
                  </div>

                  <span className="rounded-md bg-emerald-500/15 px-2 py-0.5 text-[11px] font-semibold text-emerald-300">
                    {t.editor.keptDuration(totalKeptDurationSec.toFixed(1))}
                  </span>
                </div>

                {/* Inline Track Audio Sliders */}
                <div className="flex flex-wrap items-center gap-1.5">
                  {trackMix.length > 0 ? (
                    trackMix.map((track) => {
                      const meterPct = liveLevelForTrack(
                        track.kind,
                        track.volume,
                        track.muted
                      );
                      const meterColor =
                        track.kind === "audio_mic"
                          ? "bg-emerald-400"
                          : track.kind === "audio_secondary"
                            ? "bg-violet-400"
                            : "bg-sky-400";

                      return (
                        <div
                          key={track.kind}
                          className="relative flex items-center gap-1.5 overflow-hidden rounded-lg bg-white/[0.05] px-2 py-1"
                          title={`${trackDisplayName(track.kind, track.label, lang)}: ${
                            track.muted
                              ? t.editor.mutedShort
                              : `${Math.round(track.volume * 100)}%`
                          }`}
                        >
                          <button
                            type="button"
                            onClick={() =>
                              setTrackMix((prev) =>
                                prev.map((tr) =>
                                  tr.kind === track.kind
                                    ? { ...tr, muted: !tr.muted }
                                    : tr
                                )
                              )
                            }
                            title={
                              track.muted
                                ? t.editor.unmuteHint
                                : t.editor.muteHint
                            }
                            className={`transition ${
                              track.muted
                                ? "text-rose-400"
                                : "text-slate-300 hover:text-white"
                            }`}
                          >
                            {track.muted ? (
                              <VolumeX className="h-3.5 w-3.5" />
                            ) : (
                              <Volume2 className="h-3.5 w-3.5" />
                            )}
                          </button>
                          <span className="text-[11px] font-medium text-slate-300">
                            {shortTrackLabel(track.kind, track.label)}
                          </span>
                          <input
                            type="range"
                            min={0}
                            max={2}
                            step={0.05}
                            value={track.volume}
                            disabled={track.muted}
                            onChange={(e) => {
                              const v = Number(e.target.value);
                              setTrackMix((prev) =>
                                prev.map((tr) =>
                                  tr.kind === track.kind
                                    ? { ...tr, volume: v }
                                    : tr
                                )
                              );
                            }}
                            style={sliderFillStyle(
                              track.muted ? 0 : track.volume,
                              0,
                              2
                            )}
                            className="ikoch-slider w-14"
                          />
                          <div
                            className={`absolute bottom-0 left-0 h-[2px] transition-all duration-75 ${meterColor}`}
                            style={{ width: `${meterPct}%` }}
                          />
                        </div>
                      );
                    })
                  ) : (
                    <div className="flex items-center gap-1.5 rounded-lg bg-white/[0.05] px-2 py-1">
                      <Volume2 className="h-3.5 w-3.5 text-slate-300" />
                      <input
                        type="range"
                        min={0}
                        max={1}
                        step={0.05}
                        value={masterVolume}
                        onChange={(e) =>
                          setMasterVolume(Number(e.target.value))
                        }
                        title={t.editor.masterVolumeHint}
                        style={sliderFillStyle(masterVolume, 0, 1)}
                        className="ikoch-slider w-16"
                      />
                    </div>
                  )}
                </div>

                {/* Speed Pills + Fullscreen */}
                <div className="flex items-center gap-1">
                  {[0.25, 0.5, 1, 1.5, 2, 4].map((spd) => (
                    <button
                      key={spd}
                      type="button"
                      onClick={() => {
                        if (playbackSpeed !== spd) {
                          pushUndoSnapshot();
                          setPlaybackSpeed(spd);
                        }
                      }}
                      className={`rounded-md px-1.5 py-1 font-mono text-[11px] font-semibold transition ${
                        playbackSpeed === spd
                          ? "bg-[var(--accent)] text-white"
                          : "bg-white/[0.05] text-slate-400 hover:bg-white/[0.1] hover:text-white"
                      }`}
                    >
                      {spd}x
                    </button>
                  ))}

                  <button
                    type="button"
                    onClick={() => void toggleFullscreen()}
                    title={
                      isFullscreen
                        ? t.editor.fullscreenExit
                        : t.editor.fullscreenEnter
                    }
                    className="ml-1 flex h-8 w-8 items-center justify-center rounded-lg bg-white/[0.06] text-slate-200 transition hover:bg-[var(--accent)] hover:text-white"
                  >
                    {isFullscreen ? (
                      <Minimize2 className="h-4 w-4" />
                    ) : (
                      <Maximize2 className="h-4 w-4" />
                    )}
                  </button>
                </div>
              </div>
            </div>
          </div>

          {/* Interactive Waveform Timeline (no duplicate inspector row) */}
          <TimelinePanel
            durationSec={durationSec}
            currentTime={currentTime}
            markers={clip.markers}
            waveforms={prepared?.waveforms ?? []}
            segments={segments}
            selectedSegmentIdx={selectedSegmentIdx}
            loadingMedia={loadingMedia}
            canUndo={undoStack.length > 0}
            canRedo={redoStack.length > 0}
            onUndo={handleUndo}
            onRedo={handleRedo}
            onInteractionStart={pushUndoSnapshot}
            onSeek={seekAll}
            onSelectSegment={setSelectedSegmentIdx}
            onChangeSegments={setSegments}
            onSplitAtPlayhead={handleSplitAtPlayhead}
          />
        </div>

        {/* Compact 2-Tab Right Sidebar (~292px) */}
        <aside className="flex w-[292px] shrink-0 flex-col gap-3 overflow-y-auto border-l border-white/[0.06] bg-[var(--bg-panel)] p-3.5">
          {/* Busy Progress Card */}
          {busyTask && (
            <div className="rounded-xl bg-[rgb(var(--accent-rgb)/0.15)] p-3 ring-1 ring-[rgb(var(--accent-rgb)/0.35)]">
              <div className="flex items-center justify-between text-xs font-semibold text-white">
                <span className="flex items-center gap-2 truncate">
                  <Loader2 className="h-4 w-4 shrink-0 animate-spin text-[var(--accent-hover)]" />
                  <span className="truncate">{busyTask}</span>
                </span>
                <span className="shrink-0 font-mono text-[11px] text-[var(--accent-hover)]">
                  {busyElapsedSec.toFixed(1)}
                  {t.secShort}
                </span>
              </div>
              <div className="mt-2 h-1.5 w-full overflow-hidden rounded-full bg-black/40">
                <div className="h-full w-2/3 animate-pulse rounded-full bg-gradient-to-r from-[var(--accent)] to-[var(--accent-hover)]" />
              </div>
            </div>
          )}

          {/* Last Exported Result Actions */}
          {lastOutputPath && (
            <div className="rounded-xl bg-emerald-500/10 p-3 ring-1 ring-emerald-500/25">
              <div className="text-xs font-bold text-emerald-300">
                {t.editor.fileReady}
              </div>
              <div
                className="mt-0.5 truncate font-mono text-[11px] text-slate-300"
                title={lastOutputPath}
              >
                {lastOutputPath}
              </div>
              <div className="mt-2 flex flex-wrap gap-1.5">
                <button
                  type="button"
                  onClick={() => void handleCopyPathForDiscord(lastOutputPath)}
                  className="inline-flex flex-1 items-center justify-center gap-1 rounded-lg bg-emerald-600 px-2.5 py-1.5 text-[11px] font-semibold text-white transition hover:bg-emerald-500"
                >
                  <ClipboardCopy className="h-3.5 w-3.5" />
                  {t.library.copyCtrlV}
                </button>
                <button
                  type="button"
                  onClick={onOpenFolder}
                  className="rounded-lg bg-white/[0.06] px-2.5 py-1.5 text-[11px] text-slate-200 hover:bg-white/[0.12]"
                >
                  {t.editor.folderBtn}
                </button>
              </div>
            </div>
          )}

          {/* 2-Tab Switcher: Quick | Export */}
          <div className="grid grid-cols-2 rounded-xl bg-white/[0.04] p-1">
            <button
              type="button"
              onClick={() => setSidebarTab("quick")}
              className={`rounded-lg py-1.5 text-xs font-semibold transition ${
                sidebarTab === "quick"
                  ? "bg-[var(--accent)] text-white shadow-sm"
                  : "text-slate-400 hover:text-white"
              }`}
            >
              {t.editor.tabQuick}
            </button>
            <button
              type="button"
              onClick={() => setSidebarTab("export")}
              className={`rounded-lg py-1.5 text-xs font-semibold transition ${
                sidebarTab === "export"
                  ? "bg-[var(--accent)] text-white shadow-sm"
                  : "text-slate-400 hover:text-white"
              }`}
            >
              {t.editor.tabExport}
            </button>
          </div>

          {sidebarTab === "quick" ? (
            <div className="space-y-3">
              {/* Lossless Trim & Sub-clip */}
              <div className="space-y-2.5 rounded-2xl bg-white/[0.03] p-3 ring-1 ring-white/[0.06]">
                <div className="text-[11px] font-bold uppercase tracking-wider text-slate-400">
                  {t.editor.trimAndSubclip}
                </div>

                <CustomSelect
                  value={trimAudioMode}
                  options={trimAudioOptions}
                  onChange={(val) =>
                    setTrimAudioMode(val as TrimAudioExportMode)
                  }
                  ariaLabel={t.editor.trimAndSubclip}
                />

                <button
                  type="button"
                  disabled={Boolean(busyTask)}
                  onClick={() => void handleFastTrim()}
                  className="flex w-full items-center justify-center gap-2 rounded-xl bg-emerald-600/90 py-2 text-xs font-semibold text-white transition hover:bg-emerald-500 disabled:opacity-40"
                >
                  <Scissors className="h-3.5 w-3.5" />
                  {t.editor.trimLosslessBtn}
                </button>

                <button
                  type="button"
                  disabled={Boolean(busyTask)}
                  onClick={() => void handleAdvancedExport({ subclip: true })}
                  className="flex w-full items-center justify-center gap-2 rounded-xl bg-white/[0.06] py-2 text-xs font-semibold text-violet-300 transition hover:bg-violet-600 hover:text-white disabled:opacity-40"
                >
                  <Film className="h-3.5 w-3.5" />
                  {t.editor.subclipBtn}
                </button>
              </div>

              {/* Compress for Discord */}
              <div className="space-y-2.5 rounded-2xl bg-white/[0.03] p-3 ring-1 ring-white/[0.06]">
                <div className="flex items-center justify-between">
                  <span className="text-[11px] font-bold uppercase tracking-wider text-slate-400">
                    {t.editor.forDiscord}
                  </span>
                  <span className="font-mono text-[10px] text-slate-400">
                    ~{estimatedDiscordBitrateKbps} kbps
                  </span>
                </div>

                <div className="flex gap-1.5">
                  {(
                    [
                      ["mb10", t.editor.preset10Mb],
                      ["mb50", t.editor.preset50Mb],
                      ["custom_mb", t.editor.presetCustom],
                    ] as const
                  ).map(([presetId, label]) => (
                    <button
                      key={presetId}
                      type="button"
                      onClick={() => setDiscordPresetKind(presetId)}
                      className={`flex-1 rounded-lg py-1.5 text-xs font-semibold transition ${
                        discordPresetKind === presetId
                          ? "bg-[#5865F2] text-white"
                          : "bg-white/[0.05] text-slate-400 hover:text-white"
                      }`}
                    >
                      {label}
                    </button>
                  ))}
                </div>

                {discordPresetKind === "custom_mb" && (
                  <div className="flex items-center gap-2">
                    <input
                      type="number"
                      min={2}
                      max={500}
                      value={customDiscordMb}
                      onChange={(e) =>
                        setCustomDiscordMb(
                          Math.max(1, Number(e.target.value) || 10)
                        )
                      }
                      className="w-20 rounded-lg bg-black/40 px-2.5 py-1 text-xs text-white ring-1 ring-white/[0.08]"
                    />
                    <span className="text-xs text-slate-400">
                      {t.editor.mbLimit}
                    </span>
                  </div>
                )}

                <button
                  type="button"
                  disabled={Boolean(busyTask)}
                  onClick={() => void handleDiscordCompress()}
                  className="flex w-full items-center justify-center gap-2 rounded-xl bg-[#5865F2] py-2 text-xs font-semibold text-white transition hover:bg-[#4752C4] disabled:opacity-40"
                >
                  <Download className="h-3.5 w-3.5" />
                  {t.editor.compressDiscordBtn}
                </button>

                <button
                  type="button"
                  onClick={() => void handleCopyPathForDiscord(clip.file_path)}
                  className="flex w-full items-center justify-center gap-1.5 rounded-xl bg-white/[0.05] py-1.5 text-[11px] font-medium text-slate-300 transition hover:bg-white/[0.1] hover:text-white"
                >
                  <ClipboardCopy className="h-3.5 w-3.5" />
                  {t.editor.copySourceCtrlV}
                </button>
              </div>
            </div>
          ) : (
            <div className="space-y-3">
              {/* Full Export */}
              <div className="space-y-2 rounded-2xl bg-white/[0.03] p-3 ring-1 ring-white/[0.06]">
                <div className="text-[11px] font-bold uppercase tracking-wider text-slate-400">
                  {t.editor.renderAndFormats}
                </div>

                <button
                  type="button"
                  disabled={Boolean(busyTask)}
                  onClick={() => void handleAdvancedExport({})}
                  className="flex w-full items-center justify-center gap-2 rounded-xl bg-[var(--accent)] py-2.5 text-xs font-semibold text-white shadow transition hover:bg-[var(--accent-hover)] disabled:opacity-40"
                >
                  <Film className="h-3.5 w-3.5" />
                  {t.editor.exportMp4Hq}
                </button>

                <button
                  type="button"
                  disabled={Boolean(busyTask)}
                  onClick={() =>
                    void handleAdvancedExport({
                      aspectOverride: "vertical9x16_blurred",
                    })
                  }
                  className="flex w-full items-center justify-center gap-2 rounded-xl bg-white/[0.06] py-2 text-xs font-semibold text-[var(--accent-hover)] transition hover:bg-[var(--accent)] hover:text-white disabled:opacity-40"
                >
                  <Smartphone className="h-3.5 w-3.5" />
                  {t.editor.exportShorts}
                </button>

                <div className="grid grid-cols-2 gap-2">
                  <button
                    type="button"
                    disabled={Boolean(busyTask)}
                    onClick={() => void handleAdvancedExport({ gif: true })}
                    className="flex items-center justify-center gap-1.5 rounded-xl bg-white/[0.05] py-2 text-xs font-semibold text-amber-300 transition hover:bg-amber-500/20 disabled:opacity-40"
                  >
                    {t.editor.exportGif}
                  </button>
                  <button
                    type="button"
                    disabled={Boolean(busyTask)}
                    onClick={() => void handleScreenshotPng()}
                    className="flex items-center justify-center gap-1.5 rounded-xl bg-white/[0.05] py-2 text-xs font-semibold text-slate-200 transition hover:bg-white/[0.1] disabled:opacity-40"
                  >
                    <Camera className="h-3.5 w-3.5" />
                    {t.editor.capturePng}
                  </button>
                </div>
              </div>

              {/* Fade In / Fade Out */}
              <div className="space-y-2.5 rounded-2xl bg-white/[0.03] p-3 ring-1 ring-white/[0.06]">
                <div className="text-[11px] font-bold uppercase tracking-wider text-slate-400">
                  {t.editor.fadeTitle}
                </div>
                <div className="grid grid-cols-2 gap-2.5">
                  <div className="text-[11px] text-slate-400">
                    <div>{t.editor.fadeIn(fadeInSec.toFixed(1))}</div>
                    <input
                      type="range"
                      min={0}
                      max={3}
                      step={0.1}
                      value={fadeInSec}
                      onPointerDown={pushUndoSnapshot}
                      onChange={(e) => setFadeInSec(Number(e.target.value))}
                      style={sliderFillStyle(fadeInSec, 0, 3)}
                      className="ikoch-slider mt-1.5 w-full"
                    />
                  </div>
                  <div className="text-[11px] text-slate-400">
                    <div>{t.editor.fadeOut(fadeOutSec.toFixed(1))}</div>
                    <input
                      type="range"
                      min={0}
                      max={3}
                      step={0.1}
                      value={fadeOutSec}
                      onPointerDown={pushUndoSnapshot}
                      onChange={(e) => setFadeOutSec(Number(e.target.value))}
                      style={sliderFillStyle(fadeOutSec, 0, 3)}
                      className="ikoch-slider mt-1.5 w-full"
                    />
                  </div>
                </div>
              </div>
            </div>
          )}
        </aside>
      </div>
    </div>
  );
}
