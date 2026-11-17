//! Fixed 64-byte session exit record (`<data>/last-exit.bin`), fatal exit hooks, and
//! crashing-session restore-loop escape (`<data>/skip-restore.bin`).
//!
//! Three honest fatal reasons are recorded when this process observes them itself:
//! - `Reason::OutOfMemory`: `OomGuard` saw a null heap allocation, or Windows dispatched an
//!   unhandled memory / commit-short stack-growth exception (`SetUnhandledExceptionFilter` /
//!   `SIGABRT` while `commit_short()` is true).
//! - `Reason::StackOverflow`: Windows dispatched an unhandled `STATUS_STACK_OVERFLOW`
//!   (`0xC00000FD`) with commit memory to spare (`commit_short()` is false), such as deep layout
//!   recursion on a pathological Markdown reply.
//! - `Reason::Fatal`: an uncaught Rust panic (`std::panic::set_hook`) or any other unhandled
//!   terminal OS exception (`STATUS_ACCESS_VIOLATION`, `SIGABRT` with spare commit, etc.).
//!
//! `init_hooks` arms the in-memory hooks and 16 KiB main-thread stack guarantee before Frame 1
//! with zero disk I/O; `presented` opens `<data>/last-exit.bin` off the UI thread after Frame 1
//! (a pre-Frame-1 crash opens it on demand on the failing thread's stack). On the second
//! consecutive close of the same reason while restoring the same session, `record_fault` writes a
//! 16-byte `<data>/skip-restore.bin` sentinel so the next launch opens Home instead of looping.
//!
//! Explicit lifecycle machine matching Official's
//! `packages/desktop/src/main/windows/recovery-machine.ts` (`RecoveryIncident` `terminated`).
#![cfg_attr(not(target_arch = "wasm32"), allow(unsafe_code))]
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

#[cfg(not(target_arch = "wasm32"))]
mod os;

const MAGIC: u32 = 0x4558_5431; // "EXT1"
pub(super) const SKIP_MAGIC: u32 = 0x534B_4950; // "SKIP"
const VERSION: u16 = 1;
pub const RECORD_BYTES: usize = 64;
pub(super) const SKIP_BYTES: usize = 16;

pub(super) const TAG_CLEAN: u8 = 0;
pub(super) const TAG_RUNNING: u8 = 1;
pub(super) const TAG_OUT_OF_MEMORY: u8 = 2;
pub(super) const TAG_STACK_OVERFLOW: u8 = 3;
pub(super) const TAG_FATAL: u8 = 4;

pub const FLAG_HEAP_ALLOC: u8 = 1;
pub const FLAG_OS_EXCEPTION: u8 = 2;
pub const FLAG_PANIC: u8 = 3;

/// Why the previous process ended, recorded only when that process observed the cause itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Reason {
    OutOfMemory,
    StackOverflow,
    Fatal,
}

/// What `<data>/last-exit.bin` held from the previous session when this one opened it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Previous {
    /// First launch, unreadable record, or the previous session quit cleanly (`tag = 0`).
    Clean,
    /// Previous session left `tag = 1` (`Running`): external kill or power loss; never guessed.
    Interrupted { pid: u32, seq: u64 },
    /// Previous session wrote a verified fatal reason before exiting.
    Ended {
        reason: Reason,
        flags: u8,
        code: u32,
        pid: u32,
        requested_bytes: u64,
        avail_commit_bytes: u64,
        seq: u64,
        session_hash: u64,
        consecutive: u32,
        opened_home: bool,
    },
}

/// Session lifecycle of the exit record (`packages/desktop/src/main/windows/recovery-machine.ts`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Before Frame 1 presents: no descriptor kept open, zero pre-frame I/O.
    Unopened,
    /// Descriptor open and initialized to `Running` (`seq`) for this session.
    Active { seq: u64, previous: Previous, taken: bool },
    /// Orderly quit wrote `Clean` ahead of `storage::close()`.
    Closed { seq: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Event {
    Opened { seq: u64, previous: Previous },
    TakePrevious,
    CleanQuit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    WriteRunning { seq: u64 },
    WriteClean { seq: u64 },
}

pub fn step(state: &mut State, event: Event) -> Option<Effect> {
    match (*state, event) {
        (State::Unopened, Event::Opened { seq, previous }) => {
            *state = State::Active { seq, previous, taken: false };
            Some(Effect::WriteRunning { seq })
        }
        (State::Unopened, Event::CleanQuit) => {
            *state = State::Closed { seq: 0 };
            None
        }
        (State::Active { seq, previous, .. }, Event::TakePrevious) => {
            *state = State::Active { seq, previous, taken: true };
            None
        }
        (State::Active { seq, .. }, Event::CleanQuit) => {
            *state = State::Closed { seq };
            Some(Effect::WriteClean { seq })
        }
        _ => None,
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

pub fn hash_session(id: &str) -> u64 {
    if id.is_empty() {
        return 0;
    }
    let mut h = 0xCBF2_9CE4_8422_2325u64;
    for &b in id.as_bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01B3);
    }
    h | 1
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn encode_record(
    tag: u8,
    flags: u8,
    code: u32,
    pid: u32,
    requested_bytes: u64,
    avail_commit_bytes: u64,
    seq: u64,
    session_hash: u64,
    consecutive: u32,
) -> [u8; RECORD_BYTES] {
    let mut buf = [0u8; RECORD_BYTES];
    buf[0..4].copy_from_slice(&MAGIC.to_le_bytes());
    buf[4..6].copy_from_slice(&VERSION.to_le_bytes());
    buf[6] = tag;
    buf[7] = flags;
    buf[8..12].copy_from_slice(&code.to_le_bytes());
    buf[12..16].copy_from_slice(&pid.to_le_bytes());
    buf[16..24].copy_from_slice(&requested_bytes.to_le_bytes());
    buf[24..32].copy_from_slice(&avail_commit_bytes.to_le_bytes());
    buf[32..40].copy_from_slice(&seq.to_le_bytes());
    buf[40..48].copy_from_slice(&session_hash.to_le_bytes());
    buf[48..52].copy_from_slice(&consecutive.to_le_bytes());
    let sum = crc32(&buf[0..60]);
    buf[60..64].copy_from_slice(&sum.to_le_bytes());
    buf
}

pub(super) fn decode_raw_fields(
    buf: &[u8; RECORD_BYTES],
) -> Option<(u8, u8, u32, u32, u64, u64, u64, u64, u32)> {
    if u32::from_le_bytes(buf[0..4].try_into().ok()?) != MAGIC {
        return None;
    }
    if u16::from_le_bytes(buf[4..6].try_into().ok()?) != VERSION {
        return None;
    }
    if u32::from_le_bytes(buf[60..64].try_into().ok()?) != crc32(&buf[0..60]) {
        return None;
    }
    Some((
        buf[6],
        buf[7],
        u32::from_le_bytes(buf[8..12].try_into().ok()?),
        u32::from_le_bytes(buf[12..16].try_into().ok()?),
        u64::from_le_bytes(buf[16..24].try_into().ok()?),
        u64::from_le_bytes(buf[24..32].try_into().ok()?),
        u64::from_le_bytes(buf[32..40].try_into().ok()?),
        u64::from_le_bytes(buf[40..48].try_into().ok()?),
        u32::from_le_bytes(buf[48..52].try_into().ok()?),
    ))
}

fn decode_record(buf: &[u8; RECORD_BYTES], opened_home: bool) -> Option<(Previous, u64)> {
    let (tag, flags, code, pid, requested_bytes, avail_commit_bytes, seq, session_hash, consecutive) =
        decode_raw_fields(buf)?;
    let reason = match tag {
        TAG_CLEAN => return Some((Previous::Clean, seq)),
        TAG_RUNNING => return Some((Previous::Interrupted { pid, seq }, seq)),
        TAG_OUT_OF_MEMORY => Reason::OutOfMemory,
        TAG_STACK_OVERFLOW => Reason::StackOverflow,
        TAG_FATAL => Reason::Fatal,
        _ => return None,
    };
    Some((
        Previous::Ended {
            reason,
            flags,
            code,
            pid,
            requested_bytes,
            avail_commit_bytes,
            seq,
            session_hash,
            consecutive: consecutive.max(1),
            opened_home,
        },
        seq,
    ))
}

/// Readiness of `<data>/last-exit.bin` when a window polls for the previous session's outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PollPrevious {
    /// Post-Frame-1 open on the task pool is still in flight.
    Opening,
    /// First caller in this process: returns what `<data>/last-exit.bin` recorded.
    Ready(Previous),
    /// Already taken by a window in this process, or closed/unavailable.
    Taken,
}

#[cfg(target_arch = "wasm32")]
pub fn init_hooks() {}
#[cfg(target_arch = "wasm32")]
pub fn presented() {}
#[cfg(target_arch = "wasm32")]
pub fn mark_clean() {}
#[cfg(target_arch = "wasm32")]
pub fn note_restoring_session(_id: &str) {}
#[cfg(target_arch = "wasm32")]
pub fn take_skip_restore(_id: &str) -> bool { false }
#[cfg(target_arch = "wasm32")]
pub fn mark_restore_settled(_id: &str) {}
#[cfg(target_arch = "wasm32")]
pub fn set_active_session(_id: Option<&str>) {}
#[cfg(target_arch = "wasm32")]
pub fn poll_previous() -> PollPrevious { PollPrevious::Taken }
#[cfg(target_arch = "wasm32")]
pub fn poll_debug_drill() {}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use core::alloc::{GlobalAlloc, Layout};
    use std::io::Read as _;
    use std::path::Path;
    #[cfg(debug_assertions)]
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, AtomicU8, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::thread::ThreadId;

    use super::os::{
        EncodedPaths, close_raw, commit_short_and_avail, install_os_hooks, into_raw_fd, open_file,
        open_raw_emergency, raw_pid, read_raw, reserve_main_stack_guarantee, write_raw,
        write_skip_sentinel,
    };
    use super::{
        Effect, Event, FLAG_HEAP_ALLOC, FLAG_PANIC, PollPrevious, Previous, RECORD_BYTES,
        SKIP_BYTES, SKIP_MAGIC, State, TAG_CLEAN, TAG_FATAL, TAG_OUT_OF_MEMORY, TAG_RUNNING,
        decode_raw_fields, decode_record, encode_record, hash_session, step,
    };

    pub(super) static PATHS: OnceLock<EncodedPaths> = OnceLock::new();
    static MAIN_TID: OnceLock<ThreadId> = OnceLock::new();
    static STATE: Mutex<State> = Mutex::new(State::Unopened);
    static RECORD_FD: AtomicIsize = AtomicIsize::new(-1);
    static SEQ: AtomicU64 = AtomicU64::new(0);
    pub(super) static TERMINAL_RECORDED: AtomicBool = AtomicBool::new(false);
    pub(super) static TENTATIVE_PANIC: AtomicBool = AtomicBool::new(false);

    /// Session restoration tracking for breaking consecutive restore crash loops.
    static RESTORING_HASH: AtomicU64 = AtomicU64::new(0);
    static ACTIVE_HASH: AtomicU64 = AtomicU64::new(0);
    static RESTORE_SETTLED: AtomicBool = AtomicBool::new(true);
    static SKIPPED_HASH: AtomicU64 = AtomicU64::new(0);
    static PREV_LOADED: AtomicBool = AtomicBool::new(false);
    static PREV_TAG: AtomicU8 = AtomicU8::new(0);
    static PREV_HASH: AtomicU64 = AtomicU64::new(0);
    static PREV_CONSECUTIVE: AtomicU32 = AtomicU32::new(0);

    pub struct OomGuard<A>(pub A);

    // SAFETY: delegates every operation unchanged to `A`; `on_alloc_oom` never touches the heap.
    unsafe impl<A: GlobalAlloc> GlobalAlloc for OomGuard<A> {
        #[inline(always)]
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let p = unsafe { self.0.alloc(layout) };
            if p.is_null() {
                on_alloc_oom(layout.size());
            }
            p
        }

        #[inline(always)]
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            let p = unsafe { self.0.alloc_zeroed(layout) };
            if p.is_null() {
                on_alloc_oom(layout.size());
            }
            p
        }

        #[inline(always)]
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            unsafe { self.0.dealloc(ptr, layout) };
        }

        #[inline(always)]
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            let p = unsafe { self.0.realloc(ptr, layout, new_size) };
            if p.is_null() {
                on_alloc_oom(new_size);
            }
            p
        }
    }

    #[cold]
    #[inline(never)]
    fn on_alloc_oom(requested: usize) {
        #[cfg(windows)]
        const CODE: u32 = 0xC000_0409;
        #[cfg(unix)]
        const CODE: u32 = libc::SIGABRT as u32;
        let (_, avail) = commit_short_and_avail();
        record_fault(TAG_OUT_OF_MEMORY, FLAG_HEAP_ALLOC, CODE, requested as u64, avail, true);
    }

    /// Arms in-memory crash hooks on the main thread before Frame 1, with zero file I/O.
    pub fn init_hooks() {
        static ARMED: std::sync::Once = std::sync::Once::new();
        if crate::headless() {
            return;
        }
        ARMED.call_once(|| {
            let _ = MAIN_TID.set(std::thread::current().id());
            if let Some(dir) = crate::dirs::data() {
                let _ = PATHS.set(EncodedPaths::new(dir.join("last-exit.bin"), dir.join("skip-restore.bin")));
            }
            reserve_main_stack_guarantee();
            install_panic_hook();
            install_os_hooks();
        });
    }

    pub fn note_restoring_session(id: &str) {
        let h = hash_session(id);
        RESTORING_HASH.store(h, Ordering::Release);
        ACTIVE_HASH.store(h, Ordering::Release);
        RESTORE_SETTLED.store(false, Ordering::Release);
    }

    pub fn set_active_session(id: Option<&str>) {
        let h = id.map_or(0, hash_session);
        ACTIVE_HASH.store(h, Ordering::Release);
        if h != RESTORING_HASH.load(Ordering::Acquire) {
            RESTORE_SETTLED.store(true, Ordering::Release);
        }
    }

    pub fn mark_restore_settled(id: &str) {
        if hash_session(id) == RESTORING_HASH.load(Ordering::Acquire) {
            RESTORE_SETTLED.store(true, Ordering::Release);
        }
    }

    /// Consumes `<data>/skip-restore.bin` when the previous two launches closed with the same
    /// fatal reason while restoring session `id`, returning `true` so `restore_ui` opens Home.
    /// Costs zero `read_ops` / `write_ops` when no sentinel exists.
    pub fn take_skip_restore(id: &str) -> bool {
        let want = hash_session(id);
        if want == 0 {
            return false;
        }
        if SKIPPED_HASH.load(Ordering::Acquire) == want {
            return true;
        }
        let Some(paths) = PATHS.get() else { return false };
        if !paths.skip.exists() {
            return false;
        }
        let matched = std::fs::read(&paths.skip).ok().is_some_and(|b| {
            b.len() == SKIP_BYTES
                && u32::from_le_bytes([b[0], b[1], b[2], b[3]]) == SKIP_MAGIC
                && u64::from_le_bytes(b[8..16].try_into().unwrap_or([0; 8])) == want
        });
        if matched {
            let _ = std::fs::remove_file(&paths.skip);
            SKIPPED_HASH.store(want, Ordering::Release);
            RESTORE_SETTLED.store(true, Ordering::Release);
            eprintln!("last-exit: skipped restoring session {id} after 2 consecutive closes; opening Home");
        }
        matched
    }

    pub fn presented() {
        static STARTED: std::sync::Once = std::sync::Once::new();
        if crate::headless() {
            return;
        }
        init_hooks();
        STARTED.call_once(|| {
            let _ = rune::task::spawn("last-exit", open_once);
        });
    }

    fn open_once() {
        let Some(paths) = PATHS.get() else { return };
        if let Some(parent) = Path::new(&paths.record).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let Ok(mut file) = open_file(&paths.record) else { return };
        let mut raw = [0u8; RECORD_BYTES];
        let read_all = file.read(&mut raw).ok() == Some(RECORD_BYTES);
        let opened_home = SKIPPED_HASH.load(Ordering::Acquire) != 0;
        let (previous, prev_seq) = if read_all {
            if let Some((tag, _, _, _, _, _, _, sh, cons)) = decode_raw_fields(&raw) {
                PREV_TAG.store(tag, Ordering::Release);
                PREV_HASH.store(sh, Ordering::Release);
                PREV_CONSECUTIVE.store(cons, Ordering::Release);
            }
            decode_record(&raw, opened_home).unwrap_or((Previous::Clean, 0))
        } else {
            (Previous::Clean, 0)
        };
        if previous != Previous::Clean {
            eprintln!("last-exit: previous session recorded {previous:?}");
        }
        let next_seq = prev_seq.wrapping_add(1);
        SEQ.store(next_seq, Ordering::Release);
        PREV_LOADED.store(true, Ordering::Release);
        let effect = {
            let mut guard = STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            step(&mut guard, Event::Opened { seq: next_seq, previous })
        };
        if let Some(Effect::WriteRunning { seq }) = effect {
            let fd = into_raw_fd(file);
            RECORD_FD.store(fd, Ordering::Release);
            if !TERMINAL_RECORDED.load(Ordering::Acquire) && !TENTATIVE_PANIC.load(Ordering::Acquire) {
                let buf = encode_record(TAG_RUNNING, 0, 0, raw_pid(), 0, u64::MAX, seq, 0, 0);
                write_raw(fd, &buf);
            }
            if matches!(previous, Previous::Ended { .. }) {
                crate::storage::wake_all();
            }
            #[cfg(debug_assertions)]
            maybe_start_drill();
        }
    }

    /// Writes `Clean` (`tag = 0`) ahead of `storage::close()`, unless a terminal fault ran.
    pub fn mark_clean() {
        if TERMINAL_RECORDED.load(Ordering::Acquire) {
            return;
        }
        let effect = {
            let mut guard = STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            step(&mut guard, Event::CleanQuit)
        };
        if let Some(Effect::WriteClean { seq }) = effect {
            let fd = RECORD_FD.load(Ordering::Acquire);
            if fd != -1 && !TERMINAL_RECORDED.load(Ordering::Acquire) {
                if let Some(paths) = PATHS.get() {
                    let _ = std::fs::remove_file(&paths.skip);
                }
                let buf = encode_record(TAG_CLEAN, 0, 0, raw_pid(), 0, u64::MAX, seq, 0, 0);
                write_raw(fd, &buf);
            }
        }
    }

    pub fn poll_previous() -> PollPrevious {
        let mut guard = STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match *guard {
            State::Unopened => PollPrevious::Opening,
            State::Active { previous, taken: false, .. } => {
                let _ = step(&mut guard, Event::TakePrevious);
                PollPrevious::Ready(previous)
            }
            State::Active { taken: true, .. } | State::Closed { .. } => PollPrevious::Taken,
        }
    }

    pub(super) fn record_fault(
        tag: u8,
        flags: u8,
        code: u32,
        requested: u64,
        avail: u64,
        terminal: bool,
    ) {
        if terminal {
            if TERMINAL_RECORDED.swap(true, Ordering::AcqRel) {
                return;
            }
        } else if TERMINAL_RECORDED.load(Ordering::Acquire) || TENTATIVE_PANIC.swap(true, Ordering::AcqRel) {
            return;
        }

        let mut fd = RECORD_FD.load(Ordering::Acquire);
        let mut prev_tag = PREV_TAG.load(Ordering::Acquire);
        let mut prev_hash = PREV_HASH.load(Ordering::Acquire);
        let mut prev_cons = PREV_CONSECUTIVE.load(Ordering::Acquire);
        let mut seq = SEQ.load(Ordering::Acquire);

        let mut opened_temp = false;
        if fd == -1 || fd == 0 {
            fd = open_raw_emergency();
            if fd == -1 || fd == 0 {
                return;
            }
            opened_temp = true;
            if !PREV_LOADED.load(Ordering::Acquire) {
                let mut existing = [0u8; RECORD_BYTES];
                if read_raw(fd, &mut existing)
                    && let Some((pt, _, _, _, _, _, pseq, psh, pcons)) = decode_raw_fields(&existing)
                {
                    prev_tag = pt;
                    prev_hash = psh;
                    prev_cons = pcons;
                    seq = pseq.wrapping_add(1);
                } else {
                    seq = 1;
                }
            }
        }

        let restoring = !RESTORE_SETTLED.load(Ordering::Acquire);
        let session_hash = if restoring {
            RESTORING_HASH.load(Ordering::Acquire)
        } else {
            ACTIVE_HASH.load(Ordering::Acquire)
        };
        let consecutive = if session_hash != 0 && prev_tag == tag && prev_hash == session_hash {
            prev_cons.saturating_add(1).max(2)
        } else {
            1
        };

        let buf = encode_record(tag, flags, code, raw_pid(), requested, avail, seq, session_hash, consecutive);
        write_raw(fd, &buf);
        if opened_temp {
            close_raw(fd);
        }
        if restoring && session_hash != 0 && consecutive >= 2 {
            write_skip_sentinel(tag, session_hash);
        }
    }

    fn install_panic_hook() {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let on_main = MAIN_TID.get().is_some_and(|&tid| tid == std::thread::current().id());
            record_fault(TAG_FATAL, FLAG_PANIC, 101, 0, u64::MAX, on_main);
            prev(info);
        }));
    }

    #[cfg(debug_assertions)]
    static MAIN_DRILL_PENDING: AtomicU8 = AtomicU8::new(0);

    pub fn poll_debug_drill() {
        #[cfg(debug_assertions)]
        if MAIN_DRILL_PENDING.swap(0, Ordering::AcqRel) == 1 {
            panic!("crash-drill main panic");
        }
    }

    /// Proof-only crash drill on a scratch root: triple-gated by `#[cfg(debug_assertions)]`,
    /// `build_channel() == "local"`, and `dirs::rooted()`.
    #[cfg(debug_assertions)]
    fn maybe_start_drill() {
        if crate::updater::build_channel() != "local" || !crate::dirs::rooted() {
            return;
        }
        #[allow(clippy::disallowed_methods)]
        let Some(kind) = std::env::var("OPENCODE_RUNE_CRASH_DRILL").ok() else { return };
        #[allow(clippy::disallowed_methods)]
        let Some(root) = std::env::var_os("RUNE_DIRS_ROOT").map(PathBuf::from) else { return };
        let stack_bytes = if kind == "stack-commit" { 8 << 20 } else { 1 << 20 };
        let go = root.join("drill.go");
        let _ = std::thread::Builder::new().name("crash-drill".into()).stack_size(stack_bytes).spawn(move || {
            super::os::wait_drill_go(&go);
            match kind.as_str() {
                "heap" => {
                    let mut v = Vec::<u8>::new();
                    v.reserve_exact(16 << 20);
                    std::hint::black_box(&mut v);
                }
                "stack-commit" | "stack-overflow" => {
                    #[inline(never)]
                    fn grow(depth: usize) -> u8 {
                        let mut page = [depth as u8; 4096];
                        let inner = if depth == 0 { 0 } else { grow(depth - 1) };
                        page[depth & 4095] = page[depth & 4095].wrapping_add(inner);
                        std::hint::black_box(&mut page)[0]
                    }
                    std::hint::black_box(grow(4096));
                }
                "panic-main" => MAIN_DRILL_PENDING.store(1, Ordering::Release),
                "access-violation" => super::os::raise_access_violation(),
                "panic-caught" => {
                    let _ = std::panic::catch_unwind(|| panic!("caught background drill panic"));
                    let _ = std::fs::write(root.join("drill.caught"), "ok");
                }
                _ => {}
            }
        });
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use imp::{
    OomGuard, init_hooks, mark_clean, mark_restore_settled, note_restoring_session,
    poll_debug_drill, poll_previous, presented, set_active_session, take_skip_restore,
};
