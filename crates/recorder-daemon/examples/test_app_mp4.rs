use std::path::Path;
use std::time::{Duration, Instant};

use core_audio::{start_audio_pipeline, AudioOutputEvent};
use core_mux::PacketRouter;
use core_types::{
    AppAudioSlotConfig, AudioOutputMode, AudioSettings, MediaPacket, MicDeviceConfig,
    MonotonicClock, TrackKind, VideoCodec, VideoStreamHeader,
};

fn dummy_video_header() -> VideoStreamHeader {
    VideoStreamHeader {
        codec: VideoCodec::H264,
        width: 1280,
        height: 720,
        fps_num: 30,
        fps_den: 1,
        sps: vec![0x67, 0x42, 0x00, 0x1E, 0x9A, 0x74, 0x02, 0x80, 0x2D, 0x88],
        pps: vec![0x68, 0xCE, 0x3C, 0x80],
        vps: None,
        avcc_or_hvcc: vec![
            0x01, 0x42, 0x00, 0x1E, 0xFF, 0xE1, 0x00, 0x0A, 0x67, 0x42, 0x00, 0x1E, 0x9A, 0x74,
            0x02, 0x80, 0x2D, 0x88, 0x01, 0x00, 0x04, 0x68, 0xCE, 0x3C, 0x80,
        ],
    }
}

fn main() {
    let clock = MonotonicClock::now();
    let settings = AudioSettings {
        audio_output_mode: AudioOutputMode::DefaultDevice,
        mic_device: MicDeviceConfig::None,
        mic_enabled: false,
        separate_tracks: true,
        app_audio_slots: vec![
            AppAudioSlotConfig {
                id: "discord".to_string(),
                label: "Discord".to_string(),
                process_names: vec!["Discord.exe".to_string()],
                volume: 1.0,
                muted: false,
                enabled: true,
            },
            AppAudioSlotConfig {
                id: "music".to_string(),
                label: "limusic-app".to_string(),
                process_names: vec!["limusic-app.exe".to_string()],
                volume: 1.0,
                muted: false,
                enabled: true,
            },
        ],
        ..AudioSettings::default()
    };

    let mut router = PacketRouter::new(15, 64 * 1024 * 1024).expect("PacketRouter::new failed");
    router.update_video_header(dummy_video_header());
    router.set_app_audio_labels(vec![
        (0, "Discord".to_string()),
        (1, "limusic-app".to_string()),
    ]);

    let (handle, rx) = start_audio_pipeline(settings, None, clock).expect("start_audio_pipeline");

    let start = Instant::now();
    let mut video_seq = 0u64;
    let mut last_video = Instant::now() - Duration::from_millis(40);
    let mut app0_pkts = 0usize;
    let mut app1_pkts = 0usize;

    while start.elapsed() < Duration::from_secs(3) {
        if last_video.elapsed() >= Duration::from_millis(33) {
            last_video = Instant::now();
            let pts_us = clock.now_us();
            let is_idr = video_seq % 30 == 0;
            let nalu = if is_idr {
                vec![0x00, 0x00, 0x00, 0x02, 0x65, 0x88]
            } else {
                vec![0x00, 0x00, 0x00, 0x02, 0x41, 0x9A]
            };
            router.push_packet(MediaPacket::new(
                TrackKind::Video,
                pts_us,
                pts_us,
                33_333,
                is_idr,
                video_seq,
                nalu,
            ));
            video_seq += 1;
        }

        while let Ok(evt) = rx.try_recv() {
            match evt {
                AudioOutputEvent::StreamHeader(hdr) => router.update_audio_header(hdr),
                AudioOutputEvent::Packet(pkt) => {
                    match pkt.track {
                        TrackKind::AudioApp(0) => app0_pkts += 1,
                        TrackKind::AudioApp(1) => app1_pkts += 1,
                        _ => {}
                    }
                    router.push_packet(pkt);
                }
                AudioOutputEvent::RecoverableWarning(w) => eprintln!("WARN: {w}"),
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    let tel = handle.telemetry();
    println!("app0_pkts={app0_pkts}, app1_pkts={app1_pkts}");
    println!("mixed peak_dbfs={:.2}, pkts={}", tel.mixed.peak_dbfs, tel.mixed.encoded_packets);
    for (idx, lbl, t) in &tel.apps {
        println!(
            "App[{idx}:{lbl}] dev='{}' real_frames={} pkts={} peak_dbfs={:.2}",
            t.active_device, t.real_frames_read, t.encoded_packets, t.peak_dbfs
        );
    }

    let out = Path::new("test_app_pipeline.mp4");
    let summary = router.save_replay_clip(out).expect("save_replay_clip failed");
    println!(
        "Saved {} ({:.2}s, {} bytes)",
        summary.path.display(),
        summary.duration_sec,
        summary.bytes_written
    );
    handle.stop();
}
