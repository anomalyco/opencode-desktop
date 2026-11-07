//! The default microphone through WASAPI, as the 16 kHz mono f32 samples Hex's streams take,
//! for Windows, where Hex can't record for the app.
//!
//! Windows' audio engine converts the endpoint's format to 16 kHz mono float
//! (`AUTOCONVERTPCM`). A driver that refuses that (some refuse anything but their own mix
//! format) records in its mix format, averaged to mono and resampled here. Windows' privacy
//! settings refuse the app with `E_ACCESSDENIED`; a computer with no microphone has no default
//! capture endpoint.
#![allow(unsafe_code, clippy::undocumented_unsafe_blocks)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, E_ACCESSDENIED, HANDLE};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX,
    eCapture, eConsole,
};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree};
use windows::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::core::{HSTRING, PCWSTR};

use crate::STREAM_RATE;
use crate::session::{MIC_BLOCKED, NO_MICROPHONE, Problem};

/// `HRESULT_FROM_WIN32(ERROR_NOT_FOUND)`: no default capture endpoint.
const NOT_FOUND: u32 = 0x8007_0490;
const OPEN_WAIT: Duration = Duration::from_secs(5);
const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// A recording of the default microphone; dropping it stops the microphone.
pub struct Mic {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    heard: Arc<Mutex<Heard>>,
}

#[derive(Default)]
struct Heard {
    samples: Vec<f32>,
    failed: Option<String>,
}

impl Mic {
    /// Opens the default microphone and starts recording, or says why it can't.
    pub fn open() -> Result<Mic, Problem> {
        let stop = Arc::new(AtomicBool::new(false));
        let heard = Arc::new(Mutex::new(Heard::default()));
        let (ready, opened) = mpsc::sync_channel(1);
        let (s, h) = (stop.clone(), heard.clone());
        let worker = std::thread::Builder::new()
            .name("hex-mic".into())
            .spawn(move || record(&s, &h, &ready))
            .map_err(|e| Problem::of(format!("Couldn't start the microphone: {e}")))?;
        let mic = Mic { stop, worker: Some(worker), heard };
        match opened.recv_timeout(OPEN_WAIT) {
            Ok(Ok(())) => Ok(mic),
            Ok(Err(p)) => Err(p),
            Err(_) => Err(Problem::of("The microphone didn't open")),
        }
    }

    /// What it recorded since the last take.
    pub fn take(&self) -> Vec<f32> {
        std::mem::take(&mut self.heard.lock().unwrap_or_else(PoisonError::into_inner).samples)
    }

    /// Why the recording stopped on its own (unplugged, say).
    pub fn failure(&self) -> Option<String> {
        self.heard.lock().unwrap_or_else(PoisonError::into_inner).failed.clone()
    }
}

impl Drop for Mic {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

/// The capture thread: opens, says so, then records until `stop`.
fn record(stop: &AtomicBool, heard: &Mutex<Heard>, ready: &SyncSender<Result<(), Problem>>) {
    // Balanced when the thread ends.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let opened = match open() {
        Ok(o) => o,
        Err(p) => {
            let _ = ready.send(Err(p));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    let mut convert = Resample::new(opened.format.rate);
    if let Err(why) = capture(&opened, stop, heard, &mut convert) {
        heard.lock().unwrap_or_else(PoisonError::into_inner).failed = Some(why);
    }
    let _ = unsafe { opened.client.Stop() };
}

struct Opened {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    format: Format,
    event: Event,
}

struct Event(HANDLE);

impl Drop for Event {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// Whether Windows' privacy settings deny desktop apps the microphone: the device-wide switch,
/// the user's, or the user's for desktop apps. Windows then records silence rather than refusing.
fn denied() -> bool {
    const STORE: &str = r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone";
    let read = |root: HKEY, key: &str| -> Option<String> {
        let (key, value) = (HSTRING::from(key), HSTRING::from("Value"));
        let mut buf = [0u16; 16];
        let mut len = std::mem::size_of_val(&buf) as u32;
        let got = unsafe { RegGetValueW(root, PCWSTR(key.as_ptr()), PCWSTR(value.as_ptr()), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut len)) };
        got.is_ok().then(|| String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)]))
    };
    [(HKEY_LOCAL_MACHINE, STORE.to_owned()), (HKEY_CURRENT_USER, STORE.to_owned()), (HKEY_CURRENT_USER, format!(r"{STORE}\NonPackaged"))]
        .iter()
        .any(|(root, key)| read(*root, key).as_deref() == Some("Deny"))
}

/// The default capture endpoint, started: 16 kHz mono float if the engine converts, else its
/// mix format.
fn open() -> Result<Opened, Problem> {
    if denied() {
        return Err(Problem::of(MIC_BLOCKED));
    }
    let enumerator: IMMDeviceEnumerator = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }.map_err(failed)?;
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(eCapture, eConsole) }.map_err(failed)?;
    let want = Format { tag: WAVE_FORMAT_IEEE_FLOAT, channels: 1, rate: STREAM_RATE, bits: 32, block: 4 };
    let converted = want.wave();
    let auto = AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
    let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }.map_err(failed)?;
    let format = match unsafe { client.Initialize(AUDCLNT_SHAREMODE_SHARED, auto, 0, 0, &converted, None) } {
        Ok(()) => Ok((client, want)),
        Err(e) if e.code() == E_ACCESSDENIED => Err(failed(e)),
        // This driver takes only its mix format, on a fresh client.
        Err(_) => {
            let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }.map_err(failed)?;
            let mix = unsafe { client.GetMixFormat() }.map_err(failed)?;
            let format = unsafe { Format::read(mix) };
            let init = unsafe { client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, 0, 0, mix, None) };
            unsafe { CoTaskMemFree(Some(mix.cast_const().cast())) };
            init.map_err(failed)?;
            format.map(|f| (client, f)).ok_or_else(|| Problem::of("The microphone's format isn't one Hex can take"))
        }
    };
    let (client, format) = format?;
    let event = Event(unsafe { CreateEventW(None, false, false, PCWSTR::null()) }.map_err(failed)?);
    unsafe { client.SetEventHandle(event.0) }.map_err(failed)?;
    let capture: IAudioCaptureClient = unsafe { client.GetService() }.map_err(failed)?;
    unsafe { client.Start() }.map_err(failed)?;
    Ok(Opened { client, capture, format, event })
}

/// The app's words for what Windows refused with.
fn failed(e: windows::core::Error) -> Problem {
    match e.code() {
        E_ACCESSDENIED => Problem::of(MIC_BLOCKED),
        c if c.0 as u32 == NOT_FOUND => Problem::of(NO_MICROPHONE),
        c => Problem::of(format!("The microphone didn't open ({:#010x})", c.0 as u32)),
    }
}

/// Moves packets into `heard` until `stop`, or a failure, which it returns.
fn capture(o: &Opened, stop: &AtomicBool, heard: &Mutex<Heard>, convert: &mut Resample) -> Result<(), String> {
    let lost = |e: windows::core::Error| format!("The microphone stopped ({:#010x})", e.code().0 as u32);
    while !stop.load(Ordering::Acquire) {
        unsafe { WaitForSingleObject(o.event.0, 100) };
        loop {
            let packet = unsafe { o.capture.GetNextPacketSize() }.map_err(lost)?;
            if packet == 0 {
                break;
            }
            let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0u32, 0u32);
            unsafe { o.capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None) }.map_err(lost)?;
            let n = frames as usize;
            let mono = if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                vec![0.0; n]
            } else {
                // SAFETY: GetBuffer handed `frames` frames of `block` bytes each.
                let bytes = unsafe { std::slice::from_raw_parts(data, n * usize::from(o.format.block)) };
                o.format.mono(bytes)
            };
            unsafe { o.capture.ReleaseBuffer(frames) }.map_err(lost)?;
            let samples = convert.push(&mono);
            heard.lock().unwrap_or_else(PoisonError::into_inner).samples.extend(samples);
        }
    }
    Ok(())
}

/// A PCM or float format, from its `WAVEFORMATEX` (or `WAVEFORMATEXTENSIBLE`).
#[derive(Clone, Copy)]
struct Format {
    /// `WAVE_FORMAT_PCM` or `WAVE_FORMAT_IEEE_FLOAT`; an extensible format's subtype.
    tag: u16,
    channels: u16,
    rate: u32,
    bits: u16,
    block: u16,
}

impl Format {
    /// SAFETY: `f` is a `WAVEFORMATEX` from `GetMixFormat`, followed by its `cbSize` bytes.
    unsafe fn read(f: *const WAVEFORMATEX) -> Option<Format> {
        let w = unsafe { f.read_unaligned() };
        let mut tag = w.wFormatTag;
        if tag == WAVE_FORMAT_EXTENSIBLE && w.cbSize >= 22 {
            // The subtype GUID's first two bytes are the format tag: after cbSize come the
            // valid bits (2), the channel mask (4), then the GUID.
            let sub = unsafe { f.cast::<u8>().add(std::mem::size_of::<WAVEFORMATEX>() + 6).cast::<u16>().read_unaligned() };
            tag = sub;
        }
        let ok = matches!((tag, w.wBitsPerSample), (WAVE_FORMAT_IEEE_FLOAT, 32) | (WAVE_FORMAT_PCM, 16 | 24 | 32));
        (ok && w.nChannels > 0).then_some(Format { tag, channels: w.nChannels, rate: w.nSamplesPerSec, bits: w.wBitsPerSample, block: w.nBlockAlign })
    }

    /// The plain `WAVEFORMATEX` for this format.
    fn wave(self) -> WAVEFORMATEX {
        WAVEFORMATEX {
            wFormatTag: self.tag,
            nChannels: self.channels,
            nSamplesPerSec: self.rate,
            nAvgBytesPerSec: self.rate * u32::from(self.block),
            nBlockAlign: self.block,
            wBitsPerSample: self.bits,
            cbSize: 0,
        }
    }

    /// Frames of this format, averaged to mono in [-1, 1].
    fn mono(&self, bytes: &[u8]) -> Vec<f32> {
        let width = usize::from(self.bits / 8);
        let sample = |b: &[u8]| -> f32 {
            match (self.tag, width) {
                (WAVE_FORMAT_IEEE_FLOAT, _) => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                (_, 2) => f32::from(i16::from_le_bytes([b[0], b[1]])) / 32_768.0,
                (_, 3) => (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0,
                _ => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
            }
        };
        let channels = usize::from(self.channels);
        bytes
            .chunks_exact(usize::from(self.block))
            .map(|frame| (0..channels).map(|c| sample(&frame[c * width..])).sum::<f32>() / channels as f32)
            .collect()
    }
}

/// Linear resampling to `STREAM_RATE`, carried across packets.
struct Resample {
    /// Input samples per output sample.
    step: f64,
    /// Where the next output falls, counted from `last`.
    at: f64,
    last: f32,
}

impl Resample {
    fn new(rate: u32) -> Resample {
        Resample { step: f64::from(rate) / f64::from(STREAM_RATE), at: 0.0, last: 0.0 }
    }

    fn push(&mut self, input: &[f32]) -> Vec<f32> {
        if self.step == 1.0 {
            return input.to_vec();
        }
        let mut out = Vec::with_capacity((input.len() as f64 / self.step) as usize + 1);
        // Position 0 is `last`, position i + 1 is `input[i]`.
        let last = self.last;
        let at = |i: usize| if i == 0 { last } else { input[i - 1] };
        while self.at < input.len() as f64 {
            let i = self.at.floor() as usize;
            let frac = (self.at - i as f64) as f32;
            out.push(at(i) + (at(i + 1) - at(i)) * frac);
            self.at += self.step;
        }
        self.at -= input.len() as f64;
        if let Some(&l) = input.last() {
            self.last = l;
        }
        out
    }
}
