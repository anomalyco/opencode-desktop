//! What Ctrl+click (⌘-click on macOS) on terminal output opens: the cell's OSC 8 link, else a web
//! address or a file reference in the line under the pointer, joined across soft wraps.
//!
//! | Under the pointer | Opens |
//! | --- | --- |
//! | An OSC 8 hyperlink | Its URI |
//! | `https://…`, `http://…`, `file://…` (balanced brackets kept, trailing punctuation dropped) | The address |
//! | `src/a.rs:12:5`, `src/a.rs:12`, `src\a.ts(12,5)`, `File "a.py", line 9`, a bare `src/a.rs` | The file in the side viewer at that line, resolved against the shell's folder |

use terminal::{CellFlags, TerminalSnapshot};

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
/// else the folder the terminal started in.
pub fn shell_folder(snap: &TerminalSnapshot, started_in: &str) -> String {
    let Some(rest) = snap.pwd.strip_prefix("file://") else { return started_in.to_owned() };
    let path = rest.find('/').map_or("", |i| &rest[i..]);
    let decoded = percent_decode(path);
    let b = decoded.as_bytes();
    if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' { decoded[1..].to_owned() } else if decoded.is_empty() { started_in.to_owned() } else { decoded }
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

/// Ctrl+click (⌘-click) at (`col`, `row`) of terminal `tid`: opens what [`at`] finds, or the
/// cell's OSC 8 link first.
pub(super) fn follow(app: &mut crate::App, wk: &str, tid: &str, col: u16, row: u16, tx: &mut rune::Tx) {
    let Some(t) = app.terminal_tab_mut(wk, tid) else { return };
    let snap = t.session.snapshot();
    let linked = snap.screen_rows.get(usize::from(row)).and_then(|r| r.cells.get(usize::from(col))).is_some_and(|c| c.flags.contains(CellFlags::HYPERLINK));
    let target = linked.then(|| t.session.hyperlink_at(col, row)).flatten().map(Target::Url).or_else(|| at(&snap, col, row));
    let folder = shell_folder(&snap, &t.cwd);
    let Some(target) = target else { return };
    let sid = wk.split_once('\0').map_or("", |(_, s)| s).to_owned();
    let href = match target {
        Target::Url(url) if !url.starts_with("file://") => {
            if app.live() {
                let _ = tx.open_url(&url);
            }
            return;
        }
        Target::Url(url) => url,
        Target::File { path, line } => {
            let path = path.replace('\\', "/");
            let abs = if opencode_kit::path::is_absolute(&path) || folder.is_empty() { path } else { format!("{}/{}", folder.replace('\\', "/").trim_end_matches('/'), path.trim_start_matches("./")) };
            match line {
                Some(n) => format!("{abs}:{n}"),
                None => abs,
            }
        }
    };
    app.open_link(&sid, None, &href, tx);
}
