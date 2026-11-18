//! Pure workspace file path normalization, tab ID helpers, `path:line` / `path#Lline` parsing,
//! case-insensitive file query ranking, and review path filtering/sorting.

use std::cmp::Ordering;
use std::collections::HashMap;

pub const OPEN_TAB_ID: &str = "open";
pub const OPEN_TAB_KEY: &str = "file:open";

/// Normalizes a workspace-relative path: backslashes become `/`, leading/trailing slashes are
/// stripped, and repeated `/` runs collapse to a single `/`.
pub fn normalize_path(value: &str) -> String {
    let replaced = value.replace('\\', "/");
    let trimmed = replaced.trim_matches('/');
    let mut out = String::with_capacity(trimmed.len());
    let mut prev_slash = false;
    for ch in trimmed.chars() {
        if ch == '/' {
            if !prev_slash {
                out.push('/');
            }
            prev_slash = true;
        } else {
            out.push(ch);
            prev_slash = false;
        }
    }
    out
}

pub fn file_tab_id(path: &str) -> String {
    format!("//{}", normalize_path(path))
}

pub fn file_tab_key(path: &str) -> String {
    format!("file:{}", file_tab_id(path))
}

pub fn file_tab_path(tab_id: &str) -> Option<String> {
    let rest = tab_id.strip_prefix("//")?;
    let norm = normalize_path(rest);
    (!norm.is_empty()).then_some(norm)
}

/// Splits `"src/net/stream.ts:24"` or `"src/net/stream.ts#L24"` into `("src/net/stream.ts", Some(24))`.
pub fn parse_path_and_line(raw: &str) -> (String, Option<u32>) {
    let trimmed = raw.trim();
    if let Some((base, frag)) = trimmed.rsplit_once("#L") {
        let digits: String = frag.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(line) = digits.parse::<u32>() {
            return (base.trim().to_owned(), Some(line.max(1)));
        }
    }
    if let Some((base, tail)) = trimmed.rsplit_once(':')
        && !base.is_empty()
        && let Ok(line) = tail.split(':').next().unwrap_or("").parse::<u32>()
    {
        return (base.trim().to_owned(), Some(line.max(1)));
    }
    (trimmed.to_owned(), None)
}

/// Case-insensitive fuzzy/substring scorer for `⌘P` and sidebar file search, using
/// [`compare_names_natural`] for score tie-breaks.
pub fn rank_file_query(paths: &[String], query: &str) -> Vec<String> {
    rank_file_query_with(paths, query, compare_names_natural)
}

/// [`rank_file_query`] with a caller-supplied tie-breaker (for example OS locale collation).
pub fn rank_file_query_with(
    paths: &[String],
    query: &str,
    mut tie_break: impl FnMut(&str, &str) -> Ordering,
) -> Vec<String> {
    let (clean, _) = parse_path_and_line(query);
    let needle = clean.trim().to_lowercase();
    if needle.is_empty() {
        return paths.to_vec();
    }
    let mut scored: Vec<(i32, &String)> = paths
        .iter()
        .filter_map(|p| score_path(p, &needle).map(|sc| (sc, p)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| tie_break(a.1, b.1)));
    scored.into_iter().map(|(_, p)| p.clone()).collect()
}

/// Scores `path` against a pre-lowercased `needle_lower`: exact basename match > basename prefix >
/// basename substring > full-path substring > ordered subsequence across path characters.
pub fn score_path(path: &str, needle_lower: &str) -> Option<i32> {
    let hay = path.to_lowercase();
    let base = basename(&hay);
    if base == needle_lower {
        return Some(1000 - path.len() as i32);
    }
    if base.starts_with(needle_lower) {
        return Some(850 - path.len() as i32);
    }
    if base.contains(needle_lower) {
        return Some(700 - path.len() as i32);
    }
    if hay.contains(needle_lower) {
        return Some(500 - path.len() as i32);
    }
    // Ordered subsequence match across path characters.
    let mut chars = hay.chars();
    for nc in needle_lower.chars() {
        chars.find(|&hc| hc == nc)?;
    }
    Some(200 - path.len() as i32)
}

/// Case-insensitive ASCII substring filter over review file paths.
pub fn filter_review_files(paths: &[String], query: &str) -> Vec<String> {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return paths.to_vec();
    }
    paths
        .iter()
        .filter(|p| p.to_ascii_lowercase().contains(&q))
        .cloned()
        .collect()
}

/// Depth-first file order with directories before sibling files (`review/kinds.ts::sortReviewPaths`),
/// using [`compare_names_natural`] to order sibling entries.
pub fn sort_review_paths(paths: &[String]) -> Vec<String> {
    sort_review_paths_with(paths, compare_names_natural)
}

/// [`sort_review_paths`] with a caller-supplied sibling name comparator.
pub fn sort_review_paths_with(
    paths: &[String],
    mut compare: impl FnMut(&str, &str) -> Ordering,
) -> Vec<String> {
    #[derive(Clone)]
    struct Node {
        name: String,
        path: String,
        is_file: bool,
        original: String,
    }
    let mut nodes: HashMap<String, Node> = HashMap::new();
    for raw in paths {
        let norm = normalize_path(raw);
        if norm.is_empty() {
            continue;
        }
        let parts: Vec<&str> = norm.split('/').collect();
        for (idx, name) in parts.iter().enumerate() {
            let p = parts[..=idx].join("/");
            if nodes.contains_key(&p) {
                continue;
            }
            let leaf = idx + 1 == parts.len();
            nodes.insert(
                p.clone(),
                Node {
                    name: (*name).to_owned(),
                    path: p.clone(),
                    is_file: leaf,
                    original: if leaf { raw.clone() } else { p },
                },
            );
        }
    }
    let mut children: HashMap<String, Vec<Node>> = HashMap::new();
    for node in nodes.into_values() {
        let parent = match node.path.rfind('/') {
            Some(i) => node.path[..i].to_owned(),
            None => String::new(),
        };
        children.entry(parent).or_default().push(node);
    }
    for list in children.values_mut() {
        list.sort_by(|a, b| match (a.is_file, b.is_file) {
            (false, true) => Ordering::Less,
            (true, false) => Ordering::Greater,
            _ => compare(&a.name, &b.name),
        });
    }
    let mut out = Vec::with_capacity(paths.len());
    let mut stack: Vec<Node> = children
        .remove("")
        .unwrap_or_default()
        .into_iter()
        .rev()
        .collect();
    while let Some(node) = stack.pop() {
        if node.is_file {
            out.push(node.original);
        } else if let Some(kids) = children.get(&node.path) {
            stack.extend(kids.iter().rev().cloned());
        }
    }
    out
}

/// Natural name comparison: case-folded characters with ASCII digit runs compared by numeric value,
/// falling back to raw code-point order so ties are total and stable across platforms.
pub fn compare_names_natural(a: &str, b: &str) -> Ordering {
    natural(a, b).then_with(|| a.cmp(b))
}

fn natural(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let run = |it: &mut std::iter::Peekable<std::str::Chars<'_>>| {
                    let mut s = String::new();
                    while let Some(ch) = it.peek().copied().filter(char::is_ascii_digit) {
                        s.push(ch);
                        it.next();
                    }
                    s.trim_start_matches('0').to_owned()
                };
                let (m, n) = (run(&mut x), run(&mut y));
                let by_value = m.len().cmp(&n.len()).then_with(|| m.cmp(&n));
                if by_value != Ordering::Equal {
                    return by_value;
                }
            }
            (Some(c), Some(d)) => {
                let by_char = c.to_lowercase().cmp(d.to_lowercase());
                if by_char != Ordering::Equal {
                    return by_char;
                }
                x.next();
                y.next();
            }
        }
    }
}

const SEPARATORS: [char; 2] = ['/', '\\'];

/// Last path segment across POSIX, Windows drive, and UNC paths (matches `opencode_kit::path::basename`).
fn basename(path: &str) -> &str {
    let path = trim_path(path);
    let root = root_len(path);
    if path.len() <= root {
        return if is_unc(path) {
            path[2..].rsplit(SEPARATORS).next().unwrap_or(path)
        } else {
            path
        };
    }
    path[root..].rsplit(SEPARATORS).next().unwrap_or(path)
}

fn trim_path(path: &str) -> &str {
    let trimmed = path.trim_end_matches(SEPARATORS);
    let root = root_len(path);
    if trimmed.len() < root {
        &path[..root]
    } else {
        trimmed
    }
}

fn is_drive(path: &str) -> bool {
    let b = path.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

fn is_unc(path: &str) -> bool {
    path.len() > 2
        && (path.starts_with("\\\\") || path.starts_with("//"))
        && !path[2..].starts_with(SEPARATORS)
}

fn root_len(path: &str) -> usize {
    if is_drive(path) {
        return if path[2..].starts_with(SEPARATORS) {
            3
        } else {
            2
        };
    }
    if is_unc(path) {
        let rest = &path[2..];
        let server = rest.find(SEPARATORS).unwrap_or(rest.len());
        let after = &rest[server..];
        let share = after.trim_start_matches(SEPARATORS);
        let skipped = after.len() - share.len();
        let end = share.find(SEPARATORS).unwrap_or(share.len());
        return 2 + server + skipped + end;
    }
    usize::from(path.starts_with(SEPARATORS))
}
