//! Numbers, money and times as Desktop's English `Intl` formats them.

/// `toLocaleString("en")`: `200000` → `200,000`.
pub fn number(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `Intl.NumberFormat("en", { notation: "compact", maximumFractionDigits: 0 })`, lowercased:
/// `950` → `950`, `1499` → `1k`, `1500` → `2k`, `999999` → `1m`.
pub fn compact(n: u64) -> String {
    const UNITS: [(f64, &str); 4] = [(1e12, "t"), (1e9, "b"), (1e6, "m"), (1e3, "k")];
    let v = n as f64;
    // The unit is chosen after rounding, so 999,999 reads 1m rather than 1000k.
    for (k, (scale, unit)) in UNITS.iter().enumerate() {
        let r = (v / scale).round();
        if r >= 1.0 {
            if r >= 1000.0 && k > 0 {
                let (up, upper) = UNITS[k - 1];
                return format!("{}{upper}", (v / up).round());
            }
            return format!("{r}{unit}");
        }
    }
    n.to_string()
}

/// `Intl.NumberFormat("en", { style: "currency", currency: "USD" })`: `$1,234.50`.
pub fn usd(v: f64) -> String {
    let cents = (v.abs() * 100.0).round() as u64;
    let sign = if v < 0.0 && cents > 0 { "-" } else { "" };
    format!("{sign}${}.{:02}", number(cents / 100), cents % 100)
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// The DATETIME_MED fields Desktop formats with: `Nov 7, 2026, 6:03 AM`, at `offset` seconds
/// east of UTC.
pub fn date_time(ms: i64, offset: i64) -> String {
    let secs = ms.div_euclid(1000) + offset;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil(days);
    let (hour, minute) = (rem / 3600, rem % 3600 / 60);
    let h = match hour % 12 {
        0 => 12,
        h => h,
    };
    format!("{} {d}, {y}, {h}:{minute:02} {}", MONTHS[(m - 1) as usize], if hour < 12 { "AM" } else { "PM" })
}

/// Days since the Unix epoch as (year, month, day).
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}
