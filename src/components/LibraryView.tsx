import { useEffect, useMemo, useRef, useState } from "react";
import {
  Bookmark,
  Check,
  ClipboardCopy,
  Edit3,
  Film,
  FolderOpen,
  FolderSearch,
  Gamepad2,
  Monitor,
  Pencil,
  Play,
  RefreshCw,
  Scissors,
  Search,
  Settings,
  Star,
  Trash2,
  X,
} from "lucide-react";
import { api, toAssetUrl } from "../api";
import { useI18n } from "../i18n";
import type { ClipMetadata, RecordingKind } from "../types";
import { formatBytes, formatDate, formatDuration } from "../utils/format";
import { CustomSelect } from "./CustomSelect";

type KindTab = "all" | RecordingKind | "favorites";
type SortOrder = "newest" | "oldest" | "longest" | "largest";

interface LibraryViewProps {
  clips: ClipMetadata[];
  loading: boolean;
  onRefresh: () => Promise<void>;
  onOpenEditor: (clip: ClipMetadata) => void;
  onClipUpdated: (clip: ClipMetadata) => void;
  onClipDeleted: (filePath: string) => void;
  onOpenFolder: () => void;
  onSelectMonitorOne: () => void;
  onOpenSettings: () => void;
  onNotify: (
    title: string,
    message: string,
    variant?: "success" | "error" | "info"
  ) => void;
}

export function LibraryView({
  clips,
  loading,
  onRefresh,
  onOpenEditor,
  onClipUpdated,
  onClipDeleted,
  onOpenFolder,
  onSelectMonitorOne,
  onOpenSettings,
  onNotify,
}: LibraryViewProps) {
  const { lang, t } = useI18n();
  const [kindTab, setKindTab] = useState<KindTab>("all");
  const [gameFilter, setGameFilter] = useState<string>("all");
  const [searchQuery, setSearchQuery] = useState("");
  const [sortOrder, setSortOrder] = useState<SortOrder>("newest");

  const [hoveredCardPath, setHoveredCardPath] = useState<string | null>(null);
  const [copiedPath, setCopiedPath] = useState<string | null>(null);
  const [renamingPath, setRenamingPath] = useState<string | null>(null);
  const [renameValue, setRenameValue] = useState("");
  const [confirmDeletePath, setConfirmDeletePath] = useState<string | null>(
    null
  );
  const [contextMenu, setContextMenu] = useState<{
    clip: ClipMetadata;
    x: number;
    y: number;
    confirmDelete: boolean;
  } | null>(null);
  const [generatedThumbs, setGeneratedThumbs] = useState<
    Record<string, string>
  >({});
  const attemptedThumbsRef = useRef<Set<string>>(new Set());

  useEffect(() => {
    if (!contextMenu) return;
    const close = () => setContextMenu(null);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setContextMenu(null);
    };
    window.addEventListener("click", close);
    window.addEventListener("blur", close);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("click", close);
      window.removeEventListener("blur", close);
      window.removeEventListener("keydown", onKey);
    };
  }, [contextMenu]);

  // Automatically generate missing thumbnails sequentially in the background.
  useEffect(() => {
    let cancelled = false;
    const missing = clips.filter(
      (c) =>
        !c.thumbnail_path &&
        !generatedThumbs[c.file_path] &&
        !attemptedThumbsRef.current.has(c.file_path)
    );
    if (missing.length === 0) return;

    (async () => {
      for (const item of missing.slice(0, 6)) {
        if (cancelled) break;
        attemptedThumbsRef.current.add(item.file_path);
        try {
          const prep = await api.prepareEditorMedia(item.file_path, 48);
          if (cancelled) break;
          if (prep.thumbnail_path) {
            setGeneratedThumbs((prev) => ({
              ...prev,
              [item.file_path]: prep.thumbnail_path,
            }));
          }
        } catch {
          // Video fallback handles unreadable files gracefully.
        }
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [clips, generatedThumbs]);

  const uniqueGames = useMemo(() => {
    const set = new Set<string>();
    for (const c of clips) {
      if (c.game_name?.trim()) set.add(c.game_name.trim());
    }
    return Array.from(set).sort((a, b) =>
      a.localeCompare(b, lang === "en" ? "en" : "ru")
    );
  }, [clips, lang]);

  const filteredClips = useMemo(() => {
    const q = searchQuery.trim().toLowerCase();
    const filtered = clips.filter((clip) => {
      if (kindTab === "favorites" && !clip.favorite) return false;
      if (kindTab === "clip" && clip.kind !== "clip") return false;
      if (kindTab === "session" && clip.kind !== "session") return false;
      if (gameFilter !== "all" && clip.game_name !== gameFilter) return false;
      if (q) {
        const hay = `${clip.title} ${clip.game_name} ${clip.tags.join(
          " "
        )}`.toLowerCase();
        if (!hay.includes(q)) return false;
      }
      return true;
    });

    filtered.sort((a, b) => {
      switch (sortOrder) {
        case "oldest":
          return (
            new Date(a.created_at).getTime() - new Date(b.created_at).getTime()
          );
        case "longest":
          return b.duration_sec - a.duration_sec;
        case "largest":
          return b.file_size_bytes - a.file_size_bytes;
        case "newest":
        default:
          return (
            new Date(b.created_at).getTime() - new Date(a.created_at).getTime()
          );
      }
    });

    return filtered;
  }, [clips, kindTab, gameFilter, searchQuery, sortOrder]);

  const handleCopyForDiscord = async (clip: ClipMetadata) => {
    try {
      await api.copyClipToClipboard(clip.file_path);
      setCopiedPath(clip.file_path);
      window.setTimeout(() => {
        setCopiedPath((prev) => (prev === clip.file_path ? null : prev));
      }, 2200);
      onNotify(
        t.library.copiedTitle,
        t.library.copiedMsg(clip.title),
        "success"
      );
    } catch (err) {
      onNotify(t.library.copyError, String(err), "error");
    }
  };

  const handleToggleFavorite = async (clip: ClipMetadata) => {
    try {
      const updated = await api.toggleClipFavorite(clip.file_path);
      onClipUpdated(updated);
    } catch (err) {
      onNotify(t.library.favError, String(err), "error");
    }
  };

  const handleSubmitRename = async (clip: ClipMetadata) => {
    const trimmed = renameValue.trim();
    if (!trimmed) return;
    try {
      const updated = await api.renameClip(clip.file_path, trimmed);
      onClipUpdated(updated);
      setRenamingPath(null);
      onNotify(t.library.renamedTitle, updated.title, "success");
    } catch (err) {
      onNotify(t.library.renameError, String(err), "error");
    }
  };

  const handleDelete = async (clip: ClipMetadata) => {
    try {
      await api.deleteClip(clip.file_path);
      onClipDeleted(clip.file_path);
      setConfirmDeletePath(null);
      setContextMenu(null);
      onNotify(t.library.deletedTitle, clip.title, "info");
    } catch (err) {
      onNotify(t.library.deleteError, String(err), "error");
    }
  };

  const handleRevealInExplorer = async (clip: ClipMetadata) => {
    try {
      await api.revealClipInExplorer(clip.file_path);
    } catch (err) {
      onNotify(t.library.revealError, String(err), "error");
    }
  };

  const gameOptions = [
    { value: "all", label: t.library.allGames },
    ...uniqueGames.map((g) => ({ value: g, label: g })),
  ];

  const sortOptions: { value: SortOrder; label: string }[] = [
    { value: "newest", label: t.library.sortNewest },
    { value: "oldest", label: t.library.sortOldest },
    { value: "longest", label: t.library.sortLongest },
    { value: "largest", label: t.library.sortLargest },
  ];

  return (
    <div className="relative flex h-full flex-col overflow-hidden bg-[var(--bg-app)]">
      {/* Header & Filter Bar */}
      <div className="border-b border-white/[0.06] bg-[var(--bg-panel)]/95 px-6 py-4 backdrop-blur-md">
        <div className="flex flex-wrap items-center justify-between gap-4">
          <div>
            <h1 className="text-lg font-bold tracking-tight text-[var(--text-primary)]">
              {t.library.title}
            </h1>
            <p className="text-xs text-slate-400">
              {t.library.stats(clips.length, filteredClips.length)}
            </p>
          </div>

          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={onOpenFolder}
              title={t.library.openFolderHint}
              className="inline-flex items-center gap-1.5 rounded-xl bg-white/[0.05] px-3 py-1.5 text-xs font-medium text-slate-200 transition hover:bg-white/[0.1] hover:text-white"
            >
              <FolderOpen className="h-3.5 w-3.5 text-amber-400" />
              {t.library.openFolder}
            </button>
            <button
              type="button"
              onClick={() => void onRefresh()}
              disabled={loading}
              title={t.library.refreshHint}
              className="inline-flex items-center gap-1.5 rounded-xl bg-white/[0.05] px-3 py-1.5 text-xs font-medium text-slate-200 transition hover:bg-white/[0.1] hover:text-white disabled:opacity-50"
            >
              <RefreshCw
                className={`h-3.5 w-3.5 ${loading ? "animate-spin" : ""}`}
              />
              {t.library.refresh}
            </button>
          </div>
        </div>

        {/* Filter Controls */}
        <div className="mt-4 flex flex-wrap items-center gap-2.5">
          <div className="inline-flex rounded-xl bg-white/[0.04] p-1">
            {(
              [
                ["all", t.library.tabAll],
                ["clip", t.library.tabClips],
                ["session", t.library.tabSessions],
                ["favorites", t.library.tabFavorites],
              ] as [KindTab, string][]
            ).map(([tabId, label]) => (
              <button
                key={tabId}
                type="button"
                onClick={() => setKindTab(tabId)}
                className={`rounded-lg px-3 py-1 text-xs font-semibold transition ${
                  kindTab === tabId
                    ? "bg-[var(--accent)] text-white shadow-sm"
                    : "text-slate-400 hover:text-slate-200"
                }`}
              >
                {label}
              </button>
            ))}
          </div>

          <div className="relative min-w-[200px] max-w-xs flex-1">
            <Search className="pointer-events-none absolute left-3 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-slate-500" />
            <input
              type="text"
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              placeholder={t.library.searchPlaceholder}
              className="w-full rounded-xl bg-white/[0.04] py-1.5 pl-8 pr-3 text-xs text-slate-100 placeholder-slate-500 ring-1 ring-white/[0.06] outline-none transition focus:bg-white/[0.06] focus:ring-[rgb(var(--accent-rgb)/0.6)]"
            />
          </div>

          <div className="flex items-center rounded-xl bg-white/[0.04] ring-1 ring-white/[0.06]">
            <Gamepad2 className="ml-2.5 h-3.5 w-3.5 shrink-0 text-slate-400" />
            <CustomSelect
              variant="embedded"
              value={gameFilter}
              options={gameOptions}
              onChange={setGameFilter}
              className="w-36"
              menuMinWidth={180}
            />
          </div>

          <CustomSelect
            value={sortOrder}
            options={sortOptions}
            onChange={(val) => setSortOrder(val as SortOrder)}
            className="w-40"
            menuMinWidth={170}
          />
        </div>
      </div>

      {/* Cards Grid */}
      <div
        onScroll={() => {
          if (contextMenu) setContextMenu(null);
        }}
        className="flex-1 overflow-y-auto p-6"
      >
        {filteredClips.length === 0 ? (
          <div className="flex h-80 flex-col items-center justify-center rounded-2xl border border-dashed border-white/[0.08] bg-white/[0.02] p-8 text-center">
            <div className="mb-3 flex h-12 w-12 items-center justify-center rounded-2xl bg-white/[0.05]">
              <Film className="h-6 w-6 text-[var(--accent-hover)]" />
            </div>
            <div className="text-sm font-bold text-white">
              {t.library.emptyTitle}
            </div>
            <p className="mt-1 max-w-md text-xs leading-relaxed text-slate-400">
              {t.library.emptyDesc}
            </p>
            <div className="mt-5 flex flex-wrap items-center justify-center gap-2.5">
              <button
                type="button"
                onClick={onSelectMonitorOne}
                className="inline-flex items-center gap-2 rounded-xl bg-[var(--accent)] px-4 py-2 text-xs font-semibold text-white shadow-lg transition hover:bg-[var(--accent-hover)]"
              >
                <Monitor className="h-3.5 w-3.5" />
                {t.library.enableScreenCapture}
              </button>
              <button
                type="button"
                onClick={onOpenSettings}
                className="inline-flex items-center gap-2 rounded-xl bg-white/[0.06] px-4 py-2 text-xs font-semibold text-slate-200 transition hover:bg-white/[0.1] hover:text-white"
              >
                <Settings className="h-3.5 w-3.5" />
                {t.library.goSettings}
              </button>
            </div>
          </div>
        ) : (
          <div className="grid grid-cols-1 gap-5 md:grid-cols-2 xl:grid-cols-3">
            {filteredClips.map((clip) => {
              const thumbSrc =
                clip.thumbnail_path || generatedThumbs[clip.file_path] || null;
              const isHovered = hoveredCardPath === clip.file_path;
              const isCopied = copiedPath === clip.file_path;
              const isRenaming = renamingPath === clip.file_path;
              const isConfirmDelete = confirmDeletePath === clip.file_path;

              return (
                <div
                  key={clip.id || clip.file_path}
                  onMouseEnter={() => setHoveredCardPath(clip.file_path)}
                  onMouseLeave={() => {
                    setHoveredCardPath((prev) =>
                      prev === clip.file_path ? null : prev
                    );
                    setConfirmDeletePath((prev) =>
                      prev === clip.file_path ? null : prev
                    );
                  }}
                  onContextMenu={(e) => {
                    e.preventDefault();
                    e.stopPropagation();
                    const x = Math.min(e.clientX, window.innerWidth - 236);
                    const y = Math.min(e.clientY, window.innerHeight - 260);
                    setContextMenu({
                      clip,
                      x: Math.max(8, x),
                      y: Math.max(8, y),
                      confirmDelete: false,
                    });
                  }}
                  className="group flex flex-col overflow-hidden rounded-2xl bg-white/[0.025] ring-1 ring-white/[0.05] transition duration-200 hover:-translate-y-0.5 hover:bg-white/[0.04] hover:ring-[rgb(var(--accent-rgb)/0.55)] hover:shadow-[0_18px_40px_rgba(0,0,0,0.55)]"
                >
                  {/* Large 16:9 Thumbnail & Live Hover Video Preview */}
                  <div
                    onClick={() => onOpenEditor(clip)}
                    className="relative aspect-video cursor-pointer overflow-hidden bg-black"
                  >
                    {thumbSrc ? (
                      <img
                        src={toAssetUrl(thumbSrc)}
                        alt={clip.title}
                        loading="lazy"
                        className="h-full w-full object-cover transition duration-300 group-hover:scale-[1.03]"
                      />
                    ) : (
                      <video
                        src={toAssetUrl(clip.file_path)}
                        preload="metadata"
                        muted
                        className="h-full w-full object-cover"
                      />
                    )}

                    {isHovered && (
                      <video
                        src={toAssetUrl(clip.file_path)}
                        autoPlay
                        muted
                        loop
                        playsInline
                        className="absolute inset-0 h-full w-full object-cover"
                      />
                    )}

                    {/* Top-left Status Badges */}
                    <div className="pointer-events-none absolute left-3 top-3 flex flex-wrap gap-1.5">
                      {clip.kind === "session" && (
                        <span className="rounded-md bg-violet-600/90 px-2 py-0.5 text-[10px] font-bold uppercase tracking-wide text-white shadow">
                          {t.library.sessionBadge}
                        </span>
                      )}
                      {clip.favorite && (
                        <span className="inline-flex items-center gap-1 rounded-md bg-amber-500/95 px-2 py-0.5 text-[10px] font-bold text-slate-950 shadow">
                          <Star className="h-2.5 w-2.5 fill-current" />
                        </span>
                      )}
                      {clip.is_edited && (
                        <span
                          className="inline-flex items-center gap-1 rounded-md bg-emerald-500/95 px-2 py-0.5 text-[10px] font-bold text-slate-950 shadow"
                          title={t.library.editedHint}
                        >
                          <Scissors className="h-2.5 w-2.5" />
                        </span>
                      )}
                    </div>

                    {clip.markers.length > 0 && (
                      <div className="pointer-events-none absolute right-3 top-3 flex items-center gap-1 rounded-md bg-black/70 px-2 py-0.5 text-[10px] font-semibold text-amber-300 backdrop-blur-sm">
                        <Bookmark className="h-2.5 w-2.5 fill-current" />
                        {clip.markers.length}
                      </div>
                    )}

                    {/* Hover Floating Quick Actions Bar */}
                    <div
                      onClick={(e) => e.stopPropagation()}
                      className="absolute inset-x-3 bottom-9 flex translate-y-1 items-center justify-center gap-1.5 opacity-0 transition duration-200 group-hover:translate-y-0 group-hover:opacity-100"
                    >
                      <button
                        type="button"
                        onClick={() => onOpenEditor(clip)}
                        className="inline-flex items-center gap-1.5 rounded-xl bg-[var(--accent)] px-3 py-1.5 text-xs font-semibold text-white shadow-lg backdrop-blur-md transition hover:bg-[var(--accent-hover)]"
                      >
                        <Play className="h-3 w-3 fill-current" />
                        {t.library.editBtn}
                      </button>

                      <button
                        type="button"
                        onClick={() => void handleCopyForDiscord(clip)}
                        title={t.library.copyHint}
                        className={`inline-flex items-center gap-1 rounded-xl px-2.5 py-1.5 text-xs font-semibold shadow-lg backdrop-blur-md transition ${
                          isCopied
                            ? "bg-emerald-600 text-white"
                            : "bg-black/75 text-slate-100 hover:bg-emerald-600 hover:text-white"
                        }`}
                      >
                        {isCopied ? (
                          <Check className="h-3.5 w-3.5" />
                        ) : (
                          <ClipboardCopy className="h-3.5 w-3.5" />
                        )}
                        {t.library.copyCtrlV}
                      </button>

                      <button
                        type="button"
                        onClick={() => void handleToggleFavorite(clip)}
                        title={
                          clip.favorite ? t.library.favRemove : t.library.favAdd
                        }
                        className={`rounded-xl p-1.5 shadow-lg backdrop-blur-md transition ${
                          clip.favorite
                            ? "bg-amber-500 text-slate-950"
                            : "bg-black/75 text-slate-200 hover:bg-amber-500 hover:text-slate-950"
                        }`}
                      >
                        <Star
                          className={`h-3.5 w-3.5 ${
                            clip.favorite ? "fill-current" : ""
                          }`}
                        />
                      </button>
                    </div>

                    {/* Bottom overlay stats + Duration pill */}
                    <div className="pointer-events-none absolute inset-x-0 bottom-0 flex items-center justify-between bg-gradient-to-t from-black/90 via-black/40 to-transparent px-3 pb-2.5 pt-8 text-[11px] font-medium text-slate-200">
                      <span className="truncate text-[10px] text-slate-300/90">
                        {clip.width}x{clip.height} • {clip.fps} fps
                      </span>
                      <span className="rounded-md bg-black/80 px-1.5 py-0.5 font-mono text-[11px] font-bold text-white ring-1 ring-white/10">
                        {formatDuration(clip.duration_sec)}
                      </span>
                    </div>
                  </div>

                  {/* Clean Metadata Footer */}
                  <div className="flex items-center justify-between gap-2 px-3.5 py-3">
                    <div className="min-w-0 flex-1">
                      {isRenaming ? (
                        <div className="flex items-center gap-1.5">
                          <input
                            type="text"
                            autoFocus
                            value={renameValue}
                            onChange={(e) => setRenameValue(e.target.value)}
                            onKeyDown={(e) => {
                              if (e.key === "Enter") {
                                void handleSubmitRename(clip);
                              } else if (e.key === "Escape") {
                                setRenamingPath(null);
                              }
                            }}
                            className="min-w-0 flex-1 rounded-lg bg-black/60 px-2 py-1 text-xs font-semibold text-white ring-1 ring-[var(--accent)] outline-none"
                          />
                          <button
                            type="button"
                            onClick={() => void handleSubmitRename(clip)}
                            className="rounded-lg bg-emerald-600 p-1 text-white hover:bg-emerald-500"
                            title={t.library.saveName}
                          >
                            <Check className="h-3.5 w-3.5" />
                          </button>
                          <button
                            type="button"
                            onClick={() => setRenamingPath(null)}
                            className="rounded-lg bg-white/[0.08] p-1 text-slate-300 hover:text-white"
                            title={t.library.cancel}
                          >
                            <X className="h-3.5 w-3.5" />
                          </button>
                        </div>
                      ) : (
                        <h3
                          onClick={() => onOpenEditor(clip)}
                          className="cursor-pointer truncate text-sm font-semibold text-[var(--text-primary)] transition hover:text-[var(--accent-hover)]"
                          title={clip.title}
                        >
                          {clip.title}
                        </h3>
                      )}

                      <div className="mt-1 flex items-center gap-1.5 truncate text-[11px] text-slate-400">
                        <span className="font-medium text-[var(--accent-hover)]">
                          {clip.game_name || t.library.defaultScreen}
                        </span>
                        <span>•</span>
                        <span>{formatDate(clip.created_at, lang)}</span>
                        <span>•</span>
                        <span className="font-mono text-slate-500">
                          {formatBytes(clip.file_size_bytes, lang)}
                        </span>
                      </div>
                    </div>

                    {/* Subtle rename / delete controls on hover */}
                    <div className="flex shrink-0 items-center gap-1 opacity-60 transition group-hover:opacity-100">
                      <button
                        type="button"
                        onClick={() => {
                          setRenamingPath(clip.file_path);
                          setRenameValue(clip.title);
                        }}
                        title={t.library.renameTitle}
                        className="rounded-lg p-1.5 text-slate-400 transition hover:bg-white/[0.08] hover:text-white"
                      >
                        {isRenaming ? (
                          <Edit3 className="h-3.5 w-3.5 text-[var(--accent-hover)]" />
                        ) : (
                          <Pencil className="h-3.5 w-3.5" />
                        )}
                      </button>

                      {isConfirmDelete ? (
                        <button
                          type="button"
                          onClick={() => void handleDelete(clip)}
                          className="rounded-lg bg-rose-600 px-2 py-1 text-[11px] font-bold text-white hover:bg-rose-500"
                        >
                          {t.library.deleteConfirmBtn}
                        </button>
                      ) : (
                        <button
                          type="button"
                          onClick={() => setConfirmDeletePath(clip.file_path)}
                          title={t.library.deleteBtn}
                          className="rounded-lg p-1.5 text-slate-400 transition hover:bg-rose-500/20 hover:text-rose-400"
                        >
                          <Trash2 className="h-3.5 w-3.5" />
                        </button>
                      )}
                    </div>
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </div>

      {/* Custom Floating Clip Context Menu */}
      {contextMenu && (
        <div
          onClick={(e) => e.stopPropagation()}
          onContextMenu={(e) => {
            e.preventDefault();
            e.stopPropagation();
          }}
          style={{ left: contextMenu.x, top: contextMenu.y }}
          className="fixed z-50 w-56 overflow-hidden rounded-xl border border-white/[0.09] bg-[var(--bg-card)]/95 p-1.5 shadow-2xl backdrop-blur-xl"
        >
          <button
            type="button"
            onClick={() => {
              const c = contextMenu.clip;
              setContextMenu(null);
              onOpenEditor(c);
            }}
            className="flex w-full items-center gap-2.5 rounded-lg px-2.5 py-2 text-left text-xs font-medium text-slate-100 transition hover:bg-white/[0.08]"
          >
            <Play className="h-3.5 w-3.5 text-[var(--accent-hover)]" />
            {t.library.ctxOpenEditor}
          </button>

          <button
            type="button"
            onClick={() => {
              const c = contextMenu.clip;
              setContextMenu(null);
              void handleCopyForDiscord(c);
            }}
            className="flex w-full items-center gap-2.5 rounded-lg px-2.5 py-2 text-left text-xs font-medium text-slate-100 transition hover:bg-white/[0.08]"
          >
            <ClipboardCopy className="h-3.5 w-3.5 text-emerald-400" />
            {t.library.ctxCopyFile}
          </button>

          <button
            type="button"
            onClick={() => {
              const c = contextMenu.clip;
              setContextMenu(null);
              void handleRevealInExplorer(c);
            }}
            className="flex w-full items-center gap-2.5 rounded-lg px-2.5 py-2 text-left text-xs font-medium text-slate-100 transition hover:bg-white/[0.08]"
          >
            <FolderSearch className="h-3.5 w-3.5 text-amber-400" />
            {t.library.ctxRevealInFolder}
          </button>

          <button
            type="button"
            onClick={() => {
              const c = contextMenu.clip;
              setContextMenu(null);
              void handleToggleFavorite(c);
            }}
            className="flex w-full items-center gap-2.5 rounded-lg px-2.5 py-2 text-left text-xs font-medium text-slate-100 transition hover:bg-white/[0.08]"
          >
            <Star
              className={`h-3.5 w-3.5 ${
                contextMenu.clip.favorite
                  ? "fill-amber-400 text-amber-400"
                  : "text-amber-400"
              }`}
            />
            {contextMenu.clip.favorite
              ? t.library.ctxRemoveFav
              : t.library.ctxAddFav}
          </button>

          <button
            type="button"
            onClick={() => {
              const c = contextMenu.clip;
              setContextMenu(null);
              setRenamingPath(c.file_path);
              setRenameValue(c.title);
            }}
            className="flex w-full items-center gap-2.5 rounded-lg px-2.5 py-2 text-left text-xs font-medium text-slate-100 transition hover:bg-white/[0.08]"
          >
            <Pencil className="h-3.5 w-3.5 text-slate-300" />
            {t.library.ctxRename}
          </button>

          <div className="my-1 border-t border-white/[0.08]" />

          {contextMenu.confirmDelete ? (
            <button
              type="button"
              onClick={() => void handleDelete(contextMenu.clip)}
              className="flex w-full items-center gap-2.5 rounded-lg bg-rose-600/90 px-2.5 py-2 text-left text-xs font-bold text-white transition hover:bg-rose-500"
            >
              <Trash2 className="h-3.5 w-3.5" />
              {t.library.ctxDeleteConfirm}
            </button>
          ) : (
            <button
              type="button"
              onClick={() =>
                setContextMenu((prev) =>
                  prev ? { ...prev, confirmDelete: true } : prev
                )
              }
              className="flex w-full items-center gap-2.5 rounded-lg px-2.5 py-2 text-left text-xs font-medium text-rose-300 transition hover:bg-rose-500/20 hover:text-rose-200"
            >
              <Trash2 className="h-3.5 w-3.5 text-rose-400" />
              {t.library.ctxDelete}
            </button>
          )}
        </div>
      )}
    </div>
  );
}
