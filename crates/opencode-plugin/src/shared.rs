//! What plugins share with the host and with each other in-process: stores the host keeps,
//! typed contracts and services, background tasks, wakers and icons.
//!
//! - [`Store<T>`]: a value the host persists for the plugin (globally or per session). It lives
//!   in the host, so it survives a hot reload and costs the plugin nothing to keep.
//! - [`Contract<T>`] / [`Service<T>`]: a typed value one plugin publishes and others use, by
//!   id. `T` must be `#[repr(C)]` (function pointers and plain data), since provider and
//!   consumer are separate libraries; the handshake checks its id, version, size and alignment.
//! - [`Live<T>`]: a consumer's view of a contract: pending, active (with its generation) or
//!   inactive. Every consumer must cope with all three, since providers reload and fail.
//! - [`LatestTask<T>`]: work on a host worker thread where only the newest request's answer
//!   lands, waking this plugin's `poll`.

use core::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::abi::{AbiStatus, AbiStr, IconSpecAbi, LIVE_ACTIVE, LIVE_DISABLED, LIVE_FAILED, LIVE_PENDING, LiveAbi};
use crate::app::AppSetupCx;
use crate::boundary::{ctx, host};
use crate::cx::{ActionCx, PluginWaker, ViewCx, WindowSetupCx, job_callback};

/// A value a [`Store`] holds, as text.
pub trait StoreValue: Sized {
    fn encode(&self) -> String;
    fn decode(text: &str) -> Option<Self>;
}

impl StoreValue for String {
    fn encode(&self) -> String {
        self.clone()
    }
    fn decode(text: &str) -> Option<Self> {
        Some(text.to_owned())
    }
}

macro_rules! store_value_parse {
    ($($t:ty),*) => {$(
        impl StoreValue for $t {
            fn encode(&self) -> String {
                self.to_string()
            }
            fn decode(text: &str) -> Option<Self> {
                text.parse().ok()
            }
        }
    )*};
}
store_value_parse!(bool, i32, i64, u32, u64, f32, f64);

/// A value the host keeps for this plugin under `name`, saved with the app's other state. Read it
/// in any callback; write it in setup, a handler or poll.
pub struct Store<T> {
    name: &'static str,
    _t: PhantomData<fn() -> T>,
}

impl<T: StoreValue> Store<T> {
    pub const fn new(name: &'static str) -> Self {
        Store { name, _t: PhantomData }
    }

    /// The global value.
    pub fn get(&self) -> Option<T> {
        self.get_in("")
    }

    /// The value for session `session`.
    pub fn get_in(&self, session: &str) -> Option<T> {
        let mut out = AbiStr::EMPTY;
        // SAFETY: the host writes a view valid until its next call; it's decoded before that.
        unsafe {
            if (host().store_get)(ctx(), AbiStr::new(self.name), AbiStr::new(session), &mut out) == 0 {
                return None;
            }
            T::decode(out.as_str())
        }
    }

    pub fn set(&self, value: &T) {
        self.set_in("", value);
    }

    pub fn set_in(&self, session: &str, value: &T) {
        let text = value.encode();
        // SAFETY: the strings are alive for the call.
        unsafe { (host().store_set)(ctx(), AbiStr::new(self.name), AbiStr::new(session), AbiStr::new(&text)) };
    }
}

/// The id and type both sides of a contract name. Only plugin `x` may provide `x` or `x.<name>`.
pub struct Contract<T: 'static> {
    id: &'static str,
    version: u32,
    _t: PhantomData<fn() -> T>,
}

/// A contract an [`AppPlugin`](crate::AppPlugin) provides for the whole process, rather than one
/// window half provides for its window.
pub type Service<T> = Contract<T>;

impl<T: 'static> Contract<T> {
    pub const fn new(id: &'static str, version: u32) -> Self {
        Contract { id, version, _t: PhantomData }
    }

    pub fn id(&self) -> &'static str {
        self.id
    }

    /// FNV-1a over the id, the version, and `T`'s size and alignment.
    pub fn type_hash(&self) -> u64 {
        let mut h: u64 = 0xCBF2_9CE4_8422_2325;
        let mut eat = |b: u8| h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01B3);
        self.id.bytes().for_each(&mut eat);
        let words = [u64::from(self.version), size_of::<T>() as u64, align_of::<T>() as u64];
        words.iter().flat_map(|w| w.to_le_bytes()).for_each(eat);
        h
    }
}

/// Why a contract has no value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inactive {
    /// Its provider is off, absent, or doesn't run on this OS.
    Disabled,
    /// Its provider failed to load, set up, or keep running.
    Failed,
    /// Its provider publishes a different type under this id.
    Mismatch,
}

/// A contract's state, read in the callback that asked.
pub enum LiveState<'a, T> {
    /// Its provider hasn't published it yet (still loading, or reloading).
    Pending,
    /// Published; `generation` grows each time the provider publishes it again (a reload).
    Active { value: &'a T, generation: u32 },
    Inactive(Inactive),
}

/// A consumer's handle on a contract (`cx.consume`). Main thread only: the value it gives is
/// valid until the callback that read it returns, since a reload replaces it between callbacks.
pub struct Live<T: 'static> {
    handle: u32,
    _t: PhantomData<fn() -> T>,
}

impl<T: 'static> Live<T> {
    fn raw(&self) -> LiveAbi {
        let mut out = LiveAbi::default();
        // SAFETY: a valid out pointer; any callback on the app's thread may ask.
        unsafe { (host().live)(ctx(), self.handle, &mut out) };
        out
    }

    pub fn get(&self) -> LiveState<'_, T> {
        let raw = self.raw();
        match raw.state {
            LIVE_PENDING => LiveState::Pending,
            // SAFETY: an active contract's value is the provider's `T` (the type hash matched),
            // alive until this callback returns.
            LIVE_ACTIVE if raw.value != 0 => LiveState::Active { value: unsafe { &*(raw.value as *const T) }, generation: raw.generation },
            LIVE_ACTIVE => LiveState::Pending,
            LIVE_DISABLED => LiveState::Inactive(Inactive::Disabled),
            LIVE_FAILED => LiveState::Inactive(Inactive::Failed),
            _ => LiveState::Inactive(Inactive::Mismatch),
        }
    }

    /// Runs `f` on the value while the contract is active.
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> Option<R> {
        match self.get() {
            LiveState::Active { value, .. } => Some(f(value)),
            _ => None,
        }
    }

    /// Changes whenever the state or generation does: put it in a memo's version.
    pub fn version(&self) -> u64 {
        let raw = self.raw();
        (u64::from(raw.state) << 32) | u64::from(raw.generation)
    }
}

/// Background work where only the newest request counts: an older one still running when a
/// newer arrives is dropped, and one still waiting out its delay never runs (a debounce).
pub struct LatestTask<T: Send + 'static> {
    inner: Arc<TaskInner<T>>,
}

struct TaskInner<T> {
    seq: AtomicU64,
    done: AtomicU64,
    result: Mutex<Option<T>>,
    waker: PluginWaker,
    delay_ms: u64,
}

impl<T: Send + 'static> LatestTask<T> {
    /// Runs `job` on a host worker thread after the task's delay; its answer lands in
    /// [`Self::take`] and wakes this plugin's poll, unless a newer request came first. Call it
    /// in setup, a handler or poll.
    pub fn request(&self, job: impl FnOnce() -> T + Send + 'static) {
        let seq = self.inner.seq.fetch_add(1, Ordering::AcqRel) + 1;
        let inner = self.inner.clone();
        spawn_after(self.inner.delay_ms, move || {
            if inner.seq.load(Ordering::Acquire) != seq {
                return;
            }
            let value = job();
            if inner.seq.load(Ordering::Acquire) == seq {
                if let Ok(mut slot) = inner.result.lock() {
                    *slot = Some(value);
                }
                inner.done.store(seq, Ordering::Release);
                inner.waker.wake();
            }
        });
    }

    /// The newest request's answer, once.
    pub fn take(&self) -> Option<T> {
        self.inner.result.lock().ok()?.take()
    }

    /// A request is waiting or running.
    pub fn busy(&self) -> bool {
        self.inner.done.load(Ordering::Acquire) != self.inner.seq.load(Ordering::Acquire)
    }
}

/// Runs `job` on a host worker thread after `delay_ms`; this library stays loaded until it
/// returns. Call it in setup, a handler or poll.
pub fn spawn_after(delay_ms: u64, job: impl FnOnce() + Send + 'static) {
    let cb = job_callback(job);
    // SAFETY: the host takes the job; on refusal it's ours to free.
    if unsafe { (host().spawn)(ctx(), cb, delay_ms) } != AbiStatus::Ok {
        crate::boundary::drop_packed(cb.data, cb.drop_data);
    }
}

/// An icon id for `ElSpec::icon_id`, from [`intern_icon`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct IconId(pub u32);

/// Copies an SVG icon (its inner markup, a square `view` box) into the host's pool, so nothing
/// drawn points into this library. Use the name in specs (`PanelSpec::icon`), or the id with
/// [`crate::icon_id`].
pub fn intern_icon(name: &str, body: &str, view: f32) -> IconId {
    let spec = IconSpecAbi { name: AbiStr::new(name), body: AbiStr::new(body), view };
    // SAFETY: `spec` and its strings are alive for the call.
    IconId(unsafe { (host().intern_icon)(ctx(), &spec) })
}

fn provide<T: Send + Sync + 'static>(contract: &Contract<T>, value: T) -> bool {
    unsafe extern "C" fn drop_value<T>(data: usize) {
        crate::boundary::guard(|| {
            // SAFETY: `data` is the box `provide` leaked, freed once by the host.
            drop(unsafe { Box::from_raw(data as *mut T) });
        })
        .unwrap_or(());
    }
    let value = Box::into_raw(Box::new(value)) as usize;
    // SAFETY: the host owns `value` from here and frees it with `drop_value::<T>`; on refusal
    // it frees nothing, so we do.
    let status = unsafe { (host().provide)(ctx(), AbiStr::new(contract.id), contract.type_hash(), value, Some(drop_value::<T>)) };
    if status != AbiStatus::Ok {
        // SAFETY: the host refused it, so the box is still ours.
        unsafe { drop_value::<T>(value) };
    }
    status == AbiStatus::Ok
}

fn consume<T: 'static>(contract: &Contract<T>) -> Live<T> {
    // SAFETY: setup contexts accept it.
    let handle = unsafe { (host().consume)(ctx(), AbiStr::new(contract.id), contract.type_hash()) };
    Live { handle, _t: PhantomData }
}

fn waker() -> PluginWaker {
    // SAFETY: called inside setup, whose context the host accepts.
    PluginWaker(unsafe { (host().waker)(ctx()) })
}

macro_rules! setup_common {
    ($ty:ident) => {
        impl<S: 'static> $ty<S> {
            /// Wakes this plugin's `poll` from any thread.
            pub fn waker(&mut self) -> PluginWaker {
                waker()
            }

            /// Uses contract `contract`, whatever its provider's state; read it with `Live::get`.
            pub fn consume<T: 'static>(&mut self, contract: &Contract<T>) -> Live<T> {
                consume(contract)
            }

            /// A [`LatestTask`] whose jobs wait `delay_ms` before running.
            pub fn latest_task<T: Send + 'static>(&mut self, delay_ms: u64) -> LatestTask<T> {
                let inner = TaskInner { seq: AtomicU64::new(0), done: AtomicU64::new(0), result: Mutex::new(None), waker: waker(), delay_ms };
                LatestTask { inner: Arc::new(inner) }
            }

            /// See [`intern_icon`].
            pub fn icon(&mut self, name: &str, body: &str, view: f32) -> IconId {
                intern_icon(name, body, view)
            }
        }
    };
}
setup_common!(WindowSetupCx);
setup_common!(AppSetupCx);

impl<S: 'static> AppSetupCx<S> {
    /// Publishes `value` as service `service` for every window. False when refused (the id
    /// isn't this plugin's).
    pub fn provide_service<T: Send + Sync + 'static>(&mut self, service: &Service<T>, value: T) -> bool {
        provide(service, value)
    }
}

impl<S: 'static> WindowSetupCx<S> {
    /// Publishes `value` as contract `contract` for this window. False when refused.
    pub fn provide_contract<T: Send + Sync + 'static>(&mut self, contract: &Contract<T>, value: T) -> bool {
        provide(contract, value)
    }
}

impl ActionCx {
    /// Runs `job` on a host worker thread; see [`spawn_after`].
    pub fn spawn(&mut self, job: impl FnOnce() + Send + 'static) {
        spawn_after(0, job);
    }
}

impl<P: 'static> ViewCx<P> {
    /// The id of an icon this plugin interned by `name` (0, no icon, when it hasn't).
    pub fn icon_named(&self, name: &str) -> IconId {
        intern_icon(name, "", 0.0)
    }
}
