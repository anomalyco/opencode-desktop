//! The files a prompt's comments mention, attached as Desktop attaches them
//! (`composer/submit.ts:831-863`, `composer/request.ts:159-169`). A comment's `@word` may be
//! plain text such as `@here`, and the server rejects a whole prompt that attaches a missing
//! file, so only mentions found in their folder's listing go (`GET /api/fs/list`, which reads
//! no file); the rest stay text.

use std::borrow::Cow;
use std::collections::HashSet;

use super::{Server, encode_component, list};
use crate::path::{SEPARATORS, absolute, encode_path};
use crate::prompt::{FileInput, Prompt};

impl Server {
    /// `prompt` with the files its comments mention that exist, as `file://` references
    /// under the session's folder: `directory`, else the one `session` is in. Each goes once,
    /// and not when the prompt already attaches it.
    pub(crate) fn with_mentions<'a>(&self, directory: Option<&str>, session: &str, prompt: &'a Prompt) -> Cow<'a, Prompt> {
        let mentions = crate::quote::mentions(&prompt.comments);
        if mentions.is_empty() || prompt.command.is_some() {
            return Cow::Borrowed(prompt);
        }
        let Some(directory) = directory.map(str::to_owned).or_else(|| self.session_directory(session)) else {
            return Cow::Borrowed(prompt);
        };
        let existing = self.existing(&directory, &mentions);
        let mut used: HashSet<String> = prompt.files.iter().map(|f| f.uri.clone()).collect();
        let found: Vec<FileInput> = mentions
            .iter()
            .filter(|m| existing.contains(*m))
            .filter_map(|m| {
                let uri = format!("file://{}", encode_path(&absolute(&directory, m)));
                used.insert(uri.clone()).then(|| FileInput { uri, name: filename(m).to_owned(), mime: Some("text/plain".into()), mention: None })
            })
            .collect();
        if found.is_empty() {
            return Cow::Borrowed(prompt);
        }
        // Desktop sends `[...files, ...context, ...inline]`: these come after the typed mentions
        // and the file context, before the inline images.
        let mut out = prompt.clone();
        let at = out.files.iter().position(|f| f.uri.starts_with("data:") || f.uri.starts_with("media:")).unwrap_or(out.files.len());
        out.files.splice(at..at, found);
        Cow::Owned(out)
    }

    /// The mentions found in their folder's listing, each folder listed at once as Desktop's
    /// `Promise.all` does.
    fn existing(&self, directory: &str, mentions: &[String]) -> HashSet<String> {
        let unique: HashSet<&String> = mentions.iter().collect();
        // A page has no threads: one listing after another.
        #[cfg(target_arch = "wasm32")]
        return unique.into_iter().filter(|m| self.mention_exists(directory, m)).cloned().collect();
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::scope(|scope| {
            let checks: Vec<_> = unique.into_iter().map(|m| (m, scope.spawn(move || self.mention_exists(directory, m)))).collect();
            checks.into_iter().filter_map(|(m, check)| check.join().unwrap_or(false).then(|| m.clone())).collect()
        })
    }

    /// Whether `mention`'s folder (the session's for a bare name) lists an entry of its name; a
    /// failed listing is a no.
    fn mention_exists(&self, directory: &str, mention: &str) -> bool {
        let target = mention.trim_end_matches(SEPARATORS);
        let mut url = format!("/api/fs/list?location%5Bdirectory%5D={}", encode_component(directory));
        // `getDirectory`: the segments before the last, joined by `/`, with a trailing `/`.
        if let Some((parent, _)) = target.rsplit_once(SEPARATORS) {
            url.push_str(&format!("&path={}", encode_component(&format!("{}/", parent.replace('\\', "/")))));
        }
        let name = filename(target);
        self.get(&url).is_ok_and(|v| list(&v).iter().any(|e| e["path"].as_str().is_some_and(|p| filename(p) == name)))
    }

    /// The folder `session` runs in (`GET /api/session/:id`).
    fn session_directory(&self, session: &str) -> Option<String> {
        let v = self.get(&format!("/api/session/{session}")).ok()?;
        v.get("data").unwrap_or(&v)["location"]["directory"].as_str().map(str::to_owned)
    }
}

/// `getFilename`: the last segment, trailing separators ignored.
fn filename(path: &str) -> &str {
    path.trim_end_matches(SEPARATORS).rsplit(SEPARATORS).next().unwrap_or_default()
}
