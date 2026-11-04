//! WASAPI capture sources.
//!
//! * **Process loopback** – `ActivateAudioInterfaceAsync` on the virtual
//!   `VAD\Process_Loopback` device with `AUDIOCLIENT_ACTIVATION_PARAMS`
//!   (Windows 10 2004+). Captures only the target process tree, so desktop
//!   notification sounds never bleed into the game track.
//! * **System loopback** – the default render endpoint with
//!   `AUDCLNT_STREAMFLAGS_LOOPBACK` (fallback for older Windows builds).
//! * **Microphone** – a capture endpoint.
//!
//! Every client is initialised in shared mode with `AUTOCONVERTPCM`, asking
//! the audio engine for 48 kHz stereo IEEE float so no resampling happens
//! on our side.

use crate::buffer::SampleRing;
use crate::engine::{CaptureHandle, CaptureTarget, SourceFactory};
use crate::AudioDevice;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use windows::core::{implement, Interface, HSTRING, PCWSTR};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    eCapture, eCommunications, eConsole, eRender, ActivateAudioInterfaceAsync,
    IActivateAudioInterfaceAsyncOperation, IActivateAudioInterfaceCompletionHandler,
    IActivateAudioInterfaceCompletionHandler_Impl, IAudioCaptureClient, IAudioClient,
    IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
    AUDCLNT_STREAMFLAGS_LOOPBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
    AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
    AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
    DEVICE_STATE_ACTIVE, PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
    VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, WAVEFORMATEX,
};
use windows::Win32::System::Com::StructuredStorage::{
    PropVariantToStringAlloc, PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, BLOB, CLSCTX_ALL,
    COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};
use windows::Win32::System::Variant::VT_BLOB;

/// `WAVE_FORMAT_IEEE_FLOAT` (mmreg.h).
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const SAMPLE_RATE: u32 = gc_core::ffmpeg::SAMPLE_RATE;
const CHANNELS: u16 = gc_core::ffmpeg::CHANNELS as u16;
/// 20 ms buffer in 100 ns units.
const BUFFER_HNS: i64 = 200_000;

pub struct WasapiFactory;

impl SourceFactory for WasapiFactory {
    fn open(
        &self,
        target: &CaptureTarget,
        ring: SampleRing,
    ) -> anyhow::Result<Box<dyn CaptureHandle>> {
        let stop = Arc::new(AtomicBool::new(false));
        let alive = Arc::new(AtomicBool::new(true));
        let (ready_tx, ready_rx) = crossbeam_channel::bounded::<anyhow::Result<()>>(1);
        let target = target.clone();
        let thread = {
            let (stop, alive) = (stop.clone(), alive.clone());
            std::thread::Builder::new()
                .name("gc-wasapi".into())
                .spawn(move || {
                    let r = unsafe { capture_thread(&target, &ring, &stop, &ready_tx) };
                    if let Err(e) = r {
                        log::debug!("capture {target:?} ended: {e}");
                        let _ = ready_tx.try_send(Err(e));
                    }
                    alive.store(false, Ordering::SeqCst);
                })?
        };
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Box::new(Capture {
                stop,
                alive,
                thread: Some(thread),
            })),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                stop.store(true, Ordering::SeqCst);
                anyhow::bail!("audio client activation timed out")
            }
        }
    }

    fn supports_process_loopback(&self) -> bool {
        gc_platform::system::supports_process_loopback()
    }
}

struct Capture {
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl CaptureHandle for Capture {
    fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn float_format() -> WAVEFORMATEX {
    let block_align = CHANNELS * 4;
    WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
        nChannels: CHANNELS,
        nSamplesPerSec: SAMPLE_RATE,
        nAvgBytesPerSec: SAMPLE_RATE * block_align as u32,
        nBlockAlign: block_align,
        wBitsPerSample: 32,
        cbSize: 0,
    }
}

struct ComGuard;
impl ComGuard {
    unsafe fn init() -> Self {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        ComGuard
    }
}
impl Drop for ComGuard {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

struct EventHandle(HANDLE);
impl Drop for EventHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

#[implement(IActivateAudioInterfaceCompletionHandler)]
struct ActivationHandler {
    done: HANDLE,
}

impl IActivateAudioInterfaceCompletionHandler_Impl for ActivationHandler_Impl {
    fn ActivateCompleted(
        &self,
        _operation: windows::core::Ref<IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        unsafe { SetEvent(self.done) }
    }
}

/// Activates a process-loopback `IAudioClient` for `pid` and its children.
unsafe fn process_loopback_client(pid: u32) -> anyhow::Result<IAudioClient> {
    let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: pid,
                ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
            },
        },
    };
    let prop = PROPVARIANT {
        Anonymous: PROPVARIANT_0 {
            Anonymous: std::mem::ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_BLOB,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: PROPVARIANT_0_0_0 {
                    blob: BLOB {
                        cbSize: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                        pBlobData: (&mut params as *mut AUDIOCLIENT_ACTIVATION_PARAMS).cast(),
                    },
                },
            }),
        },
    };
    let done = EventHandle(CreateEventW(None, true, false, None)?);
    let handler: IActivateAudioInterfaceCompletionHandler =
        ActivationHandler { done: done.0 }.into();
    let op = ActivateAudioInterfaceAsync(
        VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
        &IAudioClient::IID,
        Some(&prop),
        &handler,
    )?;
    // `prop` only borrows `params`; it must not be dropped as a real
    // PROPVARIANT (that would try to free the blob).
    std::mem::forget(prop);
    anyhow::ensure!(
        WaitForSingleObject(done.0, 5_000) == WAIT_OBJECT_0,
        "process loopback activation timed out"
    );
    let mut hr = windows::core::HRESULT(0);
    let mut unk = None;
    op.GetActivateResult(&mut hr, &mut unk)?;
    hr.ok()?;
    let unk = unk.ok_or_else(|| anyhow::anyhow!("no interface returned"))?;
    Ok(unk.cast::<IAudioClient>()?)
}

unsafe fn endpoint_client(render: bool, device_id: Option<&str>) -> anyhow::Result<IAudioClient> {
    let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let device = match device_id {
        Some(id) => enumerator.GetDevice(&HSTRING::from(id))?,
        None if render => enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?,
        None => enumerator.GetDefaultAudioEndpoint(eCapture, eCommunications)?,
    };
    Ok(device.Activate::<IAudioClient>(CLSCTX_ALL, None)?)
}

unsafe fn capture_thread(
    target: &CaptureTarget,
    ring: &SampleRing,
    stop: &AtomicBool,
    ready: &crossbeam_channel::Sender<anyhow::Result<()>>,
) -> anyhow::Result<()> {
    let _com = ComGuard::init();
    let (client, loopback) = match target {
        CaptureTarget::ProcessTree(pid) => (process_loopback_client(*pid)?, true),
        CaptureTarget::SystemLoopback => (endpoint_client(true, None)?, true),
        CaptureTarget::Microphone(id) => (endpoint_client(false, id.as_deref())?, false),
    };
    let mut flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK
        | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
        | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
    if loopback {
        flags |= AUDCLNT_STREAMFLAGS_LOOPBACK;
    }
    let fmt = float_format();
    client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, BUFFER_HNS, 0, &fmt, None)?;
    let event = EventHandle(CreateEventW(None, false, false, None)?);
    client.SetEventHandle(event.0)?;
    let capture: IAudioCaptureClient = client.GetService()?;
    client.Start()?;
    let _ = ready.try_send(Ok(()));

    let channels = CHANNELS as usize;
    while !stop.load(Ordering::SeqCst) {
        // Event-driven, with a timeout so we also work where loopback
        // clients never signal the event.
        let _ = WaitForSingleObject(event.0, 20);
        loop {
            let mut data = std::ptr::null_mut();
            let mut frames = 0u32;
            let mut buf_flags = 0u32;
            if capture
                .GetBuffer(&mut data, &mut frames, &mut buf_flags, None, None)
                .is_err()
                || frames == 0
            {
                break;
            }
            let n = frames as usize * channels;
            if buf_flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                ring.push_silence(n);
            } else {
                ring.push(std::slice::from_raw_parts(data as *const f32, n));
            }
            capture.ReleaseBuffer(frames)?;
        }
    }
    let _ = client.Stop();
    Ok(())
}

/// Process ids with an audio session on the default playback device, mapped
/// to whether the session is actively playing.
pub fn audio_sessions() -> anyhow::Result<std::collections::HashMap<u32, bool>> {
    use windows::Win32::Media::Audio::{
        AudioSessionStateActive, IAudioSessionControl2, IAudioSessionManager2,
    };
    unsafe {
        let _com = ComGuard::init();
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
        let sessions = manager.GetSessionEnumerator()?;
        let mut out = std::collections::HashMap::new();
        for i in 0..sessions.GetCount()? {
            let Ok(ctl) = sessions.GetSession(i) else {
                continue;
            };
            let Ok(ctl2) = ctl.cast::<IAudioSessionControl2>() else {
                continue;
            };
            let Ok(pid) = ctl2.GetProcessId() else {
                continue;
            };
            if pid == 0 {
                continue; // system sounds
            }
            let active = ctl.GetState().is_ok_and(|s| s == AudioSessionStateActive);
            let e = out.entry(pid).or_insert(false);
            *e |= active;
        }
        Ok(out)
    }
}

pub fn list_capture_devices() -> anyhow::Result<Vec<AudioDevice>> {
    unsafe {
        let _com = ComGuard::init();
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let default_id = enumerator
            .GetDefaultAudioEndpoint(eCapture, eCommunications)
            .and_then(|d| d.GetId())
            .ok()
            .map(|p| {
                let s = p.to_string().unwrap_or_default();
                CoTaskMemFree(Some(p.0 as *const _));
                s
            });
        let coll = enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)?;
        let mut out = Vec::new();
        for i in 0..coll.GetCount()? {
            let dev = coll.Item(i)?;
            let id_p = dev.GetId()?;
            let id = id_p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(id_p.0 as *const _));
            let name = dev
                .OpenPropertyStore(STGM_READ)
                .and_then(|store| store.GetValue(&PKEY_Device_FriendlyName))
                .ok()
                .and_then(|v| {
                    let p = PropVariantToStringAlloc(&v).ok()?;
                    let s = PCWSTR(p.0).to_string().ok();
                    CoTaskMemFree(Some(p.0 as *const _));
                    s
                })
                .unwrap_or_else(|| id.clone());
            out.push(AudioDevice {
                is_default: default_id.as_deref() == Some(id.as_str()),
                id,
                name,
            });
        }
        Ok(out)
    }
}
