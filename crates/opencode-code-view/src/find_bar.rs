//! The find bar over a file or a diff: Pierre's `FileSearchBar`
//! (`session-ui/src/components/file-search.tsx`), a 32 tall panel floating at the viewer's top
//! right (`file-find.ts` `positionBar`: 4 over the scroll view's top, 8 in from its right).

use rune::{El, Shadow, TextEdit, TextStyle, Transition, Tx, col, icon, row, text};
use rune_theme_opencode::icons as ic;
use session_view::palette::{Elevate, Pal};
use session_view::style::INSTANT;

/// `text-14-regular`.
const FIELD: TextStyle = TextStyle::new(14.0, 440.0).lh(25.2);
/// `text-12-regular tabular-nums`.
const COUNT: TextStyle = TextStyle::new(12.0, 440.0).lh(18.0).features("tnum");

/// What the bar does to the view it searches.
pub struct FindBar<S: 'static> {
    pub key: &'static str,
    /// Where its top sits in the view: 4 over the scroll view's top, under a sticky header's.
    pub top: f32,
    pub edit: for<'a, 'b> fn(&'a mut S, &'b str) -> &'a mut TextEdit,
    /// The field's text changed: find it now, in the keystroke's own frame.
    pub changed: fn(&mut S, &str, &mut Tx),
    pub step: fn(&mut S, &str, bool, &mut Tx),
    pub close: fn(&mut S, &str, &mut Tx),
    /// Which view's: handed back to `edit`, `changed`, `step` and `close`.
    pub target: String,
}

/// `--shadow-md`.
fn shadow_md(dark: bool) -> [Shadow; 4] {
    let black = |light: f32, dark_a: f32| rune::Color::BLACK.alpha(if dark { dark_a } else { light });
    [
        Shadow { y: 6.0, blur: 12.0, spread: -2.0, color: black(0.075, 0.1), ..Shadow::default() },
        Shadow { y: 4.0, blur: 8.0, spread: -2.0, color: black(0.075, 0.15), ..Shadow::default() },
        Shadow { y: 1.0, blur: 2.0, color: black(0.1, 0.15), ..Shadow::default() },
        Shadow::default(),
    ]
}

fn find_button_shell(p: &Pal, key: &'static str, label: &'static str, glyph: El, enabled: bool) -> El {
    // `size-6 grid place-items-center rounded`, `disabled:opacity-40`.
    let b = row().key(key).size(24.0, 24.0).center().radius(4.0).role("button").label_static(label).child(glyph);
    if enabled {
        b.hover(p.hover).hover_motion(INSTANT, INSTANT)
    } else {
        b.opacity(0.4).disabled(true)
    }
}

fn find_chevron(p: &Pal, up: bool) -> El {
    let i = icon(&ic::CHEVRON_DOWN, 14.0).color(p.icon_muted);
    if up { i.rotate(std::f32::consts::PI) } else { i }
}

fn find_input_base(p: &Pal, input: &TextEdit, spellcheck: bool) -> El {
    session_view::style::field(input, FIELD)
        .key("find-input")
        .single_line()
        .spellcheck(spellcheck)
        .w(160.0)
        .color(p.text)
        .placeholder(opencode_i18n::t!("ui.fileSearch.placeholder"), p.faint)
        .caret(p.text, p.selection)
        .selection_style(p.field_selection)
        .label(opencode_i18n::t!("ui.fileSearch.placeholder"))
}

#[allow(clippy::too_many_arguments)]
fn find_bar_chrome(p: &Pal, key: &'static str, top: f32, count: usize, index: usize, field: El, prev: El, next: El, close: El) -> El {
    let shown = if count > 0 { format!("{}/{count}", index + 1) } else { "0/0".to_owned() };
    row()
        .key(key)
        .pin(None, Some(top), Some(8.0), None)
        .h(32.0)
        // px-3 inside the 1 pt border, which takes no room of its own here.
        .px(13.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .border(1.0, p.border)
        // bg-background-base: v1's --background-base (theme.css), which the v2 palette lacks.
        .fill(rune::color::hex(if p.dark { "#121212" } else { "#f8f8f8" }).unwrap_or(p.bg_base))
        .elevate(shadow_md(p.dark))
        .transition(Transition::none())
        .child(icon(&ic::MAGNIFYING_GLASS, 14.0).color(p.icon_muted))
        .child(field)
        // `width: 10ch`, right-aligned.
        .child(row().key("find-count").w(82.77).justify_end().child(text(shown).font(COUNT).color(p.faint)))
        .child(row().key("find-steps").child(prev).child(next))
        .child(close)
}

/// The bar, pinned in the view it searches: `count` matches with `index` lit.
pub fn find_bar<S: 'static>(p: &Pal, input: &TextEdit, count: usize, index: usize, spellcheck: bool, bar: FindBar<S>) -> El {
    type PressFn<S> = Box<dyn Fn(&mut S, &mut Tx)>;
    let FindBar { key, top, edit, changed, step, close, target } = bar;
    let button = |key: &'static str, label: &'static str, glyph: El, enabled: bool, press: PressFn<S>| {
        let b = find_button_shell(p, key, label, glyph, enabled);
        if enabled {
            b.on_press_tx(move |s: &mut S, tx: &mut Tx| press(s, tx))
        } else {
            b
        }
    };
    let (t_edit, t_prev, t_next, t_close, t_submit, t_changed) = (target.clone(), target.clone(), target.clone(), target.clone(), target.clone(), target);
    let t_key = t_close.clone();
    let field = find_input_base(p, input, spellcheck)
        .on_edit(move |s: &mut S| edit(s, &t_edit))
        .on_input_change(move |s: &mut S, tx: &mut Tx| changed(s, &t_changed, tx))
        // The editor receives Escape before a pane shortcut. Unless an IME is using
        // it, dismiss the owning bar instead of merely blurring its inner field.
        .on_input_key(move |s: &mut S, ev, tx| {
            if !matches!(ev.key, rune::Key::Enter | rune::Key::Escape) || edit(s, &t_key).is_composing() {
                return false;
            }
            if ev.key == rune::Key::Escape {
                close(s, &t_key, tx);
            } else {
                step(s, &t_key, !ev.mods.shift, tx);
            }
            true
        })
        .on_submit(move |s: &mut S, tx: &mut Tx| step(s, &t_submit, true, tx));
    let prev = button("find-prev", opencode_i18n::t!("ui.fileSearch.previousMatch"), find_chevron(p, true), count > 0, Box::new(move |s, tx| step(s, &t_prev, false, tx)));
    let next = button("find-next", opencode_i18n::t!("ui.fileSearch.nextMatch"), find_chevron(p, false), count > 0, Box::new(move |s, tx| step(s, &t_next, true, tx)));
    let close_btn = button("find-close", opencode_i18n::t!("ui.fileSearch.close"), icon(&ic::XMARK_SMALL, 14.0).color(p.icon_muted), true, Box::new(move |s, tx| close(s, &t_close, tx)));
    find_bar_chrome(p, key, top, count, index, field, prev, next, close_btn)
}

/// `content` with the find bar over its top right.
pub fn over(content: El, bar: Option<El>) -> El {
    match bar {
        Some(bar) => col().key("find-host").w_full().grow(1.0).min_h(0.0).child(content).child(bar),
        None => content,
    }
}
