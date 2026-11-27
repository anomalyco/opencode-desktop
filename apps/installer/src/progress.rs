//! Monotonic install progress, live stage status, byte throughput formatting, and ZIP size probing.

use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Win32 TaskDialog progress bar range maximum (`0..=10_000`, 0.01% resolution for smooth animation).
pub const PROGRESS_MAX: u16 = 10_000;

const KIB: u64 = 1024;
const MIB: u64 = 1024 * 1024;
/// Wait briefly before displaying MB/s so initial bursts do not spike wildly.
const THROUGHPUT_MIN_ELAPSED: Duration = Duration::from_millis(150);

type Notify = Arc<dyn Fn(&Snapshot) + Send + Sync>;

struct State {
    pos: u16,
    cap: u16,
    creep_step: u16,
    status: String,
}

/// Thread-safe, strictly monotonic progress state shared between the install worker and the UI.
#[derive(Clone)]
pub struct Progress {
    state: Arc<Mutex<State>>,
    notify: Option<Notify>,
}

/// Immutable progress reading for the TaskDialog UI.
pub struct Snapshot {
    pub pos: u16,
    pub percent: u8,
    pub status: String,
}

impl Progress {
    /// Detached progress tracker for `--silent` runs (no UI notifications).
    pub fn noop() -> Self {
        Self::with_notify(None)
    }

    /// Progress tracker that invokes `notify` whenever the worker publishes a new position or status.
    /// The callback is always invoked after releasing the internal lock.
    pub fn with_notify(notify: Option<Notify>) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                pos: 0,
                cap: 0,
                creep_step: 0,
                status: String::from("Preparing installation..."),
            })),
            notify,
        }
    }

    /// Advances to at least `pos` with an exact status and disables timer creeping.
    pub fn set(&self, pos: u16, status: impl Into<String>) {
        self.update_inner(pos, 0, 0, Some(status.into()));
    }

    /// Updates only the status message without changing position or creep settings.
    pub fn set_status(&self, status: impl Into<String>) {
        let snap = {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let next = status.into();
            if s.status == next {
                None
            } else {
                s.status = next;
                Some(snapshot_of(&s))
            }
        };
        if let (Some(snap), Some(notify)) = (snap, &self.notify) {
            notify(&snap);
        }
    }

    /// Advances to at least `pos` and allows UI timer ticks to creep up toward `cap` by `creep_step`.
    pub fn set_waiting(&self, pos: u16, cap: u16, creep_step: u16, status: impl Into<String>) {
        self.update_inner(pos, cap.min(PROGRESS_MAX), creep_step, Some(status.into()));
    }

    /// Maps `done / total` linearly into `start..=end` (monotonic) and updates the status line.
    pub fn update_range(&self, start: u16, end: u16, done: u64, total: u64, status: impl Into<String>) {
        let pos = map_range(start, end, done, total);
        self.update_inner(pos, 0, 0, Some(status.into()));
    }

    /// Maps `done / total` into `start..=end` while keeping gentle timer creep active toward `cap`.
    pub fn update_waiting_range(&self, start: u16, end: u16, cap: u16, creep_step: u16, done: u64, total: u64) {
        let pos = map_range(start, end, done, total);
        self.update_inner(pos, cap.min(PROGRESS_MAX), creep_step, None);
    }

    /// Called by the UI timer (`TDN_TIMER`) to advance gentle wait-stage creep.
    /// Returns `Some(snapshot)` only when creep moved the position forward.
    pub fn tick(&self) -> Option<Snapshot> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.creep_step == 0 || s.pos >= s.cap {
            return None;
        }
        s.pos = s.pos.saturating_add(s.creep_step).min(s.cap);
        Some(snapshot_of(&s))
    }

    fn update_inner(&self, pos: u16, cap: u16, creep_step: u16, status: Option<String>) {
        let snap = {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let next_pos = pos.min(PROGRESS_MAX).max(s.pos);
            let mut dirty = next_pos != s.pos;
            s.pos = next_pos;
            s.cap = cap.max(s.pos);
            s.creep_step = creep_step;
            if let Some(next_status) = status
                && s.status != next_status
            {
                s.status = next_status;
                dirty = true;
            }
            dirty.then(|| snapshot_of(&s))
        };
        if let (Some(snap), Some(notify)) = (snap, &self.notify) {
            notify(&snap);
        }
    }
}

fn snapshot_of(s: &State) -> Snapshot {
    Snapshot {
        pos: s.pos,
        percent: percent_of(s.pos),
        status: s.status.clone(),
    }
}

/// Whole percentage `0..=100`, staying at `99` until `pos` reaches `PROGRESS_MAX`.
pub fn percent_of(pos: u16) -> u8 {
    ((u32::from(pos) * 100) / u32::from(PROGRESS_MAX)).min(100) as u8
}

/// Formats `<percent>% - <status>` for the TaskDialog content line.
pub fn format_content(percent: u8, status: &str) -> String {
    format!("{percent}% - {status}")
}

/// Formats a live byte-transfer status such as `Extracting... 124 / 195 MB (142 MB/s)`.
pub fn format_bytes_status(verb: &str, done: u64, total: u64, elapsed: Duration) -> String {
    if total == 0 {
        return verb.to_owned();
    }
    let clamped = done.min(total);
    let rate = format_rate(clamped, elapsed);
    if total >= MIB {
        let total_mb = (total.saturating_add(MIB / 2)) / MIB;
        let done_mb = ((clamped.saturating_add(MIB / 2)) / MIB).min(total_mb);
        match rate {
            Some(r) => format!("{verb} {done_mb} / {total_mb} MB ({r})"),
            None => format!("{verb} {done_mb} / {total_mb} MB"),
        }
    } else if total >= KIB {
        let total_kb = (total.saturating_add(KIB / 2)) / KIB;
        let done_kb = ((clamped.saturating_add(KIB / 2)) / KIB).min(total_kb);
        match rate {
            Some(r) => format!("{verb} {done_kb} / {total_kb} KB ({r})"),
            None => format!("{verb} {done_kb} / {total_kb} KB"),
        }
    } else {
        verb.to_owned()
    }
}

fn format_rate(done: u64, elapsed: Duration) -> Option<String> {
    if done == 0 || elapsed < THROUGHPUT_MIN_ELAPSED {
        return None;
    }
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 {
        return None;
    }
    let bps = (done as f64) / secs;
    if bps >= 10.0 * (MIB as f64) {
        Some(format!("{:.0} MB/s", bps / (MIB as f64)))
    } else if bps >= MIB as f64 {
        Some(format!("{:.1} MB/s", bps / (MIB as f64)))
    } else if bps >= KIB as f64 {
        Some(format!("{:.0} KB/s", bps / (KIB as f64)))
    } else {
        None
    }
}

fn map_range(start: u16, end: u16, done: u64, total: u64) -> u16 {
    if total == 0 || end <= start {
        return start.min(PROGRESS_MAX);
    }
    let span = u64::from(end - start);
    let ratio = done.min(total).saturating_mul(span) / total;
    start.saturating_add(ratio as u16).min(end).min(PROGRESS_MAX)
}

/// Reads the total uncompressed byte size of all entries from a ZIP archive's Central Directory.
/// Supports standard ZIP and ZIP64 headers without external dependencies.
pub fn zip_uncompressed_bytes(zip: &[u8]) -> Option<u64> {
    const EOCD_SIG: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
    const ZIP64_LOC_SIG: [u8; 4] = [0x50, 0x4b, 0x06, 0x07];
    const ZIP64_EOCD_SIG: [u8; 4] = [0x50, 0x4b, 0x06, 0x06];
    const CD_SIG: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];

    if zip.len() < 22 {
        return None;
    }
    let min_eocd = zip.len().saturating_sub(22 + 65_535);
    let mut eocd = None;
    let mut i = zip.len() - 22;
    loop {
        if zip.get(i..i + 4) == Some(&EOCD_SIG) {
            let comment_len = usize::from(read_u16(zip, i + 20)?);
            if i + 22 + comment_len == zip.len() {
                eocd = Some(i);
                break;
            }
        }
        if i == min_eocd {
            break;
        }
        i -= 1;
    }
    let eocd = eocd?;

    let mut entries = u64::from(read_u16(zip, eocd + 10)?);
    let mut cd_size = u64::from(read_u32(zip, eocd + 12)?);
    let mut cd_offset = u64::from(read_u32(zip, eocd + 16)?);

    if (entries == 0xFFFF || cd_size == 0xFFFF_FFFF || cd_offset == 0xFFFF_FFFF)
        && eocd >= 20
        && zip.get(eocd - 20..eocd - 16) == Some(&ZIP64_LOC_SIG)
    {
        let z64_off = usize::try_from(read_u64(zip, eocd - 12)?).ok()?;
        if zip.get(z64_off..z64_off + 4) == Some(&ZIP64_EOCD_SIG) {
            entries = read_u64(zip, z64_off + 32)?;
            cd_size = read_u64(zip, z64_off + 40)?;
            cd_offset = read_u64(zip, z64_off + 48)?;
        }
    }

    let mut pos = usize::try_from(cd_offset).ok()?;
    let end = pos.checked_add(usize::try_from(cd_size).ok()?)?.min(zip.len());
    let mut total: u64 = 0;
    let mut count: u64 = 0;

    while pos + 46 <= end && count < entries {
        if zip.get(pos..pos + 4) != Some(&CD_SIG) {
            break;
        }
        let uncomp32 = read_u32(zip, pos + 24)?;
        let name_len = usize::from(read_u16(zip, pos + 28)?);
        let extra_len = usize::from(read_u16(zip, pos + 30)?);
        let comment_len = usize::from(read_u16(zip, pos + 32)?);

        let extra_start = pos.checked_add(46)?.checked_add(name_len)?;
        let extra_end = extra_start.checked_add(extra_len)?;
        let next_pos = extra_end.checked_add(comment_len)?;
        if next_pos > end {
            break;
        }

        let uncomp = if uncomp32 == 0xFFFF_FFFF {
            zip64_extra_uncompressed(zip.get(extra_start..extra_end)?)?
        } else {
            u64::from(uncomp32)
        };
        total = total.saturating_add(uncomp);
        count += 1;
        pos = next_pos;
    }

    (total > 0).then_some(total)
}

fn zip64_extra_uncompressed(extra: &[u8]) -> Option<u64> {
    let mut off = 0;
    while off + 4 <= extra.len() {
        let id = read_u16(extra, off)?;
        let len = usize::from(read_u16(extra, off + 2)?);
        let data = extra.get(off + 4..off + 4 + len)?;
        if id == 0x0001 {
            return read_u64(data, 0);
        }
        off += 4 + len;
    }
    None
}

fn read_u16(buf: &[u8], at: usize) -> Option<u16> {
    let slice: [u8; 2] = buf.get(at..at + 2)?.try_into().ok()?;
    Some(u16::from_le_bytes(slice))
}

fn read_u32(buf: &[u8], at: usize) -> Option<u32> {
    let slice: [u8; 4] = buf.get(at..at + 4)?.try_into().ok()?;
    Some(u32::from_le_bytes(slice))
}

fn read_u64(buf: &[u8], at: usize) -> Option<u64> {
    let slice: [u8; 8] = buf.get(at..at + 8)?.try_into().ok()?;
    Some(u64::from_le_bytes(slice))
}
