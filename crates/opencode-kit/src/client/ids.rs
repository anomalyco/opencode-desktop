//! Ids the app makes before the server has seen them, as the server makes its own.

/// A new `SessionMessage.ID` as the server makes them (`Identifier.ascending`): `msg_`, 12 hex
/// digits of `ms * 0x1000 + counter` (its low 48 bits) and 14 random base62 characters. They
/// sort by time, so a message sent now lands after the transcript's others.
pub fn message_id() -> String {
    message_id_at(now_ms())
}

/// A new `Session.ID` as the server makes them (`Identifier.descending`): `ses_`, then the
/// message id's digits with the time inverted, so newer sessions sort first.
pub fn session_id() -> String {
    id_at("ses", now_ms(), true)
}

fn now_ms() -> u64 {
    rune::time::SystemTime::now().duration_since(rune::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

pub(super) fn message_id_at(ms: u64) -> String {
    id_at("msg", ms, false)
}

pub(super) fn id_at(prefix: &str, ms: u64, descending: bool) -> String {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::Mutex;
    static LAST: Mutex<(u64, u64)> = Mutex::new((0, 0));
    let counter = {
        let mut last = LAST.lock().unwrap();
        if last.0 != ms {
            *last = (ms, 0);
        }
        last.1 += 1;
        last.1
    };
    let mut value = ms.wrapping_mul(0x1000).wrapping_add(counter);
    if descending {
        value = !value;
    }
    let value = value & 0xffff_ffff_ffff;
    const CHARS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut out = format!("{prefix}_{value:012x}");
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(value);
    let mut r = h.finish();
    for k in 0..14 {
        if k == 7 {
            h.write_u64(r);
            r = h.finish();
        }
        out.push(CHARS[(r % 62) as usize] as char);
        r /= 62;
    }
    out
}
