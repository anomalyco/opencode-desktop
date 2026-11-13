//! The egg's sound on Windows: a WASAPI shared-mode stream on the default output, event-driven,
//! with the graph (`synth.rs`) rendering into it on the stream's own thread. The first sound
//! starts the thread; once the last one's room tail has died away it stops the stream and ends,
//! and everything it held goes.
#![allow(unsafe_code)]

use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use super::synth::Engine;
use super::{RATE, Sound};

/// The device's buffer: short, as Chromium's output is, so a sound lands with its hit.
const BUFFER: Duration = Duration::from_millis(20);

static STREAM: Mutex<Option<Sender<Sound>>> = Mutex::new(None);

pub fn play(sound: Sound) {
    let mut stream = STREAM.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(tx) = stream.as_ref()
        && tx.send(sound).is_ok()
    {
        return;
    }
    let (tx, rx) = channel();
    let _ = tx.send(sound);
    let started = std::thread::Builder::new().name("egg-sound".into()).spawn(move || run(rx));
    *stream = started.ok().map(|_| tx);
}

fn run(rx: Receiver<Sound>) {
    // SAFETY: COM for this thread, matched below.
    let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
    let _ = stream(&rx);
    // The next sound starts a new stream.
    *STREAM.lock().unwrap_or_else(|e| e.into_inner()) = None;
    if com {
        // SAFETY: matches the initialisation above.
        unsafe { CoUninitialize() };
    }
}

fn stream(rx: &Receiver<Sound>) -> windows::core::Result<()> {
    // SAFETY: COM calls on this thread's objects; the format is interleaved stereo float.
    unsafe {
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = devices.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        // The graph's own rate; the shared engine converts to the device's.
        let rate = RATE as u32;
        let format = WAVEFORMATEX { wFormatTag: 3, nChannels: 2, nSamplesPerSec: rate, nAvgBytesPerSec: rate * 8, nBlockAlign: 8, wBitsPerSample: 32, cbSize: 0 };
        let flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, BUFFER.as_nanos() as i64 / 100, 0, &format, None)?;
        let event = CreateEventW(None, false, false, None)?;
        client.SetEventHandle(event)?;
        let render: IAudioRenderClient = client.GetService()?;
        let frames = client.GetBufferSize()?;
        // The room and the noise from the numbers the first sound drew for them.
        let Ok(first) = rx.try_recv() else { return Ok(()) };
        let mut engine = Engine::new(RATE, &mut { first.tables });
        // The rendered quantum not yet written: 128 stereo frames, and how far into it.
        let mut quantum = ([0.0f32; 128], [0.0f32; 128]);
        let mut used = 128;
        let mut started = false;
        engine.play(first.cue, &mut { first.random });
        loop {
            loop {
                match rx.try_recv() {
                    Ok(sound) => engine.play(sound.cue, &mut { sound.random }),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => break,
                }
            }
            if engine.silent() {
                break;
            }
            let padding = client.GetCurrentPadding().unwrap_or(frames);
            let free = frames.saturating_sub(padding);
            if free > 0
                && let Ok(data) = render.GetBuffer(free)
            {
                let out = std::slice::from_raw_parts_mut(data.cast::<f32>(), free as usize * 2);
                for frame in out.chunks_exact_mut(2) {
                    if used == 128 {
                        engine.quantum(&mut quantum.0, &mut quantum.1);
                        used = 0;
                    }
                    frame[0] = quantum.0[used];
                    frame[1] = quantum.1[used];
                    used += 1;
                }
                let _ = render.ReleaseBuffer(free, 0);
                if !started {
                    started = client.Start().is_ok();
                }
            }
            if WaitForSingleObject(event, 50) != WAIT_OBJECT_0 && !started {
                break;
            }
        }
        let _ = client.Stop();
        let _ = CloseHandle(event);
    }
    Ok(())
}
