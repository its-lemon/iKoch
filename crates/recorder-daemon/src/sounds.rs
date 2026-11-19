//! Embedded WAV sound presets (`include_bytes!`) and custom `.wav`/`.mp3`/`.ogg` playback
//! via `rodio` with per-event volume control.

use core_types::{
    NotificationSettings, NotificationSoundEntry, SoundNotificationKind, SoundSource,
};
use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player, Source};
use std::fs::File;
use std::io::{BufReader, Cursor};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tracing::{debug, warn};

static PRESET_CHIME_WAV: &[u8] = include_bytes!("../assets/chime.wav");
static PRESET_START_WAV: &[u8] = include_bytes!("../assets/start.wav");
static PRESET_STOP_WAV: &[u8] = include_bytes!("../assets/stop.wav");
static PRESET_TICK_WAV: &[u8] = include_bytes!("../assets/tick.wav");

enum SoundCommand {
    Play {
        source: SoundSource,
        gain: f32,
    },
    Shutdown,
}

/// Non-blocking sound cue player running `rodio` on a dedicated background thread.
pub struct SoundCuePlayer {
    tx: Sender<SoundCommand>,
    stop_flag: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl SoundCuePlayer {
    /// Starts the dedicated `rodio` sound cue worker thread.
    pub fn start() -> Result<Self, String> {
        let (tx, rx) = bounded::<SoundCommand>(32);
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop_flag);
        let worker = thread::Builder::new()
            .name("rustclip-sound-cues".to_string())
            .spawn(move || run_sound_loop(rx, stop_clone))
            .map_err(|e| format!("Spawn sound cue thread failed: {e}"))?;

        Ok(Self {
            tx,
            stop_flag,
            worker: Some(worker),
        })
    }

    /// Plays the notification sound configured for `kind` inside `settings` (if not muted/silent).
    pub fn play_event(&self, kind: SoundNotificationKind, settings: &NotificationSettings) {
        if !settings.sound_enabled {
            return;
        }
        let entry = entry_for_kind(kind, settings);
        let gain = settings.sound_volume.clamp(0.0, 1.0) * entry.volume.clamp(0.0, 1.0);
        if gain <= 0.001 || matches!(entry.source, SoundSource::Silent) {
            return;
        }
        if let Err(TrySendError::Full(_)) = self.tx.try_send(SoundCommand::Play {
            source: entry.source.clone(),
            gain,
        }) {
            debug!("Sound cue queue saturated; skipping non-critical notification cue");
        }
    }

    /// Stops the sound worker thread cleanly.
    pub fn stop(mut self) {
        self.shutdown_inner();
    }

    fn shutdown_inner(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        let _ = self.tx.try_send(SoundCommand::Shutdown);
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}

impl Drop for SoundCuePlayer {
    fn drop(&mut self) {
        self.shutdown_inner();
    }
}

fn entry_for_kind(
    kind: SoundNotificationKind,
    settings: &NotificationSettings,
) -> &NotificationSoundEntry {
    match kind {
        SoundNotificationKind::ClipSaved => &settings.sounds.clip_saved,
        SoundNotificationKind::RecordingStarted => &settings.sounds.recording_started,
        SoundNotificationKind::RecordingStopped => &settings.sounds.recording_stopped,
        SoundNotificationKind::MarkerAdded => &settings.sounds.marker_added,
    }
}

fn preset_bytes(name: &str) -> &'static [u8] {
    match name.trim().to_ascii_lowercase().as_str() {
        "start" => PRESET_START_WAV,
        "stop" => PRESET_STOP_WAV,
        "tick" => PRESET_TICK_WAV,
        _ => PRESET_CHIME_WAV,
    }
}

fn run_sound_loop(rx: Receiver<SoundCommand>, stop_flag: Arc<AtomicBool>) {
    let mut sink_opt: Option<MixerDeviceSink> = None;

    while !stop_flag.load(Ordering::Relaxed) {
        let Ok(cmd) = rx.recv_timeout(Duration::from_millis(200)) else {
            continue;
        };
        match cmd {
            SoundCommand::Shutdown => break,
            SoundCommand::Play { source, gain } => {
                if sink_opt.is_none() {
                    match DeviceSinkBuilder::open_default_sink() {
                        Ok(s) => sink_opt = Some(s),
                        Err(err) => {
                            warn!("Could not open default playback sink for sound cue: {err}");
                            continue;
                        }
                    }
                }
                let Some(sink) = sink_opt.as_ref() else {
                    continue;
                };
                if let Err(err) = play_source_on_sink(sink, &source, gain) {
                    warn!("Sound cue playback failed ({err}); reopening default sink on next cue");
                    sink_opt = None;
                }
            }
        }
    }
}

fn play_source_on_sink(
    sink: &MixerDeviceSink,
    source: &SoundSource,
    gain: f32,
) -> Result<(), String> {
    let player = Player::connect_new(sink.mixer());
    match source {
        SoundSource::Silent => return Ok(()),
        SoundSource::Preset(name) => {
            let bytes = preset_bytes(name);
            let dec = Decoder::try_from(Cursor::new(bytes))
                .map_err(|e| format!("Preset decode failed: {e}"))?;
            player.append(dec.amplify(gain));
        }
        SoundSource::CustomFile(path) => {
            if let Err(err) = append_custom_file(&player, path, gain) {
                warn!(
                    "Custom sound '{}' failed ({err}); falling back to chime preset",
                    path.display()
                );
                let dec = Decoder::try_from(Cursor::new(PRESET_CHIME_WAV))
                    .map_err(|e| format!("Fallback chime decode failed: {e}"))?;
                player.append(dec.amplify(gain));
            }
        }
    }
    player.detach();
    Ok(())
}

fn append_custom_file(player: &Player, path: &PathBuf, gain: f32) -> Result<(), String> {
    let file = File::open(path).map_err(|e| format!("open failed: {e}"))?;
    let dec = Decoder::try_from(BufReader::new(file)).map_err(|e| format!("decode failed: {e}"))?;
    player.append(dec.amplify(gain));
    Ok(())
}

/// Validates that all four built-in WAV presets decode cleanly via `rodio::Decoder`.
pub fn verify_embedded_presets_decode() -> Result<(), String> {
    for (name, bytes) in [
        ("chime", PRESET_CHIME_WAV),
        ("start", PRESET_START_WAV),
        ("stop", PRESET_STOP_WAV),
        ("tick", PRESET_TICK_WAV),
    ] {
        let dec = Decoder::try_from(Cursor::new(bytes))
            .map_err(|e| format!("Embedded preset '{name}' failed rodio decode: {e}"))?;
        if dec.sample_rate().get() == 0 || dec.channels().get() == 0 {
            return Err(format!("Preset '{name}' decoded invalid stream parameters"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_wav_presets_decode_cleanly_in_rodio() -> Result<(), String> {
        verify_embedded_presets_decode()
    }
}
