<div align="center">

# iKoch

**Simple and fast game replay recorder & clip editor for Windows**  
**Простая и лёгкая программа для записи игровых моментов (откатов) и монтажа клипов на Windows**

[![Release](https://img.shields.io/badge/version-v0.2.1-4f6ef7?style=flat-square)](https://github.com/its-lemon/iKoch/releases/latest)
[![Platform](https://img.shields.io/badge/platform-Windows%2010%2F11%20x64-0078D6?style=flat-square)](https://github.com/its-lemon/iKoch/releases/latest)
[![License](https://img.shields.io/badge/license-MIT-green?style=flat-square)](Cargo.toml)

[English](#-english) • [Русский (внутри / отдельно)](README.ru.md) • [Download Latest Release / Скачать](https://github.com/its-lemon/iKoch/releases/latest)

</div>

---

<details open>
<summary><h2>🇬🇧 English</h2></summary>

**iKoch** lets you save your best gaming moments with a single keypress, record full sessions, edit clips right inside the app, and share them to Discord in seconds — without slowing down your PC.

<p align="center">
  <img src="docs/screenshots/preview-en.png" alt="iKoch English UI" width="900" />
</p>

### ✨ What iKoch Can Do
- **Save the Last Moments (`F9`)** — keeps the last 15 seconds to 5 minutes ready in the background and saves them instantly when something cool happens. You can also turn on **Reset buffer after clip** so the next clip never repeats what you already saved.
- **Record Full Videos (`F10`)** — record long gameplay sessions or your screen anytime, safely protected against crashes.
- **Automatic Game Detection** — automatically starts working when you launch a game and pauses when you Alt+Tab out (or lets you pick a specific monitor or window).
- **Separate Sound for Games, Mic & Apps** — adjust the volume of your game, microphone (with built-in noise removal), and other apps like Discord or Spotify independently. Idle apps use zero resources until you open them.
- **Easy Built-in Editor** — cut clips in seconds (save as a new file or replace the original), merge multiple clips together, speed up or slow down, adjust volume per track, shrink files to fit Discord (`10 MB` / `50 MB`), and copy clips straight to your clipboard (`Ctrl+V`).
- **Your Style, Your Language** — switch between English and Russian, pick from 8 themes or customize your own colors, and hover over `(?)` icons anytime for quick tips.
- **One-Click Updates** — checks for new versions automatically and installs updates right from Settings.
- **Runs Quietly in the Tray** — closing the window frees up memory while keeping hotkeys and replay saving active in the background.

### 📁 Where Your Videos Go
Inside your clips folder (by default `Videos\iKoch\`):
- **`Clips\<Game or Screen>\`** — instant replay clips (`F9`)
- **`Edited\`** — clips you trimmed, merged, or compressed for Discord
- **`Sessions\<Game or Screen>\`** — full session recordings (`F10`)

### ⌨️ Default Shortcuts
| Shortcut | What it does |
|---|---|
| `F9` | Save instant clip |
| `F10` | Start / stop full recording |
| `F8` | Add a bookmark marker |
| `Ctrl+Shift+M` | Mute / unmute microphone |
| `Ctrl+Shift+P` | Privacy Mode (pause capture & clear buffer) |

<details>
<summary><b>🛠 Technical Details & Build from Source (for developers)</b></summary>

- **Stack:** Rust + Tauri v2 + React + TypeScript.
- **Video Pipeline:** Windows Graphics Capture (WGC) with zero-copy D3D11 -> GPU hardware encoding (NVIDIA NVENC / AMD AMF / Intel QuickSync), GOP-aligned circular RAM replay buffer, crash-safe fMP4 session writer with automatic `faststart` MP4 remux.
- **Audio Pipeline:** Multi-track WASAPI capture (`Mixed Audio` default playback track + separate `Game`, `Microphone` with pure-Rust `nnnoiseless` RNNoise + noise gate + soft limiter + PTT/PTM, optional `Secondary`, and lazy per-app WASAPI Process Loopback stems `AudioApp(u8)` using `IAudioSessionManager2`).
- **Editor & Sidecar:** FFmpeg/FFprobe sidecars (`-c copy` lossless trim, multi-segment merge, `amix` dynamic track mixer, Discord bitrate budgeting), Win32 `CF_HDROP` clipboard copy, GitHub Releases updater.
- **App Data:** `.json` sidecars, `.jpg` thumbnails, `.tmp` stems, and logs are stored out-of-band in `%LOCALAPPDATA%\iKoch\`.

**Build from source:**
1. Install **Rust (1.82+)** and **Node.js (20+)**.
2. Place `ffmpeg-x86_64-pc-windows-msvc.exe` and `ffprobe-x86_64-pc-windows-msvc.exe` into `src-tauri/binaries/` (excluded from Git due to GitHub's 100 MiB file limit).
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

**iKoch** помогает сохранять лучшие игровые моменты одним нажатием клавиши, записывать длинные видео, быстро обрезать или склеивать клипы прямо внутри программы и отправлять их в Discord — без тормозов и лишней нагрузки на ПК.

<p align="center">
  <img src="docs/screenshots/preview-ru.png" alt="iKoch Русский интерфейс" width="900" />
</p>

### ✨ Что умеет iKoch
- **Сохранение последних моментов (`F9`)** — программа держит в памяти последние 15 секунд – 5 минут игры и сохраняет клип по нажатию кнопки. Можно включить **очистку буфера после клипа**, чтобы в следующем клипе не повторялись уже сохранённые секунды.
- **Обычная запись видео (`F10`)** — записывайте долгие игровые сессии или экран целиком, запись не потеряется даже при сбое.
- **Умное определение игр** — сама находит запущенную игру, ставит запись на паузу при сворачивании (Alt+Tab) или позволяет выбрать конкретный монитор/окно.
- **Раздельный звук для игры, микрофона и программ** — меняйте громкость игры, голоса (с подавлением шума клавиатуры и кулеров) и отдельных приложений вроде Discord или Spotify независимо друг от друга. Пока программа не запущена, она не тратит ресурсы.
- **Удобный встроенный редактор** — быстрая обрезка (можно сохранить новым файлом или заменить оригинал), склейка нескольких клипов в один, изменение скорости, микшер громкости каждой дорожки, сжатие под лимит Discord (`10 МБ` / `50 МБ`), копирование готового видео в буфер обмена (`Ctrl+V`).
- **Свой стиль и подсказки** — русский и английский язык, 8 тем + настройка любых цветов под себя и понятные подсказки `(?)` рядом с каждой настройкой.
- **Обновления в один клик** — сама проверяет новые версии на GitHub и позволяет обновиться прямо из Настроек.
- **Лёгкий фоновый режим** — когда вы закрываете окно, интерфейс полностью выгружается из памяти, а запись по горячим клавишам продолжает работать в трее.

### 📁 Где лежат ваши видео
Внутри выбранной папки (по умолчанию `Видео\iKoch\`):
- **`Clips\<Игра или Screen>\`** — мгновенные клипы (`F9`)
- **`Edited\`** — клипы после обрезки, склейки или сжатия под Discord
- **`Sessions\<Игра или Screen>\`** — полные записи сессий (`F10`)

### ⌨️ Горячие клавиши по умолчанию
| Клавиша | Что делает |
|---|---|
| `F9` | Сохранить мгновенный клип |
| `F10` | Начать / остановить полную запись |
| `F8` | Поставить закладку на таймлайне |
| `Ctrl+Shift+M` | Включить / выключить микрофон |
| `Ctrl+Shift+P` | Режим приватности (пауза записи и очистка памяти) |

<details>
<summary><b>🛠 Технические детали и сборка из исходников (для разработчиков)</b></summary>

- **Стек:** Rust + Tauri v2 + React + TypeScript.
- **Захват и видео:** Windows Graphics Capture (WGC) с zero-copy D3D11 -> аппаратное кодирование GPU (NVIDIA NVENC / AMD AMF / Intel QuickSync), кольцевой GOP-буфер в ОЗУ с сохранением по ключевым кадрам (IDR), отказоустойчивая запись `.fmp4` с автоматическим ремуксом в `faststart` MP4.
- **Аудио-ядро:** многодорожечный WASAPI (`Mixed Audio` по умолчанию + отдельные стемы `Game`, `Microphone` с `nnnoiseless` RNNoise + noise gate + soft limiter + PTT/PTM, опциональный `Secondary` и ленивый WASAPI Process Loopback `AudioApp(u8)` через `IAudioSessionManager2`).
- **Редактор и интеграции:** FFmpeg/FFprobe sidecar (lossless-обрезка `-c copy`, склейка сегментов, динамический `amix` микшер, расчёт битрейта под Discord), копирование файла в буфер Windows через `CF_HDROP`, автообновление через GitHub Releases API.
- **Служебные файлы:** `.json`-сайдкар метаданных, `.jpg`-превью, временные `.tmp`-стемы и логи хранятся отдельно в `%LOCALAPPDATA%\iKoch\`.

**Сборка из исходников:**
1. Установите **Rust (1.82+)** и **Node.js (20+)**.
2. Поместите `ffmpeg-x86_64-pc-windows-msvc.exe` и `ffprobe-x86_64-pc-windows-msvc.exe` в `src-tauri/binaries/` (исключены из Git из-за лимита GitHub в 100 МБ).
3. Выполните:
   ```powershell
   npm install
   npx tauri build
   ```

</details>

</details>
