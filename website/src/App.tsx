import { useCallback, useEffect, useState } from "react";
import {
  Code2,
  Cpu,
  Download,
  ExternalLink,
  Gauge,
  Globe,
  ShieldAlert,
  ShieldCheck,
  Sliders,
  Sparkles,
} from "lucide-react";
import { COPY, type Lang } from "./i18n";
import { SimulatorApp } from "./simulator/SimulatorApp";
import { THEME_PRESETS } from "./themes";

const RELEASES_URL = "https://github.com/its-lemon/iKoch/releases/latest";
const GITHUB_LATEST_API = "https://api.github.com/repos/its-lemon/iKoch/releases/latest";
const REPO_URL = "https://github.com/its-lemon/iKoch";

export function App() {
  const [lang, setLang] = useState<Lang>("ru");
  const [version, setVersion] = useState<string>(__IKOCH_VERSION__);
  const [downloadUrl, setDownloadUrl] = useState<string>(RELEASES_URL);
  const [activeAccent, setActiveAccent] = useState<string>(THEME_PRESETS[0].accent);
  const [activeBgPanel, setActiveBgPanel] = useState<string>(THEME_PRESETS[0].bgPanel);
  const [activeBgCard, setActiveBgCard] = useState<string>(THEME_PRESETS[0].bgCard);
  const t = COPY[lang];

  useEffect(() => {
    let active = true;
    fetch(GITHUB_LATEST_API, {
      headers: { Accept: "application/vnd.github+json" },
    })
      .then((res) => (res.ok ? res.json() : null))
      .then(
        (
          data: {
            tag_name?: string;
            html_url?: string;
            assets?: { name?: string; browser_download_url?: string }[];
          } | null
        ) => {
          if (!active || !data) return;
          const cleanTag = data.tag_name?.trim().replace(/^v/i, "");
          if (cleanTag) setVersion(cleanTag);
          const exeAsset = data.assets?.find((a) =>
            a.name?.toLowerCase().endsWith(".exe")
          );
          if (exeAsset?.browser_download_url) {
            setDownloadUrl(exeAsset.browser_download_url);
          } else if (data.html_url) {
            setDownloadUrl(data.html_url);
          }
        }
      )
      .catch(() => {});
    return () => {
      active = false;
    };
  }, []);

  const handleSyncAccent = useCallback(
    (accentHex: string, bgPanelHex?: string, bgCardHex?: string) => {
      if (accentHex) setActiveAccent(accentHex);
      if (bgPanelHex) setActiveBgPanel(bgPanelHex);
      if (bgCardHex) setActiveBgCard(bgCardHex);
    },
    []
  );

  const renderAdvantageIcon = (icon: string) => {
    const cls = "h-4 w-4";
    switch (icon) {
      case "gauge":
        return <Gauge className={cls} style={{ color: activeAccent }} />;
      case "cpu":
        return <Cpu className={cls} style={{ color: activeAccent }} />;
      case "sliders":
        return <Sliders className={cls} style={{ color: activeAccent }} />;
      case "shield":
        return <ShieldCheck className={cls} style={{ color: activeAccent }} />;
      default:
        return <Code2 className={cls} style={{ color: activeAccent }} />;
    }
  };

  return (
    <div className="min-h-screen">
      {/* Top Nav */}
      <header className="sticky top-0 z-30 border-b border-white/[0.07] bg-black/45 backdrop-blur-md">
        <div className="mx-auto flex max-w-6xl items-center justify-between px-4 py-3 sm:px-6">
          <div className="flex items-center gap-2.5">
            <img
              src={`${import.meta.env.BASE_URL}128x128.png`}
              alt="iKoch"
              className="h-8 w-8 rounded-xl object-contain shadow-lg"
            />
            <span className="text-base font-extrabold tracking-tight text-white">iKoch</span>
            <span className="hidden rounded-full bg-white/[0.06] px-2.5 py-0.5 text-[11px] font-medium text-slate-300 sm:inline-block">
              v{version}
            </span>
          </div>

          <div className="flex items-center gap-2.5">
            <button
              type="button"
              onClick={() => setLang((l) => (l === "ru" ? "en" : "ru"))}
              className="flex items-center gap-1.5 rounded-xl border border-white/10 bg-white/[0.04] px-3 py-1.5 text-xs font-semibold text-slate-200 transition hover:bg-white/[0.1]"
            >
              <Globe className="h-3.5 w-3.5" />
              {lang === "ru" ? "RU / EN" : "EN / RU"}
            </button>
            <a
              href={REPO_URL}
              target="_blank"
              rel="noreferrer"
              className="hidden items-center gap-1.5 rounded-xl border border-white/10 bg-white/[0.04] px-3 py-1.5 text-xs font-semibold text-slate-200 transition hover:bg-white/[0.1] sm:inline-flex"
            >
              {t.githubBtn}
              <ExternalLink className="h-3.5 w-3.5" />
            </a>
            <a
              href={downloadUrl}
              target="_blank"
              rel="noreferrer"
              className="inline-flex items-center gap-1.5 rounded-xl px-3.5 py-1.5 text-xs font-bold text-white shadow-lg transition hover:opacity-90"
              style={{ backgroundColor: activeAccent }}
            >
              <Download className="h-3.5 w-3.5" />
              {t.downloadBtn(version)}
            </a>
          </div>
        </div>
      </header>

      {/* Hero + Interactive Simulator */}
      <main className="mx-auto w-full max-w-[1440px] px-4 pb-20 pt-12 sm:px-6">
        <section className="text-center">
          <div className="inline-flex items-center gap-2 rounded-full border border-white/10 bg-white/[0.04] px-3.5 py-1 text-xs font-semibold text-slate-300">
            <Sparkles className="h-3.5 w-3.5" style={{ color: activeAccent }} />
            {t.badge(version)}
          </div>
          <h1 className="mx-auto mt-5 max-w-3xl text-3xl font-black tracking-tight text-white sm:text-5xl">
            {t.tagline}
          </h1>
          <p className="mx-auto mt-4 max-w-2xl text-sm leading-relaxed text-slate-300 sm:text-base">
            {t.heroDesc}
          </p>

          <div className="mt-7 flex flex-wrap items-center justify-center gap-3">
            <a
              href={downloadUrl}
              target="_blank"
              rel="noreferrer"
              className="inline-flex items-center gap-2 rounded-xl px-5 py-3 text-sm font-bold text-white shadow-xl transition hover:scale-[1.02]"
              style={{ backgroundColor: activeAccent }}
            >
              <Download className="h-4 w-4" />
              {t.downloadBtn(version)}
            </a>
            <a
              href={REPO_URL}
              target="_blank"
              rel="noreferrer"
              className="inline-flex items-center gap-2 rounded-xl border border-white/15 bg-white/[0.04] px-5 py-3 text-sm font-semibold text-white transition hover:bg-white/[0.1]"
            >
              {t.githubBtn}
              <ExternalLink className="h-4 w-4" />
            </a>
          </div>
          <p className="mt-3 text-xs text-slate-400">{t.trustLine}</p>

          <p className="mt-8 text-xs font-medium text-slate-400">{t.tryDemoHint}</p>

          {/* Simulator with subtle theme-matched radial glow */}
          <div className="relative mt-3 text-left">
            <div
              aria-hidden="true"
              className="pointer-events-none absolute -inset-x-10 -inset-y-8 -z-10 blur-3xl transition-colors duration-500"
              style={{
                background: `radial-gradient(ellipse at center, rgb(var(--accent-rgb) / 0.24), transparent 70%)`,
              }}
            />
            <SimulatorApp
              externalLang={lang}
              onSyncAccent={handleSyncAccent}
            />
          </div>
        </section>

        {/* Advantages */}
        <section className="mt-24">
          <div className="text-center">
            <h2 className="text-2xl font-extrabold text-white sm:text-3xl">
              {t.advantagesTitle}
            </h2>
            <p className="mt-2 text-sm text-slate-400">{t.advantagesSub}</p>
          </div>
          <div className="mt-9 grid grid-cols-1 gap-5 sm:grid-cols-2 lg:grid-cols-5">
            {t.advantages.map((item) => (
              <div
                key={item.title}
                className="flex flex-col rounded-2xl border border-white/[0.08] p-5 transition hover:-translate-y-0.5 hover:border-white/20"
                style={{ backgroundColor: activeBgCard }}
              >
                <div className="flex items-center justify-between">
                  <div className="text-xl font-black" style={{ color: activeAccent }}>
                    {item.metric}
                  </div>
                  <div className="flex h-8 w-8 items-center justify-center rounded-xl bg-white/[0.05] ring-1 ring-white/[0.08]">
                    {renderAdvantageIcon(item.icon)}
                  </div>
                </div>
                <div className="mt-3 text-sm font-bold text-white">{item.title}</div>
                <p className="mt-2 text-xs leading-relaxed text-slate-400">{item.desc}</p>
              </div>
            ))}
          </div>
        </section>

        {/* Installation + SmartScreen */}
        <section
          className="mt-20 rounded-2xl border border-white/[0.08] p-6 sm:p-8"
          style={{ backgroundColor: activeBgPanel }}
        >
          <div className="flex flex-col justify-between gap-6 md:flex-row md:items-center">
            <div>
              <h2 className="text-xl font-extrabold text-white sm:text-2xl">{t.installTitle}</h2>
              <p className="mt-2 max-w-2xl text-sm text-slate-300">{t.installSub}</p>
            </div>
            <a
              href={downloadUrl}
              target="_blank"
              rel="noreferrer"
              className="inline-flex shrink-0 items-center gap-2 rounded-xl px-5 py-3 text-sm font-bold text-white shadow-lg transition hover:opacity-90"
              style={{ backgroundColor: activeAccent }}
            >
              <Download className="h-4 w-4" />
              {t.installBtn(version)}
            </a>
          </div>

          <div className="mt-6 flex items-start gap-3 rounded-xl border border-amber-500/25 bg-amber-500/10 p-4">
            <ShieldAlert className="mt-0.5 h-5 w-5 shrink-0 text-amber-400" />
            <div>
              <div className="text-sm font-bold text-amber-200">{t.smartScreenTitle}</div>
              <p className="mt-1 text-xs leading-relaxed text-amber-100/80">{t.smartScreenDesc}</p>
            </div>
          </div>
        </section>
      </main>

      <footer className="border-t border-white/[0.07] py-7">
        <div className="mx-auto flex max-w-6xl flex-col items-center justify-between gap-3 px-4 text-xs text-slate-500 sm:flex-row sm:px-6">
          <span>{t.footerNote(version)}</span>
          <div className="flex items-center gap-4">
            <a href={REPO_URL} target="_blank" rel="noreferrer" className="hover:text-slate-300">
              GitHub
            </a>
            <a href={RELEASES_URL} target="_blank" rel="noreferrer" className="hover:text-slate-300">
              Releases
            </a>
          </div>
        </div>
      </footer>
    </div>
  );
}
export default App;
