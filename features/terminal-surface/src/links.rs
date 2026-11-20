//! What Ctrl+click (⌘-click on macOS) on terminal output opens: the cell's OSC 8 link, else a web
//! address or a file reference in the line under the pointer, joined across soft wraps.
//!
//! | Under the pointer | Opens |
//! | --- | --- |
//! | An OSC 8 hyperlink | Its URI |
//! | `https://…`, `http://…`, `file://…` (balanced brackets kept, trailing punctuation dropped) | The address |
//! | `src/a.rs:12:5`, `src/a.rs:12`, `src\a.ts(12,5)`, `File "a.py", line 9`, a bare `src/a.rs` | The file in the side viewer at that line, resolved against the shell's folder |

use rune::Tx;
use terminal::{CellFlags, TerminalSnapshot};

use crate::TerminalSurfaceHost;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Url(String),
    File { path: String, line: Option<u32> },
}

/// The line under (`col`, `row`): its text, joined across soft wraps, and for each char the
/// viewport cell it starts in; `at` is the char under the pointer.
struct Line {
    chars: Vec<char>,
    cells: Vec<(u16, u16)>,
    at: usize,
}

fn line_at(snap: &TerminalSnapshot, col: u16, row: u16) -> Option<Line> {
    let rows = &snap.screen_rows;
    let r = usize::from(row);
    rows.get(r)?;
    let mut first = r;
    while first > 0 && rows[first - 1].wrapped {
        first -= 1;
    }
    let mut last = r;
    while rows[last].wrapped && last + 1 < rows.len() {
        last += 1;
    }
    let (mut chars, mut cells, mut at) = (Vec::new(), Vec::new(), None);
    for (ri, row_data) in rows.iter().enumerate().take(last + 1).skip(first) {
        for (ci, cell) in row_data.cells.iter().enumerate() {
            let here = (ci as u16, ri as u16);
            if cell.flags.contains(CellFlags::WIDE_SPACER_TAIL) {
                if here == (col, row) {
                    at = Some(chars.len().saturating_sub(1));
                }
                continue;
            }
            if here == (col, row) {
                at = Some(chars.len());
            }
            let text: &str = &cell.grapheme;
            let mut it = text.chars();
            chars.push(it.next().unwrap_or(' '));
            cells.push(here);
            // Combining marks ride on their cell.
            for extra in it {
                chars.push(extra);
                cells.push(here);
            }
        }
    }
    Some(Line { chars, cells, at: at? })
}

/// The web address or file reference under (`col`, `row`).
pub fn at(snap: &TerminalSnapshot, col: u16, row: u16) -> Option<Target> {
    let line = line_at(snap, col, row)?;
    let (start, end) = token(&line.chars, line.at)?;
    let word: String = line.chars[start..end].iter().collect();
    if let Some(url) = url(&word) {
        return Some(Target::Url(url));
    }
    let rest: String = line.chars[end..].iter().collect();
    file(&word, &rest)
}

/// The cells of the link under (`col`, `row`), a run per row (row, first column, the column past
/// its last): the cell's OSC 8 run, else the address or file reference [`at`] reads there. Empty
/// off a link.
pub fn span(snap: &TerminalSnapshot, col: u16, row: u16) -> Vec<(u16, u16, u16)> {
    let Some(cells) = snap.screen_rows.get(usize::from(row)).map(|r| &r.cells) else { return Vec::new() };
    let linked = |i: usize| cells.get(i).is_some_and(|c| c.flags.contains(CellFlags::HYPERLINK));
    if linked(usize::from(col)) {
        let (mut a, mut b) = (usize::from(col), usize::from(col) + 1);
        while a > 0 && linked(a - 1) {
            a -= 1;
        }
        while linked(b) {
            b += 1;
        }
        return vec![(row, a as u16, b as u16)];
    }
    let Some(line) = line_at(snap, col, row) else { return Vec::new() };
    let Some((start, end)) = token(&line.chars, line.at) else { return Vec::new() };
    let word: String = line.chars[start..end].iter().collect();
    let rest: String = line.chars[end..].iter().collect();
    // The characters of `word` the link covers: an address within it, or the reference's path
    // and position without the punctuation after it.
    let (from, len) = match url(&word) {
        Some(u) => (word[..word.find(&u).unwrap_or(0)].chars().count(), u.chars().count()),
        None if file(&word, &rest).is_some() => (0, word.trim_end_matches(['.', ',', ';', ':', ')', ']']).chars().count()),
        None => return Vec::new(),
    };
    let mut runs: Vec<(u16, u16, u16)> = Vec::new();
    for &(c, r) in &line.cells[start + from..(start + from + len).min(line.cells.len())] {
        let width = snap.screen_rows.get(usize::from(r)).and_then(|row| row.cells.get(usize::from(c))).map_or(1, |cell| u16::from(cell.width.max(1)));
        match runs.last_mut() {
            Some(run) if run.0 == r && c < run.2 + 1 => run.2 = run.2.max(c + width),
            _ => runs.push((r, c, c + width)),
        }
    }
    runs
}

/// The run of non-space characters around `at`, without the quotes and brackets that wrap it.
fn token(chars: &[char], at: usize) -> Option<(usize, usize)> {
    let stop = |c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '<' | '>' | '|');
    if stop(*chars.get(at)?) {
        return None;
    }
    let mut start = at;
    while start > 0 && !stop(chars[start - 1]) {
        start -= 1;
    }
    let mut end = at + 1;
    while end < chars.len() && !stop(chars[end]) {
        end += 1;
    }
    // An opening bracket before the word, and its closer, aren't part of it (`(see x.rs:3)`).
    while start < end && matches!(chars[start], '(' | '[' | '{') {
        start += 1;
    }
    Some((start, end)).filter(|(s, e)| s < e && *s <= at)
}

fn url(word: &str) -> Option<String> {
    let start = ["https://", "http://", "file://"].iter().filter_map(|p| word.find(p)).min()?;
    let mut url = &word[start..];
    // Trailing punctuation ends a sentence, not the address; a closing bracket stays only when
    // the address opened it (`https://en.wikipedia.org/wiki/Rust_(language)`).
    loop {
        let trimmed = url.trim_end_matches(['.', ',', ';', ':', '!', '?']);
        let trimmed = match trimmed.chars().last() {
            Some(c @ (')' | ']' | '}')) => {
                let open = match c { ')' => '(', ']' => '[', _ => '{' };
                if trimmed.matches(open).count() < trimmed.matches(c).count() { &trimmed[..trimmed.len() - 1] } else { trimmed }
            }
            _ => trimmed,
        };
        if trimmed.len() == url.len() {
            break;
        }
        url = trimmed;
    }
    (url.len() > url.find("://")? + 3).then(|| url.to_owned())
}

/// `path:line:col`, `path:line`, `path(line,col)`, `path(line)`, a quoted path followed by
/// `, line N` (Python), or a bare path that looks like one (a separator or an extension).
fn file(word: &str, rest: &str) -> Option<Target> {
    let word = word.trim_end_matches(['.', ',', ';', ':', ')', ']']);
    // MSVC and dotnet: `src\a.ts(12,5)`.
    if let Some(open) = word.rfind('(')
        && let Some(nums) = word[open + 1..].strip_suffix(')').or(Some(&word[open + 1..]))
        && let Some(line) = nums.split(',').next().and_then(|n| n.trim().parse::<u32>().ok())
    {
        return looks_like_path(&word[..open]).then(|| Target::File { path: word[..open].to_owned(), line: Some(line) });
    }
    // `path:12:5` and `path:12`; a drive letter's colon (`C:\x`) isn't one.
    let mut path = word;
    let mut line = None;
    for _ in 0..2 {
        if let Some(i) = path.rfind(':').filter(|&i| i > 1)
            && let Ok(n) = path[i + 1..].parse::<u32>()
        {
            line = Some(n);
            path = &path[..i];
        }
    }
    // Python's `File "a.py", line 9`.
    if line.is_none()
        && let Some(n) = rest.trim_start_matches(['"', '\'']).trim_start().strip_prefix(", line ").or_else(|| rest.trim_start_matches(['"', '\'']).strip_prefix(", line "))
    {
        line = n.split(|c: char| !c.is_ascii_digit()).next().and_then(|d| d.parse().ok());
    }
    looks_like_path(path).then(|| Target::File { path: path.to_owned(), line })
}

fn looks_like_path(p: &str) -> bool {
    if p.is_empty() || p.len() > 1024 || p.contains("://") {
        return false;
    }
    let name = p.rsplit(['/', '\\']).next().unwrap_or(p);
    let ext = name.rsplit_once('.').is_some_and(|(stem, ext)| !stem.is_empty() && !ext.is_empty() && ext.len() <= 12 && ext.chars().all(|c| c.is_ascii_alphanumeric()));
    let separated = p.contains(['/', '\\']) && !name.is_empty();
    // A version number or a time isn't a file (`1.2.3`, `12:30`).
    let numeric = name.chars().all(|c| c.is_ascii_digit() || c == '.');
    (ext || separated) && !numeric
}

/// The shell's folder as a path: OSC 7's `file://host/path` (percent-decoded; `/C:/x` is `C:/x`),
/// or the plain path of Windows Terminal's OSC 9;9 (Nushell and PowerShell prompts send it,
/// sometimes quoted), else the folder the terminal started in.
pub fn shell_folder(snap: &TerminalSnapshot, started_in: &str) -> String {
    let Some(rest) = snap.pwd.strip_prefix("file://") else {
        let plain = snap.pwd.trim().trim_matches('"');
        return if plain.is_empty() { started_in.to_owned() } else { plain.to_owned() };
    };
    let path = rest.find('/').map_or("", |i| &rest[i..]);
    let decoded = percent_decode(path);
    let b = decoded.as_bytes();
    if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        return decoded[1..].to_owned();
    }
    if decoded.is_empty() {
        return started_in.to_owned();
    }
    // A Windows server's shells: Git Bash says `/c/x` and WSL `/mnt/c/x` for `C:/x`; a folder only
    // WSL has (`/home/u`) is none the server can start in, so the terminal's own is used.
    let windows = started_in.as_bytes().get(1) == Some(&b':');
    let drive = decoded.strip_prefix("/mnt").unwrap_or(&decoded);
    match drive.as_bytes() {
        [b'/', d, rest @ ..] if windows && d.is_ascii_alphabetic() && (rest.is_empty() || rest[0] == b'/') => format!("{}:{}", char::from(d.to_ascii_uppercase()), if rest.is_empty() { "/" } else { &drive[2..] }),
        _ if windows => started_in.to_owned(),
        _ => decoded,
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && b[i + 1].is_ascii_hexdigit()
            && b[i + 2].is_ascii_hexdigit()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Ctrl+click (⌘-click) at (`col`, `row`) of terminal `tid`: opens the cell's OSC 8 link (with
/// confirmation when the target differs from the visible text or uses a non-web scheme), else
/// what [`at`] finds. Plain web URLs route through [`TerminalSurfaceHost::open_web_link`] so
/// browser URL policy & `Settings > Browser` always decide.
pub fn follow(host: &mut dyn TerminalSurfaceHost, wk: &str, tid: &str, col: u16, row: u16, tx: &mut Tx) {
    let Some((osc8, osc8_visible, plain, folder)) = crate::ops::terminal_tab_mut(host, wk, tid).map(|t| {
        let snap = t.session.snapshot();
        let linked = snap
            .screen_rows
            .get(usize::from(row))
            .and_then(|r| r.cells.get(usize::from(col)))
            .is_some_and(|c| c.flags.contains(CellFlags::HYPERLINK));
        let osc8 = linked.then(|| t.session.hyperlink_at(col, row)).flatten();
        let osc8_visible = osc8.as_ref().map(|_| {
            let cells = snap.screen_rows.get(usize::from(row)).map(|r| r.cells.as_slice()).unwrap_or(&[]);
            let mut a = usize::from(col);
            while a > 0 && cells.get(a - 1).is_some_and(|c| c.flags.contains(CellFlags::HYPERLINK)) {
                a -= 1;
            }
            let mut b = usize::from(col) + 1;
            while cells.get(b).is_some_and(|c| c.flags.contains(CellFlags::HYPERLINK)) {
                b += 1;
            }
            cells[a..b]
                .iter()
                .filter(|c| !c.flags.contains(CellFlags::WIDE_SPACER_TAIL))
                .map(|c| if c.grapheme.is_empty() { " " } else { c.grapheme.as_str() })
                .collect::<String>()
                .trim()
                .to_owned()
        });
        let plain = if osc8.is_none() { at(&snap, col, row) } else { None };
        let folder = shell_folder(&snap, &t.cwd);
        (osc8, osc8_visible, plain, folder)
    }) else {
        return;
    };
    let sid = wk.split_once('\0').map_or("", |(_, s)| s).to_owned();
    let shift = tx.mods().shift;
    if let Some(url) = osc8 {
        let visible = osc8_visible.unwrap_or_default();
        let is_web = url.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("http://"))
            || url.get(..8).is_some_and(|s| s.eq_ignore_ascii_case("https://"));
        let is_file = url.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("file://"));
        if is_file {
            if visible != url {
                crate::osc8::ask(host, crate::osc8::Pending { sid, visible, url, shift }, tx);
            } else {
                host.open_link(&sid, &url, tx);
            }
            return;
        }
        if !is_web || visible != url {
            crate::osc8::ask(host, crate::osc8::Pending { sid, visible, url, shift }, tx);
            return;
        }
        host.open_web_link(&sid, &url, shift, tx);
        return;
    }
    let Some(target) = plain else { return };
    let href = match target {
        Target::Url(url) if !url.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("file://")) => {
            host.open_web_link(&sid, &url, shift, tx);
            return;
        }
        Target::Url(url) => url,
        Target::File { path, line } => {
            let path = path.replace('\\', "/");
            let abs = if opencode_kit::path::is_absolute(&path) || folder.is_empty() {
                path
            } else {
                format!("{}/{}", folder.replace('\\', "/").trim_end_matches('/'), path.trim_start_matches("./"))
            };
            match line {
                Some(n) => format!("{abs}:{n}"),
                None => abs,
            }
        }
    };
    host.open_link(&sid, &href, tx);
}
