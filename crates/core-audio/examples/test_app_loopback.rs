//! Standalone test for per-application WASAPI process loopback capture on Windows 10 2004+ / 11.
//!
//! Usage:
//!   cargo run -p core-audio --example test_app_loopback
//!       Lists all Windows processes with an active audio session (or peak > 0.0001).
//!
//!   cargo run -p core-audio --example test_app_loopback -- zen.exe
//!       Lists sessions, captures 5 seconds of audio from `zen.exe` (auto-following
//!       active PIDs if the process spawns multiple subprocesses or recreates its session),
//!       writes `test_app_audio.wav`, and prints `real_frames` and `peak_dbfs`.

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::mem::{size_of, ManuallyDrop};
use std::os::windows::ffi::OsStringExt;
use std::path::Path;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use windows::core::{IUnknown, Interface, Result as WinResult, GUID, HRESULT};
use windows::Win32::Foundation::{
    CloseHandle, E_NOINTERFACE, E_POINTER, HANDLE, MAX_PATH, S_OK, WAIT_OBJECT_0,
};
use windows::Win32::Media::Audio::{
    ActivateAudioInterfaceAsync, AudioSessionStateActive, AudioSessionStateExpired,
    AudioSessionStateInactive, IActivateAudioInterfaceAsyncOperation,
    IActivateAudioInterfaceCompletionHandler, IActivateAudioInterfaceCompletionHandler_Vtbl,
    IAudioCaptureClient, IAudioClient, IAudioSessionControl, IAudioSessionControl2,
    IAudioSessionEnumerator, IAudioSessionManager2, IMMDeviceEnumerator, ISimpleAudioVolume,
    MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
    AUDCLNT_STREAMFLAGS_LOOPBACK, AUDIOCLIENT_ACTIVATION_PARAMS,
    AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
    AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, DEVICE_STATE_ACTIVE,
    PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
    WAVEFORMATEX, eRender,
};
use windows::Win32::System::Com::IAgileObject;
use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, BLOB, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{
    CreateEventW, OpenProcess, QueryFullProcessImageNameW, WaitForSingleObject,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::System::Variant::VT_BLOB;

const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
const CAPTURE_DURATION: Duration = Duration::from_secs(5);
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;

struct ComGuard;

impl ComGuard {
    fn init_mta() -> WinResult<Self> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok()?;
        Ok(Self)
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

#[derive(Debug, Clone)]
struct AudioSessionInfo {
    pid: u32,
    exe_name: String,
    is_active: bool,
    peak: f32,
    volume: f32,
    muted: bool,
    endpoint_index: u32,
}

fn query_process_exe_name(pid: u32) -> String {
    if pid == 0 {
        return "[System Sounds]".to_string();
    }
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return format!("<pid:{pid}>");
        };
        let mut buf = [0u16; MAX_PATH as usize];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(handle);

        if !ok || len == 0 {
            return format!("<pid:{pid}>");
        }
        let full = OsString::from_wide(&buf[..len as usize]);
        Path::new(&full)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| full.to_string_lossy().into_owned())
    }
}

fn enumerate_audio_sessions() -> WinResult<Vec<AudioSessionInfo>> {
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }?;
    let collection = unsafe { enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) }?;
    let dev_count = unsafe { collection.GetCount() }?;

    let mut sessions = Vec::new();

    for dev_idx in 0..dev_count {
        let Ok(device) = (unsafe { collection.Item(dev_idx) }) else {
            continue;
        };
        let Ok(session_mgr) =
            (unsafe { device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) })
        else {
            continue;
        };
        let Ok(session_enum): Result<IAudioSessionEnumerator, _> =
            (unsafe { session_mgr.GetSessionEnumerator() })
        else {
            continue;
        };
        let count = unsafe { session_enum.GetCount() }.unwrap_or(0);

        for i in 0..count {
            let Ok(ctrl): Result<IAudioSessionControl, _> = (unsafe { session_enum.GetSession(i) })
            else {
                continue;
            };
            let Ok(ctrl2) = ctrl.cast::<IAudioSessionControl2>() else {
                continue;
            };
            let pid = unsafe { ctrl2.GetProcessId() }.unwrap_or(0);
            let state = unsafe { ctrl.GetState() }.unwrap_or(AudioSessionStateExpired);
            let is_active = state == AudioSessionStateActive;

            let peak = ctrl
                .cast::<IAudioMeterInformation>()
                .ok()
                .and_then(|m| unsafe { m.GetPeakValue() }.ok())
                .unwrap_or(0.0);

            if !is_active && peak <= 0.0001 {
                continue;
            }
            if state == AudioSessionStateInactive && peak <= 0.0001 {
                continue;
            }

            let (volume, muted) = ctrl
                .cast::<ISimpleAudioVolume>()
                .ok()
                .map(|v| {
                    let vol = unsafe { v.GetMasterVolume() }.unwrap_or(1.0);
                    let mute = unsafe { v.GetMute() }.map(|b| b.as_bool()).unwrap_or(false);
                    (vol, mute)
                })
                .unwrap_or((1.0, false));

            sessions.push(AudioSessionInfo {
                pid,
                exe_name: query_process_exe_name(pid),
                is_active,
                peak,
                volume,
                muted,
                endpoint_index: dev_idx,
            });
        }
    }

    // Deduplicate by (pid, endpoint_index) keeping highest peak
    let mut dedup: BTreeMap<(u32, u32), AudioSessionInfo> = BTreeMap::new();
    for s in sessions {
        let key = (s.pid, s.endpoint_index);
        dedup
            .entry(key)
            .and_modify(|existing| {
                if s.peak > existing.peak {
                    existing.peak = s.peak;
                }
                existing.is_active |= s.is_active;
            })
            .or_insert(s);
    }

    let mut result: Vec<AudioSessionInfo> = dedup.into_values().collect();
    result.sort_by(|a, b| {
        b.is_active
            .cmp(&a.is_active)
            .then_with(|| b.peak.total_cmp(&a.peak))
            .then_with(|| a.exe_name.to_lowercase().cmp(&b.exe_name.to_lowercase()))
    });
    Ok(result)
}

fn matches_target(exe_name: &str, target: &str) -> bool {
    let exe_l = exe_name.to_lowercase();
    let tgt_l = target.to_lowercase();
    if exe_l == tgt_l {
        return true;
    }
    let tgt_with_exe = if tgt_l.ends_with(".exe") {
        tgt_l
    } else {
        format!("{tgt_l}.exe")
    };
    exe_l == tgt_with_exe
}

fn find_best_pid_for_target(target: &str) -> WinResult<Option<AudioSessionInfo>> {
    if let Ok(explicit_pid) = target.parse::<u32>() {
        let sessions = enumerate_audio_sessions()?;
        if let Some(found) = sessions.into_iter().find(|s| s.pid == explicit_pid) {
            return Ok(Some(found));
        }
        return Ok(Some(AudioSessionInfo {
            pid: explicit_pid,
            exe_name: query_process_exe_name(explicit_pid),
            is_active: true,
            peak: 0.0,
            volume: 1.0,
            muted: false,
            endpoint_index: 0,
        }));
    }

    let sessions = enumerate_audio_sessions()?;
    let best = sessions
        .into_iter()
        .filter(|s| s.pid != 0 && matches_target(&s.exe_name, target))
        .max_by(|a, b| {
            a.is_active
                .cmp(&b.is_active)
                .then_with(|| a.peak.total_cmp(&b.peak))
        });
    Ok(best)
}

#[derive(Default)]
struct ActivationState {
    done: bool,
    result: Option<WinResult<IAudioClient>>,
}

#[repr(C)]
struct ActivationHandler {
    vtable: *const IActivateAudioInterfaceCompletionHandler_Vtbl,
    ref_count: AtomicU32,
    state: Arc<(Mutex<ActivationState>, Condvar)>,
}

impl ActivationHandler {
    const VTABLE: IActivateAudioInterfaceCompletionHandler_Vtbl =
        IActivateAudioInterfaceCompletionHandler_Vtbl {
            base__: windows::core::IUnknown_Vtbl {
                QueryInterface: Self::query_interface,
                AddRef: Self::add_ref,
                Release: Self::release,
            },
            ActivateCompleted: Self::activate_completed,
        };

    fn new_interface(
        state: Arc<(Mutex<ActivationState>, Condvar)>,
    ) -> IActivateAudioInterfaceCompletionHandler {
        let boxed = Box::new(Self {
            vtable: &Self::VTABLE,
            ref_count: AtomicU32::new(1),
            state,
        });
        let raw = Box::into_raw(boxed).cast::<c_void>();
        unsafe { IActivateAudioInterfaceCompletionHandler::from_raw(raw) }
    }

    unsafe extern "system" fn query_interface(
        this: *mut c_void,
        iid: *const GUID,
        interface: *mut *mut c_void,
    ) -> HRESULT {
        if interface.is_null() || iid.is_null() {
            return E_POINTER;
        }
        let iid_ref = unsafe { &*iid };
        if *iid_ref == IUnknown::IID
            || *iid_ref == IActivateAudioInterfaceCompletionHandler::IID
            || *iid_ref == IAgileObject::IID
        {
            unsafe {
                *interface = this;
                Self::add_ref(this);
            }
            S_OK
        } else {
            unsafe { *interface = std::ptr::null_mut() };
            E_NOINTERFACE
        }
    }

    unsafe extern "system" fn add_ref(this: *mut c_void) -> u32 {
        let me = unsafe { &*(this.cast::<Self>()) };
        me.ref_count.fetch_add(1, Ordering::Relaxed) + 1
    }

    unsafe extern "system" fn release(this: *mut c_void) -> u32 {
        let me = unsafe { &*(this.cast::<Self>()) };
        let prev = me.ref_count.fetch_sub(1, Ordering::Release);
        if prev == 1 {
            std::sync::atomic::fence(Ordering::Acquire);
            unsafe { drop(Box::from_raw(this.cast::<Self>())) };
            0
        } else {
            prev - 1
        }
    }

    unsafe extern "system" fn activate_completed(
        this: *mut c_void,
        activateoperation: *mut c_void,
    ) -> HRESULT {
        let me = unsafe { &*(this.cast::<Self>()) };
        let res = (|| -> WinResult<IAudioClient> {
            if activateoperation.is_null() {
                return Err(windows::core::Error::from(E_POINTER));
            }
            let op: IActivateAudioInterfaceAsyncOperation =
                unsafe { Interface::from_raw(activateoperation) };
            // Borrowed parameter from COM caller; prevent releasing it on drop.
            let op_no_drop = ManuallyDrop::new(op);
            let mut activate_hr = HRESULT(0);
            let mut activated_unk: Option<IUnknown> = None;
            unsafe { op_no_drop.GetActivateResult(&mut activate_hr, &mut activated_unk) }?;
            activate_hr.ok()?;
            let unk = activated_unk.ok_or_else(|| windows::core::Error::from(HRESULT(-1)))?;
            unk.cast::<IAudioClient>()
        })();

        let (lock, cv) = &*me.state;
        let mut guard = lock.lock().expect("activation mutex poisoned");
        guard.done = true;
        guard.result = Some(res);
        cv.notify_all();
        S_OK
    }
}

fn activate_process_loopback(pid: u32) -> WinResult<IAudioClient> {
    let mut activation_params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: pid,
                ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
            },
        },
    };

    let mut prop_variant = ManuallyDrop::new(PROPVARIANT {
        Anonymous: PROPVARIANT_0 {
            Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_BLOB,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: windows::Win32::System::Com::StructuredStorage::PROPVARIANT_0_0_0 {
                    blob: BLOB {
                        cbSize: size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                        pBlobData: (&mut activation_params as *mut AUDIOCLIENT_ACTIVATION_PARAMS)
                            .cast::<u8>(),
                    },
                },
            }),
        },
    });

    let state = Arc::new((Mutex::new(ActivationState::default()), Condvar::new()));
    let handler = ActivationHandler::new_interface(Arc::clone(&state));

    let _op = unsafe {
        ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(&mut *prop_variant),
            &handler,
        )
    }?;

    // Note: prop_variant points to stack-allocated activation_params; we wait here before returning.
    let (lock, cv) = &*state;
    let mut guard = lock.lock().expect("activation mutex poisoned");
    while !guard.done {
        let (g, timeout) = cv
            .wait_timeout(guard, Duration::from_secs(5))
            .expect("activation condvar poisoned");
        guard = g;
        if timeout.timed_out() && !guard.done {
            return Err(windows::core::Error::from(HRESULT(-1)));
        }
    }

    guard
        .result
        .take()
        .unwrap_or_else(|| Err(windows::core::Error::from(HRESULT(-1))))
}

struct ProcessLoopbackStream {
    pid: u32,
    audio_client: IAudioClient,
    capture_client: IAudioCaptureClient,
    event_handle: HANDLE,
}

impl ProcessLoopbackStream {
    fn open(pid: u32) -> WinResult<Self> {
        let audio_client = activate_process_loopback(pid)?;

        let block_align = CHANNELS * 4;
        let wfx = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
            nChannels: CHANNELS,
            nSamplesPerSec: SAMPLE_RATE,
            nAvgBytesPerSec: SAMPLE_RATE * u32::from(block_align),
            nBlockAlign: block_align,
            wBitsPerSample: 32,
            cbSize: 0,
        };

        let flags = AUDCLNT_STREAMFLAGS_LOOPBACK
            | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
            | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM;

        // 40 ms buffer in 100-ns units
        unsafe {
            audio_client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                flags,
                400_000,
                0,
                &wfx,
                None,
            )
        }?;

        let event_handle = unsafe { CreateEventW(None, false, false, None) }?;
        if let Err(e) = unsafe { audio_client.SetEventHandle(event_handle) } {
            let _ = unsafe { CloseHandle(event_handle) };
            return Err(e);
        }

        let capture_client: IAudioCaptureClient = match unsafe { audio_client.GetService() } {
            Ok(c) => c,
            Err(e) => {
                let _ = unsafe { CloseHandle(event_handle) };
                return Err(e);
            }
        };

        if let Err(e) = unsafe { audio_client.Start() } {
            let _ = unsafe { CloseHandle(event_handle) };
            return Err(e);
        }

        Ok(Self {
            pid,
            audio_client,
            capture_client,
            event_handle,
        })
    }
}

impl Drop for ProcessLoopbackStream {
    fn drop(&mut self) {
        let _ = unsafe { self.audio_client.Stop() };
        let _ = unsafe { CloseHandle(self.event_handle) };
    }
}

fn write_wav_i16(path: &Path, samples_f32: &[f32], sample_rate: u32, channels: u16) -> std::io::Result<()> {
    let file = File::create(path)?;
    let mut w = BufWriter::new(file);

    let data_bytes = (samples_f32.len() * 2) as u32;
    let riff_size = 36 + data_bytes;
    let block_align = channels * 2;
    let byte_rate = sample_rate * u32::from(block_align);

    w.write_all(b"RIFF")?;
    w.write_all(&riff_size.to_le_bytes())?;
    w.write_all(b"WAVE")?;
    w.write_all(b"fmt ")?;
    w.write_all(&16u32.to_le_bytes())?;
    w.write_all(&1u16.to_le_bytes())?; // PCM
    w.write_all(&channels.to_le_bytes())?;
    w.write_all(&sample_rate.to_le_bytes())?;
    w.write_all(&byte_rate.to_le_bytes())?;
    w.write_all(&block_align.to_le_bytes())?;
    w.write_all(&16u16.to_le_bytes())?;
    w.write_all(b"data")?;
    w.write_all(&data_bytes.to_le_bytes())?;

    for &s in samples_f32 {
        let clamped = s.clamp(-1.0, 1.0);
        let i16_val = (clamped * 32767.0).round() as i16;
        w.write_all(&i16_val.to_le_bytes())?;
    }
    w.flush()?;
    Ok(())
}

fn print_sessions(sessions: &[AudioSessionInfo]) {
    println!("=== Active Audio Sessions ({}) ===", sessions.len());
    println!(
        "{:<8} {:<30} {:<8} {:<10} {:<8} {}",
        "PID", "Process", "State", "Peak", "Volume", "Muted"
    );
    for s in sessions {
        let state_str = if s.is_active { "Active" } else { "PeakOnly" };
        println!(
            "{:<8} {:<30} {:<8} {:<10.5} {:<7.0}% {}",
            s.pid,
            s.exe_name,
            state_str,
            s.peak,
            s.volume * 100.0,
            if s.muted { "YES" } else { "no" }
        );
    }
}

fn main() {
    let _com = ComGuard::init_mta().expect("CoInitializeEx(MTA) failed");

    let sessions = enumerate_audio_sessions().expect("Failed to enumerate audio sessions");
    print_sessions(&sessions);

    let Some(target) = env::args().nth(1) else {
        println!("\nTip: pass a process name or PID to capture 5s, e.g.:");
        println!("  cargo run -p core-audio --example test_app_loopback -- zen.exe");
        return;
    };

    println!("\nLooking for active audio session matching '{target}'...");
    let initial = loop {
        match find_best_pid_for_target(&target) {
            Ok(Some(info)) => break info,
            Ok(None) => {
                eprintln!("No active audio session found for '{target}' yet; retrying in 500ms...");
                std::thread::sleep(Duration::from_millis(500));
            }
            Err(e) => {
                eprintln!("Session enumeration error: {e:?}");
                return;
            }
        }
    };

    println!(
        "Attaching process loopback to {} (PID={}, peak={:.4})...",
        initial.exe_name, initial.pid, initial.peak
    );

    let mut stream = match ProcessLoopbackStream::open(initial.pid) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to open process loopback for PID {}: {e:?}", initial.pid);
            return;
        }
    };

    let mut samples: Vec<f32> = Vec::with_capacity(SAMPLE_RATE as usize * CHANNELS as usize * 6);
    let mut real_frames: u64 = 0;
    let mut total_frames: u64 = 0;
    let mut peak_abs: f32 = 0.0;

    let start = Instant::now();
    let mut last_recheck = Instant::now();
    let mut last_nonzero_packet = Instant::now();

    while start.elapsed() < CAPTURE_DURATION {
        let wait_res = unsafe { WaitForSingleObject(stream.event_handle, 50) };
        if wait_res == WAIT_OBJECT_0 {
            loop {
                let packet_len = match unsafe { stream.capture_client.GetNextPacketSize() } {
                    Ok(n) => n,
                    Err(e) => {
                        eprintln!("GetNextPacketSize error: {e:?}");
                        break;
                    }
                };
                if packet_len == 0 {
                    break;
                }

                let mut data_ptr: *mut u8 = std::ptr::null_mut();
                let mut frames_read = 0u32;
                let mut flags = 0u32;

                if let Err(e) = unsafe {
                    stream.capture_client.GetBuffer(
                        &mut data_ptr,
                        &mut frames_read,
                        &mut flags,
                        None,
                        None,
                    )
                } {
                    eprintln!("GetBuffer error: {e:?}");
                    break;
                }

                let frame_count = frames_read as usize;
                let sample_count = frame_count * CHANNELS as usize;
                total_frames += frames_read as u64;

                let is_silent = (flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;
                if is_silent || data_ptr.is_null() {
                    samples.resize(samples.len() + sample_count, 0.0);
                } else {
                    let slice = unsafe {
                        std::slice::from_raw_parts(data_ptr.cast::<f32>(), sample_count)
                    };
                    let mut packet_peak = 0.0f32;
                    for &s in slice {
                        let a = s.abs();
                        if a > packet_peak {
                            packet_peak = a;
                        }
                    }
                    if packet_peak > peak_abs {
                        peak_abs = packet_peak;
                    }
                    if packet_peak > 1e-6 {
                        real_frames += frames_read as u64;
                        last_nonzero_packet = Instant::now();
                    }
                    samples.extend_from_slice(slice);
                }

                let _ = unsafe { stream.capture_client.ReleaseBuffer(frames_read) };
            }
        }

        // Auto-rebind if another PID for the same app becomes active (e.g. browser/Discord subprocesses or session recreation)
        if last_recheck.elapsed() >= Duration::from_millis(350) {
            last_recheck = Instant::now();
            if let Ok(Some(best)) = find_best_pid_for_target(&target) {
                let current_idle = last_nonzero_packet.elapsed() >= Duration::from_millis(350);
                if best.pid != stream.pid && (best.peak > 0.0001 || current_idle) {
                    println!(
                        "Detected active PID change for '{}': {} -> {} (peak={:.4}), re-attaching...",
                        target, stream.pid, best.pid, best.peak
                    );
                    if let Ok(new_stream) = ProcessLoopbackStream::open(best.pid) {
                        stream = new_stream;
                    }
                }
            }
        }
    }

    let peak_dbfs = if peak_abs > 1e-9 {
        20.0 * peak_abs.log10()
    } else {
        f32::NEG_INFINITY
    };

    let out_path = Path::new("test_app_audio.wav");
    write_wav_i16(out_path, &samples, SAMPLE_RATE, CHANNELS)
        .expect("Failed to write test_app_audio.wav");

    println!("\n=== Capture Complete ===");
    println!("output_file: {}", out_path.display());
    println!("total_frames: {total_frames}");
    println!("real_frames: {real_frames}");
    println!("peak_dbfs: {peak_dbfs:.2} dBFS");
}
