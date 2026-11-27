<div align="center">

# iKoch

**Fast, lightweight open-source Medal.tv & NVIDIA ShadowPlay alternative for Windows 10/11**  
**Быстрый и лёгкий open-source аналог Medal.tv и NVIDIA ShadowPlay для Windows 10/11**

[![Release](https://img.shields.io/badge/version-v0.2.4-4f6ef7?style=flat-square)](https://github.com/its-lemon/iKoch/releases/latest)
[![Platform](https://img.shields.io/badge/platform-Windows%2010%2F11%20x64-0078D6?style=flat-square)](https://github.com/its-lemon/iKoch/releases/latest)
[![License](https://img.shields.io/badge/license-MIT-green?style=flat-square)](Cargo.toml)

[English](#-english) • [Русский (README.ru.md)](README.ru.md) • [Download Latest Release / Скачать](https://github.com/its-lemon/iKoch/releases/latest)

</div>

---

<details open>
<summary><h2>🇬🇧 English</h2></summary>

**iKoch** is a fast, lightweight **open-source alternative to Medal.tv and NVIDIA ShadowPlay** for Windows 10/11, built on **Rust + Tauri v2** — with **no ads, no mandatory accounts, no telemetry, no anti-cheat-risky DLL hooks**, and low RAM usage (~100–130 MB during active 30s GPU buffering with **0 MB** for the UI when closed to tray, vs 350–500+ MB in Electron clippers).

<p align="center">
  <img src="docs/screenshots/preview-en.png" alt="iKoch English UI" width="900" />
</p>

### ✨ Features (`v0.2.4`)
- **Zero-Copy GPU Capture:** `Windows.Graphics.Capture` + `DXGI Desktop Duplication` (no yellow border on Windows 10) with hardware **H.264 / HEVC / AV1** encoding (**NVIDIA NVENC / AMD AMF / Intel QuickSync**, including **AV1** on **RTX 40 / RX 7000 / Arc**).
- **Instant Replays (`F9`) & Crash-Safe Sessions (`F10`):** GOP-aligned RAM `Replay Buffer` (15s–5m, with optional **Reset buffer after clip**), resilient `fMP4` session recording with automatic `faststart` MP4 remux, and timeline bookmarks (`F8`).
- **Automatic Game Detection:** Smart Whitelist/Blacklist game detection with auto-pause on Alt+Tab, plus manual target selection (**Game / Window / Monitor**).
- **Per-Application Audio Separation (like Medal):** `WASAPI Process Loopback` + `IAudioSessionEnumerator` isolates **Game**, **Discord**, **Music/Browser**, and **Microphone** into separate MP4 tracks with **0 MB idle overhead**, **0–500% boost** (synced to the Editor), pure-Rust `nnnoiseless` (**RNNoise**), **Noise Gate**, **Soft Limiter**, and **Push-to-Talk / Push-to-Mute**.
- **Built-in Clip Editor:** Multi-track audio mixer (`0–500%` + Mute) and per-track waveforms, `F8` markers, single-clip multi-segment trim/cut, **Replace current clip** / **Save as new**, `0.25x–4.0x` playback speed, **Discord compression** (`10 / 50 / Custom MB`), instant clipboard copy (`Ctrl+V`), `GIF` and `PNG` frame export.
- **Full Customization & Housekeeping:** 8 themes + custom color builder, `RU/EN` UI, `(?)` tooltips, automatic editor cache cleanup, disk quota & age limits, Windows autostart, and automatic updates via GitHub Releases.

### 📥 Installation & Requirements
- **OS:** Windows 10 (64-bit) or Windows 11. Isolated per-application audio capture (`Process Loopback`) requires **Windows 10 build 20348+** / **Windows 11**; on older Windows 10 builds, standard system `WASAPI Loopback` and `DXGI Desktop Duplication` (without the yellow capture border) work automatically.
- **SmartScreen Note:** Because iKoch is a free open-source project without a paid commercial code-signing certificate, Windows Defender SmartScreen may show a warning on the first installer launch. Click **More info** → **Run anyway** — all source code is open in this repository for full inspection.

### 📁 Output Folders
Inside your clips folder (default `Videos\iKoch\`):
- **`Clips\<Game or Screen>\`** — instant replay clips (`F9`)
- **`Edited\`** — trimmed, speed-adjusted, or Discord-compressed clips
- **`Sessions\<Game or Screen>\`** — full session recordings (`F10`)

### ⌨️ Default Shortcuts
| Shortcut | Action |
|---|---|
| `F9` | Save instant replay clip |
| `F10` | Start / stop full session recording |
| `F8` | Add a bookmark marker |
| `Ctrl+Shift+M` | Mute / unmute microphone |
| `Ctrl+Shift+P` | Privacy Mode (pause capture & clear buffer) |

<details>
<summary><b>🛠 Technical Details & Build from Source</b></summary>

- **Stack:** Rust + Tauri v2 + React + TypeScript.
- **Video Pipeline:** Zero-copy D3D11 WGC / DXGI Desktop Duplication -> GPU hardware encoding (`H.264` / `HEVC`), GOP-aligned circular RAM replay buffer, crash-safe `fMP4` session writer with `faststart` MP4 remux.
- **Audio Pipeline:** Multi-track `48 kHz` Stereo AAC (`AudioMixed` with soft-clipping + separate `AudioGame`, `AudioMic` with `nnnoiseless` RNNoise + noise gate + soft limiter + PTT/PTM, optional `AudioSecondary`, and dynamic `AudioApp(u8)` via `ActivateAudioInterfaceAsync` Process Loopback & `IAudioSessionManager2`).
- **Editor & Sidecar:** FFmpeg/FFprobe sidecars (`-c copy` lossless trim, single-clip multi-segment cut, `amix` multi-track mixer, Discord bitrate budgeting), Win32 `CF_HDROP` clipboard copy, GitHub Releases updater.
- **App Data:** `.json` sidecars, `.jpg` thumbnails, `.m4a` stems, and logs are stored out-of-band in `%LOCALAPPDATA%\iKoch\`.

**Build from source:**
1. Install **Rust (1.82+)** and **Node.js (20+)**.
2. Download and place `ffmpeg-x86_64-pc-windows-msvc.exe` and `ffprobe-x86_64-pc-windows-msvc.exe` into `src-tauri/binaries/` (excluded from Git due to GitHub's 100 MiB file limit) using PowerShell:
   ```powershell
   $zip = "$env:TEMP\ffmpeg-release-essentials.zip"
   $tmp = "$env:TEMP\ffmpeg-extract"
   Invoke-WebRequest -Uri "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip" -OutFile $zip
   Expand-Archive -Path $zip -DestinationPath $tmp -Force
   $bin = (Get-ChildItem -Path $tmp -Directory | Select-Object -First 1).FullName + "\bin"
   New-Item -ItemType Directory -Force -Path "src-tauri\binaries" | Out-Null
   Copy-Item "$bin\ffmpeg.exe" "src-tauri\binaries\ffmpeg-x86_64-pc-windows-msvc.exe" -Force
   Copy-Item "$bin\ffprobe.exe" "src-tauri\binaries\ffprobe-x86_64-pc-windows-msvc.exe" -Force
   Remove-Item $zip, $tmp -Recurse -Force
   ```
3. Run:
   ```powershell
   npm install
   npx tauri build
   ```

</details>

</details>

---

<details>
<summary><h2>🇷🇺 Русский</h2></summary>

**iKoch** — это быстрый и лёгкий **open-source аналог Medal.tv и NVIDIA ShadowPlay для Windows 10/11** на **Rust + Tauri v2**: **без рекламы, без обязательных аккаунтов, без телеметрии, без античит-рисковых DLL-хуков** и с низким расходом ОЗУ (~100–130 МБ при активной 30с буферизации с GPU-драйверами и **0 МБ** на интерфейс в трее против 350–500+ МБ у Electron-клипперов).

<p align="center">
  <img src="docs/screenshots/preview-ru.png" alt="iKoch Русский интерфейс" width="900" />
</p>

### ✨ Возможности (`v0.2.4`)
- **Zero-Copy GPU захват:** `Windows.Graphics.Capture` + `DXGI Desktop Duplication` (без жёлтой рамки на Windows 10) с аппаратным кодированием **H.264 / HEVC / AV1** (**NVIDIA NVENC / AMD AMF / Intel QuickSync**, включая **AV1** на **RTX 40 / RX 7000 / Arc**).
- **Мгновенные откаты (`F9`) и запись сессий (`F10`):** кольцевой `Replay Buffer` в ОЗУ (15с–5м, с опцией **сброса буфера после клипа**), отказоустойчивая запись сессий в `fMP4` с ремуксом в `faststart` MP4 и закладки (`F8`).
- **Авто-детект игр:** умный Whitelist/Blacklist игр с авто-паузой при сворачивании (Alt+Tab) или ручной выбор цели (**Игра / Окно / Монитор**).
- **Разделение звука по приложениям как в Medal:** `WASAPI Process Loopback` + `IAudioSessionEnumerator` пишет **Игру**, **Discord**, **Музыку/браузер** и **Микрофон** на отдельные дорожки MP4 с **0 МБ** оверхеда в простое, усилением **0–500%** и синхронизацией с Редактором, шумодавом `nnnoiseless` (**RNNoise**), **Noise Gate**, **Soft Limiter** и **Push-to-Talk / Push-to-Mute**.
- **Встроенный видеоредактор:** многодорожечный микшер (`0–500%` + Mute) и волны по каждой дорожке, закладки `F8`, нарезка сегментов одного клипа, **«Заменить текущий клип» / «Сохранить как новый»**, скорость `0.25x–4.0x`, сжатие под Discord (`10 / 50 / Custom МБ`), копирование файла в буфер (`Ctrl+V`), экспорт в `GIF` и `PNG`.
- **Полная кастомизация:** 8 тем + конструктор своей темы, `RU/EN` языки, подсказки `(?)`, авто-очистка кэша, квота диска, автозапуск и автообновление через GitHub Releases.

### 📥 Установка и требования
- **ОС:** Windows 10 (64-bit) или Windows 11. Для изолированного захвата звука отдельных приложений (`Process Loopback`) требуется **Windows 10 build 20348+** / **Windows 11**; на более старых сборках Windows 10 работают стандартный системный `WASAPI Loopback` и `DXGI Desktop Duplication` без жёлтой рамки.
- **Про Windows Defender SmartScreen:** так как это бесплатный open-source проект без платного коммерческого сертификата подписи кода, Windows может показать предупреждение SmartScreen при первом запуске установщика. Нажмите **Подробнее** → **Выполнить в любом случае** — весь исходный код полностью открыт для проверки.

### 📁 Где лежат ваши видео
Внутри выбранной папки (по умолчанию `Видео\iKoch\`):
- **`Clips\<Игра или Screen>\`** — мгновенные откаты (`F9`)
- **`Edited\`** — клипы после обрезки, ускорения или сжатия под Discord
- **`Sessions\<Игра или Screen>\`** — полные записи сессий (`F10`)

### ⌨️ Горячие клавиши по умолчанию
| Клавиша | Что делает |
|---|---|
| `F9` | Сохранить мгновенный клип |
| `F10` | Начать / остановить полную запись |
| `F8` | Поставить закладку на таймлайне |
| `Ctrl+Shift+M` | Включить / выключить микрофон |
| `Ctrl+Shift+P` | Режим приватности (пауза записи и очистка буфера) |

<details>
<summary><b>🛠 Технические детали и сборка из исходников</b></summary>

- **Стек:** Rust + Tauri v2 + React + TypeScript.
- **Захват и видео:** Zero-copy D3D11 WGC / DXGI Desktop Duplication -> аппаратное кодирование GPU (`H.264` / `HEVC`), кольцевой GOP-буфер в ОЗУ, отказоустойчивая запись `.fmp4` с автоматическим ремуксом в `faststart` MP4.
- **Аудио-ядро:** многодорожечный `48 kHz` Stereo AAC (`AudioMixed` с мягким клиппером + отдельные стемы `AudioGame`, `AudioMic` с `nnnoiseless` RNNoise + noise gate + soft limiter + PTT/PTM, опциональный `AudioSecondary` и динамический `AudioApp(u8)` через `ActivateAudioInterfaceAsync` Process Loopback и `IAudioSessionManager2`).
- **Редактор и интеграции:** FFmpeg/FFprobe sidecar (lossless-обрезка `-c copy`, нарезка сегментов одного клипа, `amix` микшер, расчёт битрейта под Discord), копирование файла в буфер Windows через `CF_HDROP`, обновление через GitHub Releases API.
- **Служебные файлы:** `.json`-сайдкар метаданных, `.jpg`-превью, `.m4a`-стемы и логи хранятся отдельно в `%LOCALAPPDATA%\iKoch\`.

**Сборка из исходников:**
1. Установите **Rust (1.82+)** и **Node.js (20+)**.
2. Скачайте и поместите `ffmpeg-x86_64-pc-windows-msvc.exe` и `ffprobe-x86_64-pc-windows-msvc.exe` в `src-tauri/binaries/` (исключены из Git из-за лимита GitHub в 100 МБ) с помощью PowerShell:
   ```powershell
   $zip = "$env:TEMP\ffmpeg-release-essentials.zip"
   $tmp = "$env:TEMP\ffmpeg-extract"
   Invoke-WebRequest -Uri "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip" -OutFile $zip
   Expand-Archive -Path $zip -DestinationPath $tmp -Force
   $bin = (Get-ChildItem -Path $tmp -Directory | Select-Object -First 1).FullName + "\bin"
   New-Item -ItemType Directory -Force -Path "src-tauri\binaries" | Out-Null
   Copy-Item "$bin\ffmpeg.exe" "src-tauri\binaries\ffmpeg-x86_64-pc-windows-msvc.exe" -Force
   Copy-Item "$bin\ffprobe.exe" "src-tauri\binaries\ffprobe-x86_64-pc-windows-msvc.exe" -Force
   Remove-Item $zip, $tmp -Recurse -Force
   ```
3. Выполните:
   ```powershell
   npm install
   npx tauri build
   ```

</details>

</details>
