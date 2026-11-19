# iKoch

Desktop game & screen replay recorder and clip editor for Windows (Tauri v2 + Rust + React).

## Features
- **Instant Replay Buffer (`F9`)** & **Full Session Recording (`F10`)** with GPU hardware encoding (Media Foundation / NVENC / AMF / QuickSync)
- **Auto Game Detection** and per-monitor / window capture via Windows Graphics Capture
- **Multi-track Audio** (Game, Microphone with RNNoise / Noise Gate / Soft Limiter, Secondary output) via WASAPI
- **Built-in Clip Editor** (lossless trim, multi-segment cut, playback speed, `16:9` / `9:16` / `1:1`, audio mixer, Discord compression, GIF & PNG export, `Ctrl+V` file copy)
- **Organized Media Folders** inside your clips directory:
  - `Clips\<Game or Screen>\` — instant replay clips (`F9`)
  - `Edited\` — exported & edited files from the Editor
  - `Sessions\<Game or Screen>\` — full-length session recordings (`F10`)
- Metadata (`.json`), thumbnails (`.jpg`), and temporary stems (`.tmp`) are stored out-of-band in `%LOCALAPPDATA%\iKoch\cache`.

## Build Prerequisites (Windows)
1. **Rust** (1.82+) and **Node.js** (20+)
2. **FFmpeg & FFprobe sidecars** (excluded from Git due to GitHub's 100 MiB limit):
   Place `ffmpeg-x86_64-pc-windows-msvc.exe` and `ffprobe-x86_64-pc-windows-msvc.exe` into `src-tauri/binaries/`.

## Development & Release Build
```powershell
npm install
npm run tauri dev
```

To build the release `.exe`, `.msi`, and NSIS installer:
```powershell
npm run tauri build
```
