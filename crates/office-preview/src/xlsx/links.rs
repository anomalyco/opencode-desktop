//! Where a sheet's link goes (`@betteroffice/xlsx` `hyperlinks`, `spreadsheet.tsx` `sheetLinkUrl`
//! and `links.ts` `externalUrl`): a web or mail address the host may open, or a cell of a sheet.

pub use betteroffice_xlsx::HyperlinkRegion;

/// A link's web or mail address (`sheetLinkUrl`): the engine's check (`safeExternalHyperlink`,
/// which lets `tel:` through and adds the link's location as the fragment), then the schemes the
/// host opens (`externalUrl`: http, https and mailto), normalized as a URL's `href`.
pub fn link_url(link: &HyperlinkRegion) -> Option<String> {
    let href = link.external_target.as_deref()?;
    // `sanitizeHref`: tabs and line breaks go anywhere, controls and spaces before it.
    let probe: String = href.chars().filter(|c| !matches!(c, '\t' | '\n' | '\r')).collect();
    let probe = probe.trim_start_matches(|c: char| c <= ' ').trim_end_matches(|c: char| c <= ' ');
    let colon = probe.find(':')?;
    let scheme = probe[..colon].to_ascii_lowercase();
    let valid = scheme.starts_with(|c: char| c.is_ascii_alphabetic()) && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c));
    if !valid || !matches!(scheme.as_str(), "http" | "https" | "mailto") {
        return None;
    }
    let rest = &probe[colon + 1..];
    let mut url = match scheme.as_str() {
        "mailto" => format!("mailto:{}", encode(rest)),
        _ => {
            // A web address needs its host (`new URL` throws without one).
            let after = rest.strip_prefix("//").or_else(|| rest.strip_prefix("\\\\"))?;
            let end = after.find(['/', '\\', '?', '#']).unwrap_or(after.len());
            let host = &after[..end];
            if host.is_empty() || host.contains(char::is_whitespace) {
                return None;
            }
            let tail = after[end..].replace('\\', "/");
            let tail = if tail.starts_with('/') { tail } else { format!("/{tail}") };
            format!("{scheme}://{}{}", host.to_ascii_lowercase(), encode(&tail))
        }
    };
    if let Some(location) = link.location.as_deref().filter(|_| !url.contains('#')) {
        url.push('#');
        url.push_str(&encode(location.trim_start_matches('#')));
    }
    Some(url)
}

/// A link's place in the workbook (`parseHyperlinkLocation`): "Sheet2!B3", "'My sheet'!$A$1" or
/// "C4" on `current`; its sheet's name and zero-based row and column.
pub fn link_location(location: &str, current: &str) -> Option<(String, [u32; 2])> {
    let target = location.strip_prefix('#').unwrap_or(location);
    let (sheet, address) = match target.rfind('!') {
        Some(at) => (&target[..at], &target[at + 1..]),
        None => (current, target),
    };
    let address = address.split(':').next().unwrap_or_default();
    let address = address.strip_prefix('$').unwrap_or(address);
    let split = address.find(|c: char| !c.is_ascii_alphabetic())?;
    let (letters, digits) = address.split_at(split);
    let digits = digits.strip_prefix('$').unwrap_or(digits);
    if letters.is_empty() || letters.len() > 3 || digits.is_empty() || digits.starts_with('0') || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let col = letters.bytes().fold(0u32, |n, b| n * 26 + u32::from(b.to_ascii_uppercase() - b'A' + 1));
    let row = digits.parse::<u32>().ok()?;
    if !(1..=16_384).contains(&col) || !(1..=1_048_576).contains(&row) {
        return None;
    }
    let sheet = match sheet.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
        Some(quoted) => quoted.replace("''", "'"),
        None => sheet.to_owned(),
    };
    (!sheet.is_empty()).then(|| (sheet, [row - 1, col - 1]))
}

/// Percent-encodes what a URL's path, query and fragment can't hold as it is: spaces, controls,
/// quotes, angle brackets, backticks and non-ASCII.
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b <= b' ' || b >= 0x7f || matches!(b, b'"' | b'<' | b'>' | b'`') {
            out.push_str(&format!("%{b:02X}"));
        } else {
            out.push(b as char);
        }
    }
    out
}
