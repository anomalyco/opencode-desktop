//! Inline code in a previewed Markdown file that names a file there becomes a link to it, as
//! Desktop's Markdown marks it (`session-ui/src/components/markdown.tsx` `markInlineCode` and
//! `inlineCodeKind`, checked by `gui-plugins/src/file/link-exists-machine.ts`): code that reads as
//! a path is checked once, by listing its folder as Desktop's `files.exists` does, and shows as a
//! link while the answer is yes.

use std::collections::HashMap;

use super::links::{Link, classify};
use crate::App;

/// What is known about one path (`LinkExistsEntry`, without its expiry: a document's checks go
/// with it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PathCheck {
    Checking,
    Settled { exists: bool },
}

/// One document's checks, by the code's text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CodePaths {
    checks: HashMap<String, PathCheck>,
    /// Moves when a check settles, so the preview draws again.
    pub rev: u64,
}

impl CodePaths {
    /// The code naming files that exist.
    pub fn found(&self) -> impl Iterator<Item = &str> {
        self.checks.iter().filter(|(_, c)| **c == PathCheck::Settled { exists: true }).map(|(k, _)| k.as_str())
    }
}

/// Inline code that reads as a path (`inlineCodeKind` `path`): no spaces, scheme, code or glob
/// characters, nor a network share; a folder separator or a dot beside a letter, or a file
/// name that has none (`Makefile`).
pub fn reads_as_path(text: &str) -> bool {
    const NAMES: &[&str] = &["Makefile", "Dockerfile", "Gemfile", "Procfile", "Rakefile", "Justfile", "LICENSE", "README", "CHANGELOG"];
    if text.is_empty() || text.contains(char::is_whitespace) || text.contains("://") {
        return false;
    }
    if text.contains(['{', '}', '<', '>', '"', '\'', '`', ';', '=', '|', '&', '*', ',']) || text.starts_with("//") || text.starts_with("\\\\") {
        return false;
    }
    let path = strip_line_suffix(text);
    NAMES.contains(&path) || (path.contains(['.', '/', '\\']) && path.contains(|c: char| c.is_ascii_alphabetic()))
}

/// `path:12`, `path:12-20`, `path#L12`: the path alone (`parsePathLineSuffix`).
fn strip_line_suffix(text: &str) -> &str {
    let text = text.split_once("#L").map_or(text, |(p, _)| p);
    match text.split_once(':') {
        Some((p, rest)) if !p.is_empty() && rest.chars().next().is_some_and(|c| c.is_ascii_digit()) => p,
        _ => text,
    }
}

/// The single-backtick code spans of Markdown `src` outside fenced blocks that read as paths.
fn candidates(src: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut fenced = false;
    for line in src.lines() {
        if line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let mut parts = line.split('`');
        parts.next();
        while let (Some(code), Some(_)) = (parts.next(), parts.next()) {
            let code = code.trim();
            if reads_as_path(code) && !out.iter().any(|c| c == code) {
                out.push(code.to_owned());
            }
        }
    }
    out
}

impl App {
    /// Checks the paths Markdown file `path`'s inline code names, once each.
    pub(super) fn check_code_paths(&mut self, sid: &str, path: &str) {
        let Some(ws) = self.files.sessions.get(sid) else { return };
        let Some(doc) = ws.docs.get(path).filter(|d| d.ready() && !d.binary()) else { return };
        if !super::markdown::is_markdown(path) {
            return;
        }
        let known = ws.code_paths.get(path);
        let todo: Vec<String> = candidates(doc.text()).into_iter().filter(|c| known.is_none_or(|k| !k.checks.contains_key(c))).collect();
        if todo.is_empty() {
            return;
        }
        let (dir, base) = (self.link_directory(sid), opencode_kit::path::parent(path).to_owned());
        let live = matches!(self.source, crate::Source::Live { .. });
        for code in todo {
            let target = match classify(&code, &base, &dir, &self.home) {
                Link::Inside { path, .. } => Some(path),
                Link::Web(_) | Link::Heading(_) | Link::Outside { .. } => None,
            };
            let ws = self.files.sessions.entry(sid.to_owned()).or_default();
            let checks = ws.code_paths.entry(path.to_owned()).or_default();
            let Some(target) = target else {
                checks.checks.insert(code, PathCheck::Settled { exists: false });
                continue;
            };
            if !live {
                let exists = ws.known_files.contains(&target);
                checks.checks.insert(code, PathCheck::Settled { exists });
                checks.rev += 1;
                continue;
            }
            checks.checks.insert(code.clone(), PathCheck::Checking);
            let (sid, doc, dir) = (sid.to_owned(), path.to_owned(), dir.clone());
            // Lists the folder instead of reading the file, so a check moves no content.
            self.job(move |srv| {
                let parent = opencode_kit::path::parent(&target).to_owned();
                let exists = super::data::fetch_live_dir(srv, &dir, &parent).iter().any(|(p, is_dir)| !is_dir && *p == target);
                Box::new(move |s: &mut App, tx: &mut rune::Tx| {
                    let Some(checks) = s.files.sessions.get_mut(&sid).and_then(|ws| ws.code_paths.get_mut(&doc)) else { return };
                    if checks.checks.get(&code) == Some(&PathCheck::Checking) {
                        checks.checks.insert(code, PathCheck::Settled { exists });
                        checks.rev += 1;
                        tx.instant();
                    }
                }) as crate::jobs::Apply
            });
        }
    }
}
