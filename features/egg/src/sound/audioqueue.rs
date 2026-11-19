//! The egg's sound on macOS: an AudioQueue output on the default device, the graph (`synth.rs`)
//! rendering into its buffers from the queue's own callback thread. The first sound makes the
//! queue; once the last one's room tail has died away it stops, is disposed of, and everything it
//! held goes.
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};

use super::synth::Engine;
use super::{RATE, Sound};

/// Three buffers of 512 frames: about 21 ms queued, as short as Chromium's output.
const FRAMES: u32 = 512;
const BUFFERS: usize = 3;

#[repr(C)]
struct StreamFormat {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

#[repr(C)]
struct QueueBuffer {
    capacity: u32,
    data: *mut c_void,
    size: u32,
    user: *mut c_void,
    descriptions_capacity: u32,
    descriptions: *mut c_void,
    descriptions_count: u32,
}

type Queue = *mut c_void;
type Callback = extern "C" fn(*mut c_void, Queue, *mut QueueBuffer);

#[link(name = "AudioToolbox", kind = "framework")]
unsafe extern "C" {
    fn AudioQueueNewOutput(format: *const StreamFormat, callback: Callback, user: *mut c_void, run_loop: *const c_void, mode: *const c_void, flags: u32, out: *mut Queue) -> i32;
    fn AudioQueueAllocateBuffer(queue: Queue, bytes: u32, out: *mut *mut QueueBuffer) -> i32;
    fn AudioQueueEnqueueBuffer(queue: Queue, buffer: *mut QueueBuffer, descriptions: u32, packets: *const c_void) -> i32;
    fn AudioQueueStart(queue: Queue, at: *const c_void) -> i32;
    fn AudioQueueStop(queue: Queue, immediate: u8) -> i32;
    fn AudioQueueDispose(queue: Queue, immediate: u8) -> i32;
}

static STREAM: Mutex<Option<Sender<Sound>>> = Mutex::new(None);

/// What the callback renders from, shared with the thread that owns the queue.
struct Render {
    engine: Engine,
    rx: Receiver<Sound>,
    quantum: ([f32; 128], [f32; 128]),
    used: usize,
    /// The tail has died away: the owning thread stops the queue.
    done: bool,
}

type Shared = (Mutex<Render>, Condvar);

pub fn play(sound: Sound) {
    let mut stream = STREAM.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(tx) = stream.as_ref()
        && tx.send(sound).is_ok()
    {
        return;
    }
    let (tx, rx) = channel();
    // The room and the noise from the numbers this first sound drew for them.
    let mut engine = Engine::new(RATE, &mut { sound.tables });
    engine.play(sound.cue, &mut { sound.random });
    let render = Render { engine, rx, quantum: ([0.0; 128], [0.0; 128]), used: 128, done: false };
    let started = std::thread::Builder::new().name("egg-sound".into()).spawn(move || run(render));
    *stream = started.ok().map(|_| tx);
}

fn run(render: Render) {
    let shared: Arc<Shared> = Arc::new((Mutex::new(render), Condvar::new()));
    let format = StreamFormat {
        sample_rate: RATE,
        format_id: u32::from_be_bytes(*b"lpcm"),
        // Float, packed: interleaved stereo f32.
        format_flags: 1 | 8,
        bytes_per_packet: 8,
        frames_per_packet: 1,
        bytes_per_frame: 8,
        channels_per_frame: 2,
        bits_per_channel: 32,
        reserved: 0,
    };
    let user = Arc::into_raw(shared.clone()).cast_mut().cast::<c_void>();
    let mut queue: Queue = std::ptr::null_mut();
    // SAFETY: the format outlives the call; `user` is an `Arc<Shared>` the callback borrows until
    // the queue is disposed of below, then it's released.
    let made = unsafe { AudioQueueNewOutput(&format, callback, user, std::ptr::null(), std::ptr::null(), 0, &mut queue) } == 0;
    if made {
        let mut primed = 0;
        for _ in 0..BUFFERS {
            let mut buffer = std::ptr::null_mut();
            // SAFETY: a live queue; the buffer is the queue's and is filled before it's enqueued.
            if unsafe { AudioQueueAllocateBuffer(queue, FRAMES * 8, &mut buffer) } == 0 {
                callback(user, queue, buffer);
                primed += 1;
            }
        }
        // SAFETY: a live queue with buffers enqueued.
        if primed > 0 && unsafe { AudioQueueStart(queue, std::ptr::null()) } == 0 {
            let (lock, wake) = &*shared;
            let mut r = lock.lock().unwrap_or_else(|e| e.into_inner());
            while !r.done {
                r = wake.wait(r).unwrap_or_else(|e| e.into_inner());
            }
        }
        // SAFETY: stopped and disposed of at once, on this thread: no callback runs after.
        unsafe {
            AudioQueueStop(queue, 1);
            AudioQueueDispose(queue, 1);
        }
    }
    // SAFETY: the reference `into_raw` made above; the queue that borrowed it is gone.
    unsafe { drop(Arc::from_raw(user.cast::<Shared>())) };
    // The next sound starts a new queue.
    *STREAM.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The queue wants `buffer` filled: render the graph into it, unless the tail is over.
extern "C" fn callback(user: *mut c_void, queue: Queue, buffer: *mut QueueBuffer) {
    // SAFETY: `user` is the `Arc<Shared>` `run` holds for the queue's life.
    let (lock, wake) = unsafe { &*user.cast::<Shared>() };
    let mut r = lock.lock().unwrap_or_else(|e| e.into_inner());
    if r.done {
        return;
    }
    while let Ok(sound) = r.rx.try_recv() {
        r.engine.play(sound.cue, &mut { sound.random });
    }
    if r.engine.silent() {
        r.done = true;
        wake.notify_one();
        return;
    }
    // SAFETY: the queue's buffer has capacity `FRAMES * 8` bytes (`FRAMES * 2` `f32`s) and is ours
    // until enqueued; `MaybeUninit<f32>` avoids assuming `AudioQueueAllocateBuffer` initialized it.
    let out = unsafe {
        std::slice::from_raw_parts_mut(
            (*buffer).data.cast::<std::mem::MaybeUninit<f32>>(),
            FRAMES as usize * 2,
        )
    };
    for frame in out.chunks_exact_mut(2) {
        if r.used == 128 {
            let r = &mut *r;
            r.engine.quantum(&mut r.quantum.0, &mut r.quantum.1);
            r.used = 0;
        }
        frame[0].write(r.quantum.0[r.used]);
        frame[1].write(r.quantum.1[r.used]);
        r.used += 1;
    }
    // SAFETY: as above; the buffer goes back to its own queue, full.
    unsafe {
        (*buffer).size = FRAMES * 8;
        AudioQueueEnqueueBuffer(queue, buffer, 0, std::ptr::null());
    }
}
