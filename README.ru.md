<div align="center">

# iKoch

**Лёгкий рекордер откатов и сессий со встроенным видеоредактором**

[![Release](https://img.shields.io/github/v/release/its-lemon/iKoch?style=flat-square&color=4f6ef7)](https://github.com/its-lemon/iKoch/releases/latest)
[![Windows](https://img.shields.io/badge/platform-Windows%2010%2F11%20x64-0078D6?style=flat-square)](https://github.com/its-lemon/iKoch/releases/latest)
[![Linux](https://img.shields.io/badge/Linux-Wayland%20%2F%20X11%20%E2%80%A2%20Experimental-E95420?style=flat-square)](https://github.com/its-lemon/iKoch/tree/linux)
[![Stack](https://img.shields.io/badge/built%20with-Rust%20%2B%20Tauri%20v2%20%2B%20React-orange?style=flat-square)](#сборка-из-исходников)
[![License](https://img.shields.io/badge/license-MIT-green?style=flat-square)](Cargo.toml)

[English](README.md) • [Русский](README.ru.md) • [Скачать последний релиз](https://github.com/its-lemon/iKoch/releases/latest)

</div>

---

**iKoch** — быстрый и лёгкий рекордер откатов и игровых сессий со встроенным редактором на базе **Rust**, **Tauri v2** и **React**.
- **Windows (ветка `main`):** захват через **Windows Graphics Capture (WGC)** с аппаратным кодированием GPU (**NVIDIA NVENC / AMD AMF / Intel QuickSync**) и многодорожечный звук **WASAPI**.
- **Linux (ветка `linux` — Experimental / CI-Verified):** разрабатывается в отдельной ветке [`linux`](https://github.com/its-lemon/iKoch/tree/linux), чтобы никак не конфликтовать со сборкой для Windows.

<p align="center">
  <img src="docs/screenshots/preview-ru.png" alt="iKoch Русский интерфейс" width="900" />
</p>

### ✨ Основные возможности
- **Мгновенный откат (`F9`)** — кольцевой буфер в ОЗУ (15–300 сек), сохраняющий клип ровно по ключевым кадрам (IDR).
- **Полная запись сессий (`F10`)** — отказоустойчивая запись в `.fmp4` с автоматическим ремуксом в `faststart` MP4.
- **Автоопределение игр** — сам находит запущенную игру, ставит запись на паузу при сворачивании (Alt+Tab) или захватывает выбранный монитор/окно (поддерживает как нативные Linux-игры, так и игры через Proton/Wine).
- **Многодорожечный звук** — итоговый микс (`Mixed Audio`) + отдельные дорожки `Игра`, `Микрофон` (**RNNoise**, **Noise Gate**, **Soft Limiter**, Push-to-Talk) и дополнительный источник.
- **Встроенный редактор** — быстрая обрезка без перекодирования, склейка фрагментов, скорость (`0.25x–4.0x`), форматы `16:9` / `9:16` (вертикальный с размытием) / `1:1`, микшер громкости дорожек, сжатие под Discord (`10 МБ` / `50 МБ` / свой размер), экспорт в GIF и PNG, копирование файла в буфер (`Ctrl+V`).
- **Темы и языки** — русский и английский интерфейс, 8 готовых тем + конструктор своей темы в реальном времени.
- **Фоновый режим без расхода ОЗУ** — при закрытии окна процесс WebView выгружается полностью, а лёгкий демон записи продолжает работать в системном трее.

---

### 🐧 Поддержка Linux (ветка `linux` — Experimental / CI-Verified)

Linux-версия развивается в отдельной ветке [`linux`](https://github.com/its-lemon/iKoch/tree/linux) и использует общие крейты `core-mux`, `core-types`, `recorder-daemon` и React-интерфейс, изолируя платформенный захват, кодирование и звук:

- **Захват экрана и окон:**
  - **Приоритетный режим (`gpu-screen-recorder`):** если в `PATH` установлен `gpu-screen-recorder` (принцип работы как в Vice), iKoch автоматически использует его для максимальной производительности на Wayland/X11;
  - **Встроенный fallback (`ashpd` + GStreamer):** `xdg-desktop-portal` + **PipeWire** + **GStreamer** (`nvh264enc` / `vaapih264enc` / `x264enc`) работает «из коробки» без внешних утилит.
- **Звук:** **PipeWire / PulseAudio** (`libpulse-binding`) с той же схемой 3/4 дорожек (`Mixed`, `Game`, `Mic`, `Secondary`), шумоподавлением `nnnoiseless` (RNNoise) и Soft Limiter.
- **Системная интеграция:**
  - Глобальные горячие клавиши через **Wayland GlobalShortcuts Portal** (`ashpd`), **X11** или `/dev/input/event*` (`evdev` fallback при наличии пользователя в группе `input`);
  - Системный трей через `libayatana-appindicator3` / `libappindicator3` (если библиотека отсутствует — приложение запускает главное окно напрямую без падения);
  - Автозапуск через `~/.config/autostart/ikoch.desktop`;
  - Копирование файлов в буфер обмена через `wl-copy` (Wayland) / `xclip` (X11);
  - Открытие папки и выделение файла через D-Bus (`org.freedesktop.FileManager1`) / `xdg-open`.

#### Установка на Linux
Скачайте пакет `.AppImage` или `.deb` из раздела **[Releases](https://github.com/its-lemon/iKoch/releases)** (или из **Actions → Artifacts** ветки `linux`).

---

### 📁 Порядок в папке с видео
Внутри выбранной папки клипов (по умолчанию `%USERPROFILE%\Videos\iKoch\` на Windows, `~/Videos/iKoch/` на Linux):
- **`Clips/<Игра или Screen>/`** — мгновенные клипы (`F9`)
- **`Edited/`** — все файлы из Редактора (обрезка, `9:16`, сжатие под Discord, GIF, PNG, sub-clips)
- **`Sessions/<Игра или Screen>/`** — длинные записи сессий (`F10`)

Метаданные (`.json`), превью (`.jpg`), временные файлы (`.tmp`) и логи хранятся отдельно (`%LOCALAPPDATA%\iKoch\` на Windows, `~/.local/share/iKoch/` на Linux), не засоряя папки с видео.

### ⌨️ Горячие клавиши по умолчанию
| Клавиша | Действие |
|---|---|
| `F9` | Сохранить мгновенный клип (откат) |
| `F10` | Начать / остановить запись сессии |
| `F7` | Поставить закладку (маркер) |
| `F6` | Включить / выключить микрофон |
| `Ctrl+Shift+P` | Режим приватности (пауза захвата и очистка буфера) |

---

### 🛠 Сборка из исходников

#### Windows (ветка `main`)
1. Установите **Rust (1.82+)** и **Node.js (20+)**.
2. Поместите `ffmpeg-x86_64-pc-windows-msvc.exe` и `ffprobe-x86_64-pc-windows-msvc.exe` в `src-tauri/binaries/` (исключены из Git из-за лимита GitHub в 100 МБ).
3. Выполните:
   ```powershell
   npm install
   npx tauri build
   ```

#### Linux (ветка `linux`)
1. Клонируйте ветку `linux`:
   ```bash
   git clone -b linux https://github.com/its-lemon/iKoch.git
   cd iKoch
   ```
2. Установите системные зависимости (**Rust 1.82+**, **Node.js 20+**, `webkit2gtk-4.1`, `gstreamer`, `pipewire`, `libpulse`):
   - **Ubuntu / Debian:**
     ```bash
     sudo apt update
     sudo apt install -y \
       libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf libssl-dev \
       libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
       gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-plugins-ugly gstreamer1.0-libav \
       libpipewire-0.3-dev libpulse-dev libasound2-dev libxdo-dev ffmpeg
     ```
   - **Arch / CachyOS:**
     ```bash
     sudo pacman -S --needed \
       webkit2gtk-4.1 libayatana-appindicator librsvg openssl patchelf \
       gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad gst-plugins-ugly gst-libav \
       pipewire libpulse alsa-lib xdotool ffmpeg
     ```
   - *(Опционально для максимальной производительности на Wayland)* установите `gpu-screen-recorder` и добавьте пользователя в группу `input` для `evdev`-хоткеев:
     ```bash
     sudo usermod -aG input "$USER"
     ```
3. Скопируйте или сделайте симлинки системных `ffmpeg` и `ffprobe` в `src-tauri/binaries/`:
   ```bash
   mkdir -p src-tauri/binaries
   ln -sf "$(which ffmpeg)" src-tauri/binaries/ffmpeg-x86_64-unknown-linux-gnu
   ln -sf "$(which ffprobe)" src-tauri/binaries/ffprobe-x86_64-unknown-linux-gnu
   ```
4. Установите фронтенд-зависимости и соберите `.deb` / `.AppImage`:
   ```bash
   npm install
   npx tauri build
   ```
