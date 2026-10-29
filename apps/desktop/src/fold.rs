//! Folding long prompts, as Desktop's "Fold long messages" (`UserMessageFold`): a prompt past
//! 12 lines shows its first 8, its last lines fading out into a Read more / Show less toggle.
//! The text is laid out whole and clipped, so copy and select-all still get all of it.
//!
//! Opening grows the row downward on the timeline's growth spring with its top held where it
//! is, so a timeline pinned to its end doesn't scroll the prompt's start away. Closing a prompt
//! whose top is scrolled above the timeline holds the toggle where it was pressed instead.

use rune::{El, Tx, col, icon, row, text};
use rune_theme_opencode::icons as ic;

use crate::App;
use crate::palette::Pal;
use crate::view::{BUBBLE, BUBBLE_MAX, CONTENT_W, UI_MED};

/// iOS's `UserBubbleFold`, as Desktop: past 12 lines a prompt shows 8, so a fold always hides
/// at least 5 lines and a prompt just past the limit never folds away a line or two.
pub const OVER: usize = 12;
pub const SHOW: usize = 8;
/// The fade at the folded edge (Desktop's mask: the last 40 px).
const FADE: f32 = 40.0;

/// Where a prompt's bubble is laid out, for folding it: the timeline viewport's width, and
/// whether the row's top is scrolled above it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fold {
    pub viewport: f32,
    pub past: bool,
}

/// Where prompts lay out, for folding them (`fold.rs`): the timeline's width, and for an opened
/// prompt whether its top is scrolled above the timeline. Before the timeline's first layout,
/// the widest column: a prompt that's long there is long at any width.
pub fn place<'a>(s: &'a App, session: &str) -> impl Fn(&str) -> Fold + 'a {
    let view = rune::memo::layout_with(&format!("scroll-{session}"), |r| [r[1], r[2]]);
    move |id| {
        let Some([top, w]) = view else { return Fold { viewport: CONTENT_W + 48.0, past: false } };
        let open = s.expanded.contains(&key(id));
        let past = open && rune::memo::layout_with(&format!("u-{id}"), move |r| r[1] < top - 0.5).unwrap_or(false);
        Fold { viewport: w / s.zoom, past }
    }
}

/// The `expanded` key of an opened prompt, Desktop's disclosure key.
pub fn key(id: &str) -> String {
    format!("user-expanded:{id}")
}

/// The width a prompt's words wrap at in a timeline viewport `viewport` points wide: the row is
/// the content column inside the timeline's 24 pt sides, the bubble `min(82%, 64ch)` of it
/// with 12 pt sides. `quotes`: the bubble holds quote cards and is 342 wide.
pub fn text_width(viewport: f32, quotes: bool) -> f32 {
    if quotes {
        return 342.0 - 24.0;
    }
    let row = (viewport - 48.0).clamp(0.0, CONTENT_W);
    (row * 0.82).min(BUBBLE_MAX) - 24.0
}

/// How many lines `text` lays out to at `width`: its hard line breaks at least, and its wrapped
/// height when it could be long.
pub fn lines(text: &str, width: f32) -> usize {
    let hard = text.trim_end().split('\n').count();
    // At 14 pt a line holds 30 characters or more in any bubble; shorter text can't pass 12.
    if hard > OVER || text.len() <= 30 * OVER {
        return hard;
    }
    let (_, h) = rune::text::measure(text, &[], BUBBLE, Some(width.max(1.0)));
    hard.max((h / BUBBLE.line_px()).round() as usize)
}

/// A prompt's words, folded with the toggle under them when they run past [`OVER`] lines.
/// `quotes`: the bubble holds quote cards.
pub fn view(s: &App, p: &Pal, id: &str, shown: &str, words: El, fold: Fold, quotes: bool) -> Vec<El> {
    if !s.settings.fold_user || lines(shown, text_width(fold.viewport, quotes)) <= OVER {
        return vec![words];
    }
    let open = s.expanded.contains(&key(id));
    let mut clip = col().key("fold").child(words);
    if !open {
        clip = clip.max_h(SHOW as f32 * BUBBLE.line_px()).clip_fade(0.0, 0.0, 0.0, FADE);
    }
    vec![clip, toggle(p, id, open, fold.past)]
}

fn toggle(p: &Pal, id: &str, open: bool, past: bool) -> El {
    let (id, label) = (id.to_owned(), if open { "Show less" } else { "Read more" });
    row()
        .key("fold-toggle")
        .role("button")
        .label(label)
        .h(24.0)
        .mt(2.0)
        .gap(2.0)
        .center_y()
        .hover_group()
        .hover_motion(rune::Anim::INSTANT, rune::Anim::INSTANT)
        // The label and the chevron flip in the press's frame.
        .instant_changes()
        .on_press_tx(move |s: &mut App, tx: &mut Tx| flip(s, &id, past, tx))
        .child(text(label).font(UI_MED).color(p.muted).group_color(p.text))
        .child(icon(&ic::CHEVRON_DOWN, 16.0).color(p.icon_muted).rotate(if open { std::f32::consts::PI } else { 0.0 }).rotate_motion(rune::Anim::INSTANT))
}

/// Opens or closes prompt `id`'s fold. The height change rides the growth spring; what holds
/// still is its top, or, closing one whose top is above the timeline, the toggle.
pub fn flip(s: &mut App, id: &str, past: bool, tx: &mut Tx) {
    let k = key(id);
    let opening = s.toggle_expanded(&k);
    let row = format!("u-{id}");
    tx.keep_on_screen(if !opening && past { format!("{row}/fold-toggle") } else { row });
}
