<div align="center">

# iKoch

**Lightweight Windows game & screen replay recorder and built-in clip editor**  
**Лёгкий рекордер откатов и сессий для Windows со встроенным видеоредактором**

[![Release](https://img.shields.io/badge/version-v0.2.0-4f6ef7?style=flat-square)](https://github.com/its-lemon/iKoch/releases/latest)
[![Platform](https://img.shields.io/badge/platform-Windows%2010%2F11%20x64-0078D6?style=flat-square)](https://github.com/its-lemon/iKoch/releases/latest)
[![Stack](https://img.shields.io/badge/built%20with-Rust%20%2B%20Tauri%20v2%20%2B%20React-orange?style=flat-square)](#build-from-source)
[![License](https://img.shields.io/badge/license-MIT-green?style=flat-square)](Cargo.toml)

[English](#-english) • [Русский](#-русский) • [Download Latest Release / Скачать](https://github.com/its-lemon/iKoch/releases/latest)

</div>

---

## 🇬🇧 English

**iKoch** is a fast, low-overhead Windows replay recorder and clip editor built with **Rust**, **Tauri v2**, and **React**. It captures games and monitors using **Windows Graphics Capture (WGC)** + GPU hardware encoding (**NVIDIA NVENC / AMD AMF / Intel QuickSync**) and **WASAPI** multi-track audio, while keeping background tray RAM usage minimal.

<p align="center">
  <img src="docs/screenshots/preview-en.png" alt="iKoch English UI" width="900" />
</p>

### ✨ Key Features
- **Instant Replay Buffer (`F9`)** — always-on circular RAM buffer (15–300s) saving along clean IDR keyframe boundaries, with optional **Reset buffer after clip** so the next clip never repeats already-saved moments.
- **Full Session Recording (`F10`)** — crash-safe fragmented MP4 recording with automatic `faststart` remuxing.
- **Auto Game Detection** — automatically detects launched games, pauses when unfocused/minimized, or captures a chosen monitor/window.
- **Per-App Audio Separation & Multi-Track WASAPI** — records `Mixed Audio` (default playback track) + separate `Game`, `Microphone` (**RNNoise**, **Noise Gate**, **Soft Limiter**, Push-to-Talk), optional `Secondary`, and custom **Per-App Audio Channels** (e.g., *Discord*, *Spotify*, browsers — lazily captured only while the target app is running).
- **Built-in Clip Editor** — lossless trim (with **Save as New** or **Replace Original**), multi-clip merge, playback speed (`0.25x–4.0x`), dynamic per-track volume mixer (`0–200%` + `Mute`), Discord size presets (`10 MB` / `50 MB` / custom), `Ctrl+V` file copy (`CF_HDROP`).
- **Themes, Tooltips & Bilingual UI** — `EN` / `RU` interface, 8 built-in themes + live Custom Theme Builder, and interactive `(?)` tooltips across Settings.
- **GitHub Releases Auto-Updater** — check for new versions on startup or manually in Settings, view release notes, and download + launch the Windows installer in one click.
- **Zero-RAM Tray Background** — closing the window destroys the WebView2 UI process while keeping the Rust recording daemon alive in the system tray.

### 📁 Organized Media Folders
Inside your configured clips folder (default: `%USERPROFILE%\Videos\iKoch\`):
- **`Clips\<Game or Screen>\`** — instant replay clips (`F9`)
- **`Edited\`** — all exports from the Editor (trims, Discord compress, merged clips)
- **`Sessions\<Game or Screen>\`** — full-length session recordings (`F10`)

Metadata (`.json`), thumbnails (`.jpg`), temporary stems (`.tmp`), and logs are stored out-of-band in `%LOCALAPPDATA%\iKoch\` so your video folders stay clean.

### ⌨️ Default Hotkeys
| Shortcut | Action |
|---|---|
| `F9` | Save instant replay clip |
| `F10` | Start / stop full session recording |
| `F8` | Add bookmark marker |
| `Ctrl+Shift+M` | Mute / unmute microphone |
| `Ctrl+Shift+P` | Toggle Privacy Mode (pauses capture & clears buffer) |

### 🛠 Build from Source
1. Install **Rust (1.82+)** and **Node.js (20+)**.
2. Place `ffmpeg-x86_64-pc-windows-msvc.exe` and `ffprobe-x86_64-pc-windows-msvc.exe` into `src-tauri/binaries/` (excluded from Git due to GitHub's 100 MiB file limit).
3. Run:
   ```powershell
   npm install
   npx tauri build
   ```

---

## 🇷🇺 Русский

**iKoch** — быстрый и лёгкий рекордер откатов и игровых сессий для Windows на базе **Rust**, **Tauri v2** и **React**. Захват видео работает через **Windows Graphics Capture (WGC)** с аппаратным кодированием GPU (**NVIDIA NVENC / AMD AMF / Intel QuickSync**), а звук пишется через **WASAPI** сразу в несколько дорожек при минимальной нагрузке на систему.

<p align="center">
  <img src="docs/screenshots/preview-ru.png" alt="iKoch Русский интерфейс" width="900" />
</p>

### ✨ Основные возможности
- **Мгновенный откат (`F9`)** — кольцевой буфер в ОЗУ (15–300 сек), сохраняющий клип ровно по ключевым кадрам (IDR), с опцией **очистки буфера после клипа** (следующий откат не повторяет уже сохранённые моменты).
- **Полная запись сессий (`F10`)** — отказоустойчивая запись в `.fmp4` с автоматическим ремуксом в `faststart` MP4.
- **Автоопределение игр** — сам находит запущенную игру, ставит запись на паузу при сворачивании (Alt+Tab) или захватывает выбранный монитор/окно.
- **Разделение звука по приложениям и многодорожечный WASAPI** — итоговый микс (`Mixed Audio`) + отдельные дорожки `Игра`, `Микрофон` (**RNNoise**, **Noise Gate**, **Soft Limiter**, Push-to-Talk), дополнительный источник и пользовательские **каналы приложений** (*Discord*, *Spotify*, браузер — ленивый перехват запускается только пока процесс активен).
- **Встроенный редактор** — быстрая обрезка (с заменой оригинала или сохранением в `Edited/`), склейка нескольких клипов, скорость (`0.25x–4.0x`), динамический микшер громкости всех дорожек (`0–200%` + `Mute`), сжатие под Discord (`10 МБ` / `50 МБ` / свой размер), копирование файла в буфер (`Ctrl+V`).
- **Темы, подсказки `(?)` и языки** — русский и английский интерфейс, 8 готовых тем + конструктор своей темы в реальном времени и подробные подсказки `(?)` по всем настройкам.
- **Автообновление через GitHub Releases** — проверка обновлений при запуске или по кнопке в Настройках, просмотр чейнджлога и установка в один клик.
- **Фоновый режим без расхода ОЗУ** — при закрытии окна процесс WebView2 выгружается полностью, а лёгкий демон записи продолжает работать в системном трее.

### 📁 Порядок в папке с видео
Внутри выбранной папки клипов (по умолчанию `%USERPROFILE%\Videos\iKoch\`):
- **`Clips\<Игра или Screen>\`** — мгновенные клипы (`F9`)
- **`Edited\`** — все файлы из Редактора (обрезка, сжатие под Discord, склейка клипов)
- **`Sessions\<Игра или Screen>\`** — длинные записи сессий (`F10`)

Метаданные (`.json`), превью (`.jpg`), временные файлы (`.tmp`) и логи хранятся отдельно в `%LOCALAPPDATA%\iKoch\`, не засоряя папки с видео.

### ⌨️ Горячие клавиши по умолчанию
| Клавиша | Действие |
|---|---|
| `F9` | Сохранить мгновенный клип (откат) |
| `F10` | Начать / остановить запись сессии |
| `F8` | Поставить закладку (маркер) |
| `Ctrl+Shift+M` | Включить / выключить микрофон |
| `Ctrl+Shift+P` | Режим приватности (пауза захвата и очистка буфера) |

### 🛠 Сборка из исходников
1. Установите **Rust (1.82+)** и **Node.js (20+)**.
2. Поместите `ffmpeg-x86_64-pc-windows-msvc.exe` и `ffprobe-x86_64-pc-windows-msvc.exe` в `src-tauri/binaries/` (исключены из Git из-за лимита GitHub в 100 МБ).
3. Выполните:
   ```powershell
   npm install
   npx tauri build
   ```
