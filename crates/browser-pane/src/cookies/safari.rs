//! Bounds-checked Safari `Cookies.binarycookies` parser (`profile/safari.ts`).
//!
//! Every count and offset is checked against `data.len()` so damaged or hostile files cost no more
//! than O(size) and never allocate unboundedly. With `read_values == false`, every cookie value is
//! left empty for the Choose-sites list.

use super::CookieRead;
use crate::protocol::{CookieSameSite, WireCookie};

/// Seconds between 1970-01-01 and 2001-01-01 (Apple/Cocoa reference epoch).
const MAC_EPOCH_OFFSET: f64 = 978_307_200.0;

pub fn parse_binary_cookies(data: &[u8], read_values: bool) -> Option<CookieRead> {
    if data.len() < 8 || &data[..4] != b"cook" {
        return None;
    }
    let page_count = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;
    let table_end = 8usize.checked_add(page_count.checked_mul(4)?)?;
    if table_end > data.len() {
        return None;
    }

    let mut cookies = Vec::new();
    let mut failed = 0u32;
    let mut cursor = table_end;

    for i in 0..page_count {
        let p = 8 + i * 4;
        let size = u32::from_be_bytes([data[p], data[p + 1], data[p + 2], data[p + 3]]) as usize;
        let start = cursor;
        cursor = cursor.saturating_add(size);
        let avail = data.len().saturating_sub(start).min(size);
        if avail < 8 {
            failed = failed.saturating_add(1);
            continue;
        }
        let page = &data[start..start + avail];
        let records = u32::from_le_bytes([page[4], page[5], page[6], page[7]]) as usize;
        let rec_table_end = match records.checked_mul(4).and_then(|n| 8usize.checked_add(n)) {
            Some(end) if end <= page.len() => end,
            _ => {
                failed = failed.saturating_add(1);
                continue;
            }
        };
        let _ = rec_table_end;
        for r in 0..records {
            let rp = 8 + r * 4;
            let offset =
                u32::from_le_bytes([page[rp], page[rp + 1], page[rp + 2], page[rp + 3]]) as usize;
            if let Some(cookie) = read_record(page, offset, read_values) {
                cookies.push(cookie);
            } else {
                failed = failed.saturating_add(1);
            }
        }
    }

    Some(CookieRead {
        cookies,
        protected: 0,
        failed,
        skipped: 0,
    })
}

fn read_record(page: &[u8], offset: usize, read_values: bool) -> Option<WireCookie> {
    if offset.checked_add(56)? > page.len() {
        return None;
    }
    let size = u32::from_le_bytes(page[offset..offset + 4].try_into().ok()?) as usize;
    let end = offset.checked_add(size)?;
    if end > page.len() || size < 56 {
        return None;
    }
    let flags = u32::from_le_bytes(page[offset + 8..offset + 12].try_into().ok()?);
    let read_str = |field: usize| -> Option<String> {
        let rel = u32::from_le_bytes(page[offset + field..offset + field + 4].try_into().ok()?)
            as usize;
        let start = offset.checked_add(rel)?;
        c_string(page, start, end)
    };
    let host = read_str(16)?;
    let name = read_str(20)?;
    let path = read_str(24)?;
    let value = if read_values { read_str(28)? } else { String::new() };
    let expiry = f64::from_le_bytes(page[offset + 40..offset + 48].try_into().ok()?);
    let expires = if expiry.is_finite() && expiry > 0.0 {
        Some(expiry + MAC_EPOCH_OFFSET)
    } else {
        None
    };

    Some(WireCookie {
        host,
        name,
        value,
        path: if path.is_empty() { "/".to_owned() } else { path },
        secure: (flags & 1) != 0,
        http_only: (flags & 4) != 0,
        expires,
        same_site: CookieSameSite::Unspecified,
    })
}

fn c_string(page: &[u8], start: usize, end: usize) -> Option<String> {
    if start >= end || end > page.len() {
        return None;
    }
    let slice = &page[start..end];
    let nul = slice.iter().position(|&b| b == 0)?;
    Some(String::from_utf8_lossy(&slice[..nul]).into_owned())
}
