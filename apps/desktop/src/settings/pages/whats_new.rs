//! Settings › What's new (the updater plugin's beta page, `gui-plugins/src/updater/beta-page.tsx`):
//! every change the build carries, New then Fixes, each with its author and pull request, and
//! History on GitHub. Only builds that embed their notes have it, as Desktop's beta builds do:
//! the build sets `OPENCODE_RUNE_WHATS_NEW` to the notes' JSON (`{"history": url, "notes":
//! [{"type": "feat"|"fix"|"perf", "summary", "author", "pr"}]}`).

use rune::{El, TextStyle, Tx, col, el, row, text};
use serde_json::Value;

use super::{catalog, ghost_button, header_with, page_with};
use crate::App;
use crate::palette::Pal;
use opencode_i18n::t;

const SUMMARY: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0).tracking(-0.04);
const META: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0).tracking(-0.04);
const INITIAL: TextStyle = TextStyle::new(8.0, 530.0).lh(10.0);

fn notes() -> Option<Value> {
    serde_json::from_str(option_env!("OPENCODE_RUNE_WHATS_NEW")?).ok()
}

/// Whether this build carries its notes.
pub fn available() -> bool {
    notes().is_some_and(|v| v["notes"].as_array().is_some_and(|n| !n.is_empty()))
}

pub fn view(s: &App, p: &Pal) -> El {
    let v = notes().unwrap_or_default();
    let all = v["notes"].as_array().cloned().unwrap_or_default();
    let history = v["history"].as_str().unwrap_or("https://github.com/anomalyco/rune-desktop/commits/main").to_owned();
    let github = ghost_button(p, "settings-whats-new-history", t!("plugin.updater.beta.github"), None, true, true, move |_, tx: &mut Tx| {
        let _ = tx.open_url(&history);
    });
    let added: Vec<&Value> = all.iter().filter(|n| n["type"] == "feat").collect();
    let fixed: Vec<&Value> = all.iter().filter(|n| n["type"] != "feat").collect();
    let body = col().w_full().gap(32.0)
        .children((!added.is_empty()).then(|| super::section(p, "settings-whats-new-added", Some(t!("plugin.updater.beta.added")), catalog(p, "whats-new-added", added.iter().map(|n| note(p, n)).collect()))))
        .children((!fixed.is_empty()).then(|| super::section(p, "settings-whats-new-fixed", Some(t!("plugin.updater.beta.fixed")), catalog(p, "whats-new-fixed", fixed.iter().map(|n| note(p, n)).collect()))));
    let _ = s;
    page_with(p, "settings-whats-new-page", header_with(p, t!("plugin.updater.beta.title"), t!("plugin.updater.beta.page.description"), Some(github)), body, true)
}

/// A change: its summary over its author's initial and name, then its pull request.
fn note(p: &Pal, n: &Value) -> El {
    let author = n["author"].as_str().unwrap_or_default();
    let pr = n["pr"].as_u64();
    let mut meta = row().gap(6.0).center_y();
    if !author.is_empty() {
        let initial = author.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
        meta = meta.child(row().size(14.0, 14.0).center().radius(7.0).box_shadow(Pal::ring_in(p.border)).child(text(initial).font(INITIAL).color(p.muted)))
            .child(text(author.to_owned()).font(META).color(p.muted));
    }
    if let Some(pr) = pr {
        meta = meta.children((!author.is_empty()).then(|| text("\u{b7}").font(META).color(p.faint))).child(text(format!("#{pr}")).font(META).color(p.muted));
    }
    col().w_full().py(16.0).gap(8.0)
        .child(text(n["summary"].as_str().unwrap_or_default().to_owned()).font(SUMMARY).color(p.text).wrap())
        .children((!author.is_empty() || pr.is_some()).then(|| meta.my(-3.5)))
        .child(el())
}
