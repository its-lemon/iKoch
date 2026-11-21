<div align="center">

# iKoch

**Lightweight game & screen replay recorder and built-in clip editor**

[![Release](https://img.shields.io/github/v/release/its-lemon/iKoch?style=flat-square&color=4f6ef7)](https://github.com/its-lemon/iKoch/releases/latest)
[![Windows](https://img.shields.io/badge/platform-Windows%2010%2F11%20x64-0078D6?style=flat-square)](https://github.com/its-lemon/iKoch/releases/latest)
[![Linux](https://img.shields.io/badge/Linux-Wayland%20%2F%20X11%20%E2%80%A2%20Experimental-E95420?style=flat-square)](https://github.com/its-lemon/iKoch/tree/linux)
[![Stack](https://img.shields.io/badge/built%20with-Rust%20%2B%20Tauri%20v2%20%2B%20React-orange?style=flat-square)](#build-from-source)
[![License](https://img.shields.io/badge/license-MIT-green?style=flat-square)](Cargo.toml)

[English](README.md) • [Русский](README.ru.md) • [Download Latest Release](https://github.com/its-lemon/iKoch/releases/latest)

</div>

---

**iKoch** is a fast, low-overhead replay recorder and clip editor built with **Rust**, **Tauri v2**, and **React**.
- **Windows (`main` branch):** captures games and monitors using **Windows Graphics Capture (WGC)** + GPU hardware encoding (**NVIDIA NVENC / AMD AMF / Intel QuickSync**) and **WASAPI** multi-track audio.
- **Linux (`linux` branch — Experimental / CI-Verified):** developed in a separate [`linux`](https://github.com/its-lemon/iKoch/tree/linux) branch so it never conflicts with the Windows build.

<p align="center">
  <img src="docs/screenshots/preview-en.png" alt="iKoch English UI" width="900" />
</p>

### ✨ Key Features
- **Instant Replay Buffer (`F9`)** — always-on circular RAM buffer (15–300s) saving along clean IDR keyframe boundaries.
- **Full Session Recording (`F10`)** — crash-safe fragmented MP4 recording with automatic `faststart` remuxing.
- **Auto Game Detection** — automatically detects launched games, pauses when unfocused/minimized, or captures a chosen monitor/window.
- **Multi-Track Audio** — records `Mixed Audio` (default playback track) + separate `Game`, `Microphone` (with **RNNoise**, **Noise Gate**, **Soft Limiter**, Push-to-Talk), and optional `Secondary` tracks.
- **Built-in Clip Editor** — lossless trim, multi-segment cut, playback speed (`0.25x–4.0x`), `16:9` / `9:16` (blurred Shorts) / `1:1`, per-track volume mixer, Discord size presets (`10 MB` / `50 MB` / custom), GIF & PNG frame export, `Ctrl+V` file copy (`CF_HDROP` / `text/uri-list`).
- **Themes & Bilingual UI** — `EN` / `RU` interface, 8 built-in themes + live Custom Theme Builder.
- **Zero-RAM Tray Background** — closing the window destroys the WebView UI process while keeping the Rust recording daemon alive in the system tray.

---

### 🐧 Linux Support (`linux` branch — Experimental / CI-Verified)

The Linux backend lives in the dedicated [`linux`](https://github.com/its-lemon/iKoch/tree/linux) branch and reuses the shared `core-mux`, `core-types`, `recorder-daemon`, and React frontend while isolating platform-specific capture, encoding, and audio:

- **Screen & Window Capture:**
  - **Priority (`gpu-screen-recorder`):** if `gpu-screen-recorder` is installed in `PATH` (same approach as Vice), iKoch uses it automatically for maximum Wayland/X11 performance;
  - **Built-in Fallback (`ashpd` + GStreamer):** `xdg-desktop-portal` + **PipeWire** + **GStreamer** pipeline (`nvh264enc` / `vaapih264enc` / `x264enc` fallback) works out of the box without extra tools.
- **Audio:** **PipeWire / PulseAudio** (`libpulse-binding`) preserving the same 3/4-track layout (`Mixed`, `Game`, `Mic`, `Secondary`) with `nnnoiseless` RNNoise suppression and Soft Limiter.
- **Desktop Integration:**
  - Global hotkeys via **Wayland GlobalShortcuts Portal** (`ashpd`), **X11**, or `/dev/input/event*` (`evdev` fallback when user is in the `input` group);
  - System tray via `libayatana-appindicator3` / `libappindicator3` (gracefully opens the main window without tray if absent);
  - Autostart via `~/.config/autostart/ikoch.desktop`;
  - Clipboard file copy via `wl-copy` (Wayland) / `xclip` (X11);
  - File manager reveal via D-Bus (`org.freedesktop.FileManager1`) / `xdg-open`.

#### Installing on Linux
Download the `.AppImage` or `.deb` package from **[Releases](https://github.com/its-lemon/iKoch/releases)** (or from **Actions → Artifacts** on the `linux` branch).

---

### 📁 Organized Media Folders
Inside your configured clips folder (default: `%USERPROFILE%\Videos\iKoch\` on Windows, `~/Videos/iKoch/` on Linux):
- **`Clips/<Game or Screen>/`** — instant replay clips (`F9`)
- **`Edited/`** — all exports from the Editor (trims, `9:16`, Discord compress, GIF, PNG, sub-clips)
- **`Sessions/<Game or Screen>/`** — full-length session recordings (`F10`)

Metadata (`.json`), thumbnails (`.jpg`), temporary stems (`.tmp`), and logs are stored out-of-band (`%LOCALAPPDATA%\iKoch\` on Windows, `~/.local/share/ikoch/` on Linux) so your video folders stay clean.

### ⌨️ Default Hotkeys
| Shortcut | Action |
|---|---|
| `F9` | Save instant replay clip |
| `F10` | Start / stop full session recording |
| `F7` | Add bookmark marker |
| `F6` | Mute / unmute microphone |
| `Ctrl+Shift+P` | Toggle Privacy Mode (pauses capture & clears buffer) |

---

### 🛠 Build from Source

#### Windows (`main` branch)
1. Install **Rust (1.82+)** and **Node.js (20+)**.
2. Place `ffmpeg-x86_64-pc-windows-msvc.exe` and `ffprobe-x86_64-pc-windows-msvc.exe` into `src-tauri/binaries/` (excluded from Git due to GitHub's 100 MiB file limit).
3. Run:
   ```powershell
   npm install
   npx tauri build
   ```

#### Linux (`linux` branch)
1. Clone the `linux` branch:
   ```bash
   git clone -b linux https://github.com/its-lemon/iKoch.git
   cd iKoch
   ```
2. Install system dependencies (**Rust 1.82+**, **Node.js 20+**, `webkit2gtk-4.1`, `gstreamer`, `pipewire`, `libpulse`):
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
   - *(Optional for best Wayland performance)* install `gpu-screen-recorder` and add your user to the `input` group for `evdev` global hotkeys:
     ```bash
     sudo usermod -aG input "$USER"
     ```
3. Copy or symlink `ffmpeg` and `ffprobe` into `src-tauri/binaries/`:
   ```bash
   mkdir -p src-tauri/binaries
   ln -sf "$(which ffmpeg)" src-tauri/binaries/ffmpeg-x86_64-unknown-linux-gnu
   ln -sf "$(which ffprobe)" src-tauri/binaries/ffprobe-x86_64-unknown-linux-gnu
   ```
4. Build the frontend and Tauri bundles (`.deb` / `.AppImage`):
   ```bash
   npm install
   npx tauri build
   ```
