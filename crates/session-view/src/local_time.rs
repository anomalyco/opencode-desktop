//! An instant as the user's wall clock reads it: the OS time zone, with daylight saving and its
//! offset from UTC at that instant.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Local {
    pub year: i32,
    /// 1-12.
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    /// Seconds east of UTC.
    pub offset: i32,
}

/// `ms` (epoch ms) in the OS time zone.
#[cfg(unix)]
#[allow(unsafe_code)]
pub fn local(ms: i64) -> Local {
    let secs = ms.div_euclid(1000) as libc::time_t;
    // SAFETY: `libc::tm` is a plain C struct where all-zero bytes are valid.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: `localtime_r` reads `secs` and writes only into `tm`.
    unsafe { libc::localtime_r(&secs, &mut tm) };
    Local { year: tm.tm_year + 1900, month: tm.tm_mon as u32 + 1, day: tm.tm_mday as u32, hour: tm.tm_hour as u32, minute: tm.tm_min as u32, offset: tm.tm_gmtoff as i32 }
}

/// `ms` (epoch ms) in the OS time zone. The dynamic zone carries the zone's yearly daylight
/// saving rules, so a past or future instant gets the offset of its own year.
#[cfg(windows)]
#[allow(unsafe_code)]
pub fn local(ms: i64) -> Local {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::Time::{
        DYNAMIC_TIME_ZONE_INFORMATION, FileTimeToSystemTime, GetDynamicTimeZoneInformation, SystemTimeToFileTime,
        SystemTimeToTzSpecificLocalTimeEx,
    };

    // FILETIME counts 100 ns ticks from 1601-01-01 UTC.
    const EPOCH_TICKS: i64 = 116_444_736_000_000_000;
    let ticks = |f: FILETIME| ((u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime)) as i64;
    let utc_ticks = EPOCH_TICKS + ms.div_euclid(1000) * 10_000_000;
    let utc_file = FILETIME { dwLowDateTime: utc_ticks as u32, dwHighDateTime: (utc_ticks >> 32) as u32 };
    // SAFETY: plain C structs where all-zero bytes are valid.
    let (mut utc, mut wall, mut zone, mut wall_file): (SYSTEMTIME, SYSTEMTIME, DYNAMIC_TIME_ZONE_INFORMATION, FILETIME) =
        unsafe { std::mem::zeroed() };
    // SAFETY: each call reads and writes only the structs passed to it, which outlive the calls.
    let ok = unsafe {
        FileTimeToSystemTime(&utc_file, &mut utc) != 0
            && GetDynamicTimeZoneInformation(&mut zone) != u32::MAX
            && SystemTimeToTzSpecificLocalTimeEx(&zone, &utc, &mut wall) != 0
            && SystemTimeToFileTime(&wall, &mut wall_file) != 0
    };
    // On failure, UTC; an instant FILETIME can't hold (before 1601) leaves `utc` zeroed, so keep
    // month and day in range for the formatters.
    let (t, offset) = if ok { (wall, (ticks(wall_file) - utc_ticks) / 10_000_000) } else { (utc, 0) };
    Local {
        year: i32::from(t.wYear),
        month: u32::from(t.wMonth).max(1),
        day: u32::from(t.wDay).max(1),
        hour: u32::from(t.wHour),
        minute: u32::from(t.wMinute),
        offset: offset as i32,
    }
}
