//! Per-application WASAPI process loopback capture (`ActivateAudioInterfaceAsync`)
//! and Windows audio session enumeration (`IAudioSessionManager2` / `IAudioSessionEnumerator`).

use std::collections::BTreeMap;
use std::ffi::{c_void, OsString};
use std::mem::{size_of, ManuallyDrop};
use std::os::windows::ffi::OsStringExt;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use core_types::{ActiveAudioAppInfo, AudioError};
use parking_lot::{Condvar, Mutex};
use windows::core::{IUnknown, Interface, Result as WinResult, GUID, HRESULT};
use windows::Win32::Foundation::{
    CloseHandle, E_NOINTERFACE, E_POINTER, HANDLE, MAX_PATH, S_OK, WAIT_OBJECT_0,
};
use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
use windows::Win32::Media::Audio::{
    eRender, ActivateAudioInterfaceAsync, AudioSessionStateActive, AudioSessionStateExpired,
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
    WAVEFORMATEX,
};
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, IAgileObject, BLOB, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{
    CreateEventW, GetExitCodeProcess, OpenProcess, QueryFullProcessImageNameW, WaitForSingleObject,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::System::Variant::VT_BLOB;

const PROCESS_LOOPBACK_SAMPLE_RATE: u32 = 48_000;
const PROCESS_LOOPBACK_CHANNELS: u16 = 2;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const STILL_ACTIVE_CODE: u32 = 259;

/// Active Windows audio session metadata from `IAudioSessionEnumerator`.
#[derive(Debug, Clone)]
pub struct AudioSessionInfo {
    pub pid: u32,
    pub exe_name: String,
    pub is_active: bool,
    pub peak: f32,
    pub volume: f32,
    pub muted: bool,
    pub endpoint_index: u32,
}

/// Resolves the executable filename (e.g. `Discord.exe`) for a Windows PID.
#[must_use]
pub fn query_process_exe_name(pid: u32) -> String {
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

/// Returns `true` if `pid` is still running.
#[must_use]
pub fn is_pid_running(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut exit_code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut exit_code).is_ok();
        let _ = CloseHandle(handle);
        ok && exit_code == STILL_ACTIVE_CODE
    }
}

/// Enumerates all active Windows audio sessions across active render endpoints.
pub fn enumerate_audio_sessions() -> Result<Vec<AudioSessionInfo>, AudioError> {
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };

    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }.map_err(|e| {
            AudioError::WasapiInitFailed(format!("CoCreateInstance(MMDeviceEnumerator) failed: {e}"))
        })?;
    let collection = unsafe { enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) }
        .map_err(|e| AudioError::WasapiInitFailed(format!("EnumAudioEndpoints failed: {e}")))?;
    let dev_count = unsafe { collection.GetCount() }.unwrap_or(0);

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

/// Returns deduplicated running applications with active audio sessions for UI pickers.
pub fn list_active_audio_apps() -> Result<Vec<ActiveAudioAppInfo>, AudioError> {
    let sessions = enumerate_audio_sessions()?;
    let mut by_exe: BTreeMap<String, ActiveAudioAppInfo> = BTreeMap::new();
    for s in sessions {
        if s.pid == 0 || s.exe_name.starts_with('<') {
            continue;
        }
        let key = s.exe_name.to_ascii_lowercase();
        if key == "ikoch.exe" || key == "recorder-daemon.exe" {
            continue;
        }
        by_exe
            .entry(key)
            .and_modify(|existing| {
                existing.is_active |= s.is_active;
                if s.peak > existing.peak {
                    existing.peak = s.peak;
                    existing.pid = s.pid;
                }
            })
            .or_insert(ActiveAudioAppInfo {
                pid: s.pid,
                exe_name: s.exe_name,
                is_active: s.is_active,
                peak: s.peak,
            });
    }

    let mut result: Vec<ActiveAudioAppInfo> = by_exe.into_values().collect();
    result.sort_by(|a, b| {
        b.is_active
            .cmp(&a.is_active)
            .then_with(|| b.peak.total_cmp(&a.peak))
            .then_with(|| a.exe_name.to_lowercase().cmp(&b.exe_name.to_lowercase()))
    });
    Ok(result)
}

#[must_use]
pub fn matches_process_name(exe_name: &str, target: &str) -> bool {
    let exe_l = exe_name.trim().to_ascii_lowercase();
    let tgt_l = target.trim().to_ascii_lowercase();
    if tgt_l.is_empty() {
        return false;
    }
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

/// Finds the best active audio session PID matching any executable name in `process_names`.
pub fn find_best_pid_for_processes(
    process_names: &[String],
) -> Result<Option<AudioSessionInfo>, AudioError> {
    if process_names.is_empty() {
        return Ok(None);
    }
    let sessions = enumerate_audio_sessions()?;
    let best = sessions
        .into_iter()
        .filter(|s| {
            s.pid != 0
                && process_names
                    .iter()
                    .any(|target| matches_process_name(&s.exe_name, target))
        })
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
            let op_no_drop = ManuallyDrop::new(op);
            let mut activate_hr = HRESULT(0);
            let mut activated_unk: Option<IUnknown> = None;
            unsafe { op_no_drop.GetActivateResult(&mut activate_hr, &mut activated_unk) }?;
            activate_hr.ok()?;
            let unk = activated_unk.ok_or_else(|| windows::core::Error::from(HRESULT(-1)))?;
            unk.cast::<IAudioClient>()
        })();

        let (lock, cv) = &*me.state;
        let mut guard = lock.lock();
        guard.done = true;
        guard.result = Some(res);
        cv.notify_all();
        S_OK
    }
}

fn activate_process_loopback(pid: u32) -> Result<IAudioClient, AudioError> {
    let mut activation_params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: pid,
                ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
            },
        },
    };

    // ManuallyDrop prevents PropVariantClear from calling CoTaskMemFree on stack memory.
    let mut prop_variant = ManuallyDrop::new(PROPVARIANT {
        Anonymous: PROPVARIANT_0 {
            Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_BLOB,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: PROPVARIANT_0_0_0 {
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
    }
    .map_err(|e| {
        AudioError::WasapiInitFailed(format!(
            "ActivateAudioInterfaceAsync(PID={pid}) failed: {e}"
        ))
    })?;

    let (lock, cv) = &*state;
    let mut guard = lock.lock();
    while !guard.done {
        let wait_res = cv.wait_for(&mut guard, Duration::from_secs(5));
        if wait_res.timed_out() && !guard.done {
            return Err(AudioError::WasapiInitFailed(format!(
                "ActivateAudioInterfaceAsync(PID={pid}) timed out"
            )));
        }
    }

    guard
        .result
        .take()
        .unwrap_or_else(|| Err(windows::core::Error::from(HRESULT(-1))))
        .map_err(|e| {
            AudioError::WasapiInitFailed(format!(
                "Process loopback activation failed for PID {pid}: {e}"
            ))
        })
}

/// Event-driven WASAPI process loopback stream outputting 48 kHz / 2 ch / f32 PCM.
pub struct ProcessLoopbackStream {
    pid: u32,
    audio_client: IAudioClient,
    capture_client: IAudioCaptureClient,
    event_handle: HANDLE,
}

unsafe impl Send for ProcessLoopbackStream {}

impl ProcessLoopbackStream {
    /// Opens a process loopback stream for `pid` (including its child process tree).
    pub fn open(pid: u32) -> Result<Self, AudioError> {
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let audio_client = activate_process_loopback(pid)?;

        let block_align = PROCESS_LOOPBACK_CHANNELS * 4;
        let wfx = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
            nChannels: PROCESS_LOOPBACK_CHANNELS,
            nSamplesPerSec: PROCESS_LOOPBACK_SAMPLE_RATE,
            nAvgBytesPerSec: PROCESS_LOOPBACK_SAMPLE_RATE * u32::from(block_align),
            nBlockAlign: block_align,
            wBitsPerSample: 32,
            cbSize: 0,
        };

        let flags = AUDCLNT_STREAMFLAGS_LOOPBACK
            | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
            | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM;

        unsafe {
            audio_client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 400_000, 0, &wfx, None)
        }
        .map_err(|e| {
            AudioError::WasapiInitFailed(format!(
                "Process loopback IAudioClient::Initialize(PID={pid}) failed: {e}"
            ))
        })?;

        let event_handle = unsafe { CreateEventW(None, false, false, None) }.map_err(|e| {
            AudioError::WasapiInitFailed(format!("CreateEventW failed for PID {pid}: {e}"))
        })?;

        if let Err(e) = unsafe { audio_client.SetEventHandle(event_handle) } {
            let _ = unsafe { CloseHandle(event_handle) };
            return Err(AudioError::WasapiInitFailed(format!(
                "SetEventHandle failed for PID {pid}: {e}"
            )));
        }

        let capture_client: IAudioCaptureClient = match unsafe { audio_client.GetService() } {
            Ok(c) => c,
            Err(e) => {
                let _ = unsafe { CloseHandle(event_handle) };
                return Err(AudioError::WasapiInitFailed(format!(
                    "GetService<IAudioCaptureClient> failed for PID {pid}: {e}"
                )));
            }
        };

        if let Err(e) = unsafe { audio_client.Start() } {
            let _ = unsafe { CloseHandle(event_handle) };
            return Err(AudioError::WasapiInitFailed(format!(
                "IAudioClient::Start failed for PID {pid}: {e}"
            )));
        }

        Ok(Self {
            pid,
            audio_client,
            capture_client,
            event_handle,
        })
    }

    #[must_use]
    pub const fn pid(&self) -> u32 {
        self.pid
    }

    /// Waits up to `timeout_ms` for a WASAPI event and appends interleaved 48 kHz stereo `f32` samples.
    /// Returns `(total_frames_read, non_silent_frames_read)`.
    pub fn wait_and_read(
        &mut self,
        timeout_ms: u32,
        out_f32: &mut Vec<f32>,
    ) -> Result<(u64, u64), AudioError> {
        let wait_res = unsafe { WaitForSingleObject(self.event_handle, timeout_ms) };
        if wait_res != WAIT_OBJECT_0 {
            return Ok((0, 0));
        }

        let mut total_frames = 0u64;
        let mut real_frames = 0u64;

        loop {
            let packet_len = unsafe { self.capture_client.GetNextPacketSize() }.map_err(|e| {
                AudioError::DeviceDisconnected {
                    device: format!("Process loopback GetNextPacketSize(PID={}) failed: {e}", self.pid),
                }
            })?;
            if packet_len == 0 {
                break;
            }

            let mut data_ptr: *mut u8 = std::ptr::null_mut();
            let mut frames_read = 0u32;
            let mut flags = 0u32;

            unsafe {
                self.capture_client.GetBuffer(
                    &mut data_ptr,
                    &mut frames_read,
                    &mut flags,
                    None,
                    None,
                )
            }
            .map_err(|e| AudioError::DeviceDisconnected {
                device: format!("Process loopback GetBuffer(PID={}) failed: {e}", self.pid),
            })?;

            let frame_count = frames_read as usize;
            let sample_count = frame_count * usize::from(PROCESS_LOOPBACK_CHANNELS);
            total_frames += u64::from(frames_read);

            let is_silent = (flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;
            if is_silent || data_ptr.is_null() {
                out_f32.resize(out_f32.len() + sample_count, 0.0);
            } else {
                let slice =
                    unsafe { std::slice::from_raw_parts(data_ptr.cast::<f32>(), sample_count) };
                if slice.iter().any(|&s| s.abs() > 1e-6) {
                    real_frames += u64::from(frames_read);
                }
                out_f32.extend_from_slice(slice);
            }

            let _ = unsafe { self.capture_client.ReleaseBuffer(frames_read) };
        }

        Ok((total_frames, real_frames))
    }
}

impl Drop for ProcessLoopbackStream {
    fn drop(&mut self) {
        let _ = unsafe { self.audio_client.Stop() };
        let _ = unsafe { CloseHandle(self.event_handle) };
    }
}
