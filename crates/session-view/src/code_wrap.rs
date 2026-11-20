//! Wrapping a code block's long lines (`session-ui/src/components/markdown.tsx`'s
//! `createWrapButton`, `markdown.css`, anomalyco/opencode#64545): Settings › Timeline's Wrap code
//! blocks (`general.wrapCodeBlocks`) wraps every block; a block's own Wrap button, shown on hover
//! while a line is wider than the block, overrides it either way. Choosing the default again
//! follows the setting from then on.
//!
//! Markdown draws code blocks without the host's state, so the choices live here for the window's
//! thread, as Desktop keeps each in its block's `data-wrap`. Rows that draw code read
//! [`version`] so they build again when one changes.

use std::cell::RefCell;
use std::collections::HashMap;

use rune::{El, TextStyle, col, rich};

use crate::palette::Pal;

#[derive(Default)]
struct State {
    /// The setting, as the host last said (`set_default`).
    default: bool,
    /// Each block's own choice where it differs from the setting, by block key.
    overrides: HashMap<String, bool>,
    /// Moves on every change, for row memos.
    version: u64,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
}

/// The host's Wrap code blocks setting, read once per timeline build.
pub fn set_default(on: bool) {
    STATE.with_borrow_mut(|s| {
        if s.default != on {
            s.default = on;
            s.version += 1;
        }
    });
}

/// Whether block `key` wraps: its own choice, else the setting.
pub fn wraps(key: &str) -> bool {
    STATE.with_borrow(|s| s.overrides.get(key).copied().unwrap_or(s.default))
}

/// Block `key`'s Wrap button: it switches, and a choice that matches the setting is dropped.
pub fn toggle(key: &str) {
    STATE.with_borrow_mut(|s| {
        let next = !s.overrides.get(key).copied().unwrap_or(s.default);
        if next == s.default {
            s.overrides.remove(key);
        } else {
            s.overrides.insert(key.to_owned(), next);
        }
        s.version += 1;
    });
}

/// Moves whenever any block's wrapping may have changed.
pub fn version() -> u64 {
    STATE.with_borrow(|s| s.version)
}

/// Whether a line of `source` is wider than a block `width` wide, unwrapped (`measureCodeOverflow`):
/// only then does the block offer Wrap.
pub fn overflows(source: &str, font: TextStyle, width: f32) -> bool {
    // Code is monospaced: the line with the most characters is the widest.
    let longest = source.lines().max_by_key(|l| l.chars().map(|c| if c == '\t' { 4 } else { 1 }).sum::<usize>()).unwrap_or("");
    let shown = longest.replace('\t', "    ");
    // Inside the block's 12 padding each side and its 0.5 border.
    !shown.is_empty() && rune::text::measure(&shown, &[], font, None).0 > width - 25.0
}

const SHELLS: [&str; 7] = ["bash", "sh", "shell", "zsh", "fish", "console", "terminal"];
const PLAIN_TEXT: [&str; 4] = ["text", "txt", "plain", "plaintext"];

/// A code block `source` in `language` drawn with its long lines wrapped (`white-space: pre-wrap;
/// overflow-wrap: anywhere`): Markdown's own card, which scrolls sideways, in the same colours.
pub fn card(p: &Pal, source: &str, language: Option<&str>, font: TextStyle) -> El {
    let shell = language.is_some_and(|l| SHELLS.iter().any(|s| l.eq_ignore_ascii_case(s)));
    let plain = language.is_none_or(|l| PLAIN_TEXT.iter().any(|t| l.eq_ignore_ascii_case(t)));
    let (border, ink) = match (shell, plain) {
        (true, _) => (p.border_muted, p.text),
        (false, true) => (p.border, p.muted),
        (false, false) => (p.border, p.syntax.parameter),
    };
    let spans = match language.filter(|l| !shell && rune_kit::syntax::known(l)) {
        Some(l) => {
            let theme = p.syntax.syntax(rune_kit::syntax::SyntaxTheme::from_kit(crate::palette::kit(p)), p.diff_add, p.diff_del);
            theme.spans(&rune_kit::syntax::highlight(Some(l), source), 0)
        }
        None => Vec::new(),
    };
    let body = rich(source.to_owned(), spans)
        .font(font)
        .color(ink)
        .wrap_anywhere()
        .wrap_greedy()
        .w_full()
        .selectable(true)
        .selection_color(p.timeline_selection);
    col().key("card-wrapped").role("code").dir(rune::Direction::Ltr).w_full().radius(6.0).fill(p.code_bg).border(0.5, border).border_layout().px(12.0).py(12.0).child(body)
}
