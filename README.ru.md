<div align="center">

# iKoch

**Быстрый и лёгкий open-source аналог Medal.tv и NVIDIA ShadowPlay для Windows 10/11**

[![Release](https://img.shields.io/badge/version-v0.2.5-4f6ef7?style=flat-square)](https://github.com/its-lemon/iKoch/releases/latest)
[![Platform](https://img.shields.io/badge/platform-Windows%2010%2F11%20x64-0078D6?style=flat-square)](https://github.com/its-lemon/iKoch/releases/latest)
[![License](https://img.shields.io/badge/license-MIT-green?style=flat-square)](Cargo.toml)

[English](README.md) • [Русский](README.ru.md) • [Скачать последний релиз](https://github.com/its-lemon/iKoch/releases/latest)

</div>

---

**iKoch** — это быстрый и лёгкий **open-source аналог Medal.tv и NVIDIA ShadowPlay для Windows 10/11** на **Rust + Tauri v2**: **без рекламы, без обязательных аккаунтов, без телеметрии, без античит-рисковых DLL-хуков** и с низким расходом ОЗУ (~100–130 МБ при активной 30с буферизации с GPU-драйверами и **0 МБ** на интерфейс в трее против 350–500+ МБ у Electron-клипперов).

<p align="center">
  <img src="docs/screenshots/preview-ru.png" alt="iKoch Русский интерфейс" width="900" />
</p>

### ✨ Возможности (`v0.2.5`)
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
