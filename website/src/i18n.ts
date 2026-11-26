export type Lang = "ru" | "en";

export const COPY = {
  ru: {
    badge: "Open Source • Rust + Tauri v2 • v0.2.2",
    tagline: "Лёгкий open-source аналог Medal.tv и ShadowPlay на Rust",
    heroDesc:
      "Мгновенные откаты (F9), запись сессий (F10), разделение звука по приложениям (Игра / Discord / Музыка / Мик) и встроенный редактор под Discord — без рекламы, аккаунтов и телеметрии.",
    downloadBtn: "Скачать для Windows (v0.2.2)",
    githubBtn: "GitHub",
    trustLine: "Windows 10/11 x64 • ~58 МБ • Без рекламы и аккаунтов",
    tryDemoHint: "Живое окно iKoch прямо в браузере — переключайте вкладки и настраивайте темы",
    advantagesTitle: "Почему iKoch, а не обычные клипперы?",
    advantagesSub: "Максимум FPS в играх и чистый многодорожечный звук без фонового раздувания памяти.",
    advantages: [
      {
        icon: "gauge",
        metric: "~20 МБ",
        title: "Ожидание в трее",
        desc: "В режиме «Авто-игра» на рабочем столе и 0 МБ на интерфейс при закрытии окна. При активной буферизации — ~90–120 МБ.",
      },
      {
        icon: "cpu",
        metric: "Zero-Copy",
        title: "GPU NVENC / AMF / QSV",
        desc: "Windows.Graphics.Capture и DXGI Desktop Duplication без жёлтой рамки на Win 10 — кадры идут напрямую в аппаратный энкодер.",
      },
      {
        icon: "sliders",
        metric: "0–500%",
        title: "Раздельные дорожки",
        desc: "Игра, Discord, Музыка и Микрофон (RNNoise) пишутся в отдельные дорожки MP4 с усилением до 500% и мягким лимитером.",
      },
      {
        icon: "shield",
        metric: "0 хуков",
        title: "Без DLL-инъекций",
        desc: "Никаких рисков для античитов (Vanguard, EAC, BattlEye, VAC). Захват работает на уровне системных API Windows.",
      },
      {
        icon: "code",
        metric: "MIT",
        title: "Open Source",
        desc: "Никакой рекламы, подписок, принудительных аккаунтов, облачных загрузок и скрытой телеметрии.",
      },
    ],
    installTitle: "Установка на Windows 10 / 11",
    installSub:
      "Требуется 64-битная Windows 10 или Windows 11. Изолированный захват звука приложений (Process Loopback) поддерживается на Win 10 build 20348+ и Windows 11.",
    installBtn: "Скачать iKoch_0.2.2_x64-setup.exe",
    smartScreenTitle: "Если Windows Defender SmartScreen показал предупреждение",
    smartScreenDesc:
      "iKoch — бесплатный open-source проект без платного коммерческого сертификата подписи кода. При первом запуске установщика нажмите «Подробнее» → «Выполнить в любом случае». Весь код открыт на GitHub.",
    footerNote: "iKoch v0.2.2 • Rust + Tauri v2 • Лицензия MIT",
  },
  en: {
    badge: "Open Source • Rust + Tauri v2 • v0.2.2",
    tagline: "Lightweight open-source Medal.tv & ShadowPlay alternative in Rust",
    heroDesc:
      "Instant replays (F9), crash-safe sessions (F10), per-app audio separation (Game / Discord / Music / Mic), and built-in Discord editor — zero ads, no accounts, no telemetry.",
    downloadBtn: "Download for Windows (v0.2.2)",
    githubBtn: "GitHub",
    trustLine: "Windows 10/11 x64 • ~58 MB • No ads or accounts",
    tryDemoHint: "Live iKoch window in your browser — click tabs, mix tracks, and switch themes",
    advantagesTitle: "Why iKoch over bloated clippers?",
    advantagesSub: "Engineered for maximum FPS and clean multi-track audio without background RAM waste.",
    advantages: [
      {
        icon: "gauge",
        metric: "~20 MB",
        title: "Idle Tray Footprint",
        desc: "In Auto-Game idle mode (with 0 MB UI when closed to tray). During active 30s GPU buffering — ~90–120 MB RAM.",
      },
      {
        icon: "cpu",
        metric: "Zero-Copy",
        title: "GPU NVENC / AMF / QSV",
        desc: "Windows.Graphics.Capture & DXGI Desktop Duplication (no yellow border on Win 10) feed GPU encoders directly.",
      },
      {
        icon: "sliders",
        metric: "0–500%",
        title: "Separate Audio Stems",
        desc: "Game, Discord, Music, and Mic (RNNoise) recorded to separate MP4 tracks with up to 500% boost and soft-limiter.",
      },
      {
        icon: "shield",
        metric: "0 Hooks",
        title: "Anti-Cheat Safe",
        desc: "No DLL injections into games. Captures cleanly via official Windows graphics and audio APIs.",
      },
      {
        icon: "code",
        metric: "MIT",
        title: "Open Source",
        desc: "No ads, no paywalls, no mandatory accounts, no forced cloud uploads, and zero telemetry.",
      },
    ],
    installTitle: "Installation for Windows 10 / 11",
    installSub:
      "Requires 64-bit Windows 10 or Windows 11. Isolated per-app audio capture (Process Loopback) requires Windows 10 build 20348+ or Windows 11.",
    installBtn: "Download iKoch_0.2.2_x64-setup.exe",
    smartScreenTitle: "Windows Defender SmartScreen Notice",
    smartScreenDesc:
      "iKoch is a free open-source project without a paid commercial code-signing certificate. On first installer launch, click \"More info\" → \"Run anyway\". All source code is open on GitHub.",
    footerNote: "iKoch v0.2.2 • Rust + Tauri v2 • MIT License",
  },
} as const;
