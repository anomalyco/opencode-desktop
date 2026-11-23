//! Live GPU canvases handed to the window on the GPU (Rune's Metal backend; `widgets-gpu.md`): a
//! run replays on a stock wgpu Metal device on Rune's `MTLDevice`, whose queue Rune's doesn't
//! order. As Dawn's shared textures do (a fence: a shared event and a value), each canvas has a
//! ring of slot textures and two shared events:
//! - right after the run submitted a frame, its own queue copies it into a free slot and signals
//!   `ready` with the frame's number ([`MetalShare::copy`]): that queue's order puts the copy after
//!   the frame and the run's next frame after the copy, so the run never waits;
//! - the window shows the newest slot whose `ready` value was signalled, waits for it on Rune's
//!   queue before sampling it (`rune_gpu::metal::wait_for_event`, already satisfied), and signals
//!   `read` once its frame finished (`signal_event_on_finish`);
//! - a slot is copied into again only once `read` says the window finished with it, and the copy
//!   waits for that on the GPU too.
//!
//! Rune's queue only ever waits for a value already signalled: a run whose GPU hangs or is lost
//! never holds the window up (a wait Metal gives up on would lose Rune's device). A shared event's
//! listener wakes the window when a frame is ready; a frame with no free slot is copied when one
//! frees, which its `read` listener tells.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{MTLDevice, MTLEvent, MTLSharedEvent};
use opencode_widget::gpu::{Canvas, Canvases, Handoff};

use super::live::{MetalShare, RawEvent};

/// Slots a canvas's ring has: the newest frame, the one the window may still be reading, and one
/// to copy into (measured: `gpu_handoff_check`).
pub const SLOTS: usize = 3;

/// The rings, by run and canvas id.
pub struct Rings {
    share: MetalShare,
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    slots: usize,
    rings: Mutex<HashMap<(usize, String), Ring>>,
    /// Copies committed, and frames that found no free slot (`gpu_handoff_check`).
    copies: AtomicU64,
    behind: AtomicU64,
}

struct Ring {
    run: Weak<Canvases>,
    /// The run's canvas texture the slots copy, and the queue it's drawn on.
    source: wgpu::Texture,
    queue: wgpu::Queue,
    slots: Vec<Slot>,
    ready: RawEvent,
    read: RawEvent,
    /// The `ready` value of the last copy committed, and the last `read` value the window took.
    copied: u64,
    reads: u64,
    /// The run's newest frame wasn't copied yet: no slot was free.
    behind: bool,
    /// Moves when the slots change: the window wraps them again.
    generation: u64,
}

struct Slot {
    /// On the run's device, as the source is.
    texture: wgpu::Texture,
    /// The `ready` value its copy signals (0: none yet), and the `read` value of the last window
    /// frame that sampled it.
    frame: u64,
    read: u64,
}

/// A slot the window shows: its texture (on the run's device), which ring generation and slot.
pub struct Shown {
    pub texture: wgpu::Texture,
    pub generation: u64,
    pub slot: usize,
}

impl Rings {
    /// Rings of `slots` slots, their events made on `device` (the runs' and the window's).
    pub fn new(share: MetalShare, device: &ProtocolObject<dyn MTLDevice>, slots: usize) -> Rings {
        Rings { share, device: objc2::Message::retain(device), slots, rings: Mutex::default(), copies: AtomicU64::new(0), behind: AtomicU64::new(0) }
    }

    /// Copies committed so far, and frames that found no free slot.
    pub fn stats(&self) -> (u64, u64) {
        (self.copies.load(Ordering::Relaxed), self.behind.load(Ordering::Relaxed))
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<(usize, String), Ring>> {
        self.rings.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The ring of canvas `id` of `run` for `c` (made again when the run's texture changed).
    fn ring<'a>(&self, rings: &'a mut HashMap<(usize, String), Ring>, run: &Arc<Canvases>, queue: &wgpu::Queue, id: &str, c: &Canvas) -> Option<&'a mut Ring> {
        let key = (Arc::as_ptr(run) as usize, id.to_owned());
        if rings.get(&key).is_none_or(|r| r.source != c.texture) {
            let generation = rings.get(&key).map_or(0, |r| r.generation + 1);
            let device = run.device()?;
            let desc = wgpu::TextureDescriptor {
                label: Some("widget gpu canvas slot"),
                size: c.texture.size(),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: c.texture.format(),
                usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            };
            let slots = (0..self.slots).map(|_| Slot { texture: device.create_texture(&desc), frame: 0, read: 0 }).collect();
            let ring = Ring {
                run: Arc::downgrade(run),
                source: c.texture.clone(),
                queue: queue.clone(),
                slots,
                ready: self.device.newSharedEvent()?,
                read: self.device.newSharedEvent()?,
                copied: 0,
                reads: 0,
                behind: false,
                generation,
            };
            rings.insert(key.clone(), ring);
        }
        rings.get_mut(&key)
    }

    /// Copies the run's newest frame into a free slot, if one is: not the newest copied, nor the
    /// newest the window may show, and one the window finished reading. Else it's behind, and a
    /// listener on `read` tells when the slot it waits for frees.
    fn copy(&self, ring: &mut Ring) {
        let (ready, read) = (ring.ready.signaledValue(), ring.read.signaledValue());
        let newest = |done: u64| ring.slots.iter().enumerate().filter(|(_, s)| s.frame > 0 && s.frame <= done).max_by_key(|(_, s)| s.frame).map(|(i, _)| i);
        let (last, showable) = (newest(u64::MAX), newest(ready));
        let others = || ring.slots.iter().enumerate().filter(|(i, _)| Some(*i) != last && Some(*i) != showable);
        let Some(i) = others().filter(|(_, s)| s.read <= read).min_by_key(|(_, s)| s.frame).map(|(i, _)| i) else {
            if !ring.behind {
                self.behind.fetch_add(1, Ordering::Relaxed);
            }
            ring.behind = true;
            // The slot the window will be done with first.
            if let Some(next) = others().map(|(_, s)| s.read).min() {
                let run = ring.run.clone();
                (self.share.notify)(&ring.read, next, Box::new(move || {
                    if let Some(run) = run.upgrade() {
                        run.retry();
                        run.wake();
                    }
                }));
            }
            return;
        };
        let value = ring.copied + 1;
        let slot = &mut ring.slots[i];
        let wait = (slot.read > 0).then_some((&ring.read, slot.read));
        if !(self.share.copy)(&ring.queue, &ring.source, &slot.texture, wait, (&ring.ready, value)) {
            return;
        }
        ring.copied = value;
        slot.frame = value;
        self.copies.fetch_add(1, Ordering::Relaxed);
        ring.behind = false;
        let run = ring.run.clone();
        (self.share.notify)(&ring.ready, value, Box::new(move || {
            if let Some(run) = run.upgrade() {
                run.wake();
            }
        }));
    }

    /// The slot canvas `id` of `run` shows in the frame `encoder` records: the newest whose copy
    /// finished. Rune's queue waits for it (already done) before the frame's passes, and the
    /// frame's end tells the run's queue the window is done with it.
    pub fn show(&self, run: &Arc<Canvases>, id: &str, encoder: &mut wgpu::CommandEncoder) -> Option<Shown> {
        let mut rings = self.lock();
        let ring = rings.get_mut(&(Arc::as_ptr(run) as usize, id.to_owned()))?;
        let done = ring.ready.signaledValue();
        let (i, slot) = ring.slots.iter_mut().enumerate().filter(|(_, s)| s.frame > 0 && s.frame <= done).max_by_key(|(_, s)| s.frame)?;
        ring.reads += 1;
        slot.read = ring.reads;
        let (ready, read) = (event(&ring.ready), event(&ring.read));
        if !rune_gpu::metal::wait_for_event(encoder, ready, slot.frame) || !rune_gpu::metal::signal_event_on_finish(encoder, read, slot.read) {
            return None;
        }
        Some(Shown { texture: slot.texture.clone(), generation: ring.generation, slot: i })
    }

    /// Whether canvas `id` of `run` has a frame the window doesn't show yet.
    pub(super) fn pending(&self, run: &Canvases, id: &str) -> bool {
        self.lock().get(&(run as *const Canvases as usize, id.to_owned())).is_some_and(|r| r.behind || r.copied > r.ready.signaledValue())
    }
}

fn event(e: &RawEvent) -> &ProtocolObject<dyn MTLEvent> {
    ProtocolObject::from_ref(&**e)
}

impl Handoff for Rings {
    fn publish(&self, canvases: &Arc<Canvases>, queue: &wgpu::Queue, frames: &[(String, Canvas)]) {
        let mut rings = self.lock();
        rings.retain(|_, r| r.run.strong_count() > 0);
        for (id, c) in frames {
            if let Some(ring) = self.ring(&mut rings, canvases, queue, id, c) {
                self.copy(ring);
            }
        }
    }

    fn retry(&self, canvases: &Arc<Canvases>, _queue: &wgpu::Queue) {
        let run = Arc::as_ptr(canvases) as usize;
        for ((r, _), ring) in self.lock().iter_mut() {
            if *r == run && ring.behind {
                self.copy(ring);
            }
        }
    }

    fn forget(&self, canvases: &Canvases) {
        let run = canvases as *const Canvases as usize;
        self.lock().retain(|(r, _), _| *r != run);
    }
}
