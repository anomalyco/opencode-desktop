//! Shared OpenCode session composer (`p254`): OpenCode (Rune)'s auto-growing editor (`180pt` cap
//! with internal edge-faded scroll), attachment chips & image paste, prompt history (`history`),
//! `+` attach button, and instant Send / Stop button with `stop_glyph_swap()`.

pub mod attachments;
pub mod history;

use std::cell::Cell;

use opencode_kit::client::PathAttachment;
use opencode_kit::model::Attachment;
use rune::{
    Color, El, Key, KeyEvent, MotionIntent, ScrollSpec, Shadow, Spring, SpringPair, TextEdit, TextStyle,
    Transition, Tx, col, el, icon, row, spacer, text, text_input,
};
use rune_theme_opencode::icons as ic;

use crate::TimelineHost;
use crate::palette::{Elevate as _, Pal};
use crate::style::{DOCK_SIDE, UI};
use crate::user_parts::mention_pill_style;

/// The editor's height cap, its padding included (Desktop's `max-h-[180px]`).
pub const COMPOSER_MAX_H: f32 = 180.0;
pub const PROMPT: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0);
pub const SHELL_PROMPT: TextStyle = TextStyle::new(13.0, 400.0).mono().lh(20.0);

thread_local! {
    static WHOLE: Cell<bool> = const { Cell::new(true) };
}

/// Whether the composer showed its whole prompt when last laid out: a longer one scrolls inside
/// the editor, its words starting out of sight.
pub fn shows_whole_prompt() -> bool {
    WHOLE.get()
}

/// Where sent prompt `local` shows up. When the timeline shows its end, it rises out of the
/// composer, its words where they were typed. Read further up, nothing flies: the timeline cuts
/// to its end, where the message already stands. A prompt longer than the composer, whose words
/// start out of sight in it, arrives where it lands instead of flying.
pub fn land<S: TimelineHost>(s: &S, session: &str, local: &str, from: &str, tx: &mut Tx) {
    if s.ends().at_end(session) {
        if shows_whole_prompt() {
            tx.launch(from, format!("u-{local}/bubble"));
        }
    } else {
        tx.scroll_to_bottom_instant(format!("scroll-{session}"));
    }
}

/// Renders the shared composer in its bottom dock (`12pt` side and bottom inset, matching
/// `jump-lane` so the timeline runs right down to its top edge).
pub fn dock<S: ComposerHost>(s: &S, p: &Pal) -> El {
    col()
        .key("dock")
        .w_full()
        .center()
        .px(DOCK_SIDE)
        .pb(DOCK_SIDE)
        .child(composer(s, p))
}

pub fn composer_fill(p: &Pal) -> Color {
    if p.dark { p.layer1 } else { p.bg_base }
}

/// The two glyphs of the submit button changing places: the leaving one shrinks away as the
/// arriving one grows in, quickly and without bounce.
pub fn stop_glyph_swap() -> Transition {
    Transition::scale(0.5)
        .allow_small_scale()
        .springs(SpringPair::new(Spring::new(0.2, 0.0), Spring::new(0.14, 0.0)))
}

/// Whether `ev` is `Esc` or `⌃G` without extra modifiers (stops a running session in the composer).
pub fn is_stop_shortcut(ev: &KeyEvent) -> bool {
    let m = ev.mods;
    let esc = ev.key == Key::Escape && !m.cmd && !m.ctrl && !m.alt && !m.shift;
    esc || crate::keymap::Bind::Stop.matches(ev)
}

/// Ghost toolbar button shared by Desktop's agent/model/effort pickers.
pub fn ghost(p: &Pal, key: &str, label: &str, chevron: bool, color: Color) -> El {
    ghost_with(p, key, text(label.to_owned()).font(UI).color(color).truncate_fade(), chevron, false)
}

/// [`ghost`] around any label: `button-v2` ghost-muted at size normal (`button.css:41-44,
/// 247-283`), 28 tall with 6 pt corners and 11 in from its start. The chevron
/// (`editor.tsx:1108`, `-ms-0.5 -me-1` on the 6 pt gap) sits 4 after the label and 7 in
/// from the end, in `icon-icon-muted`, or `icon-icon-base` while its menu is `open`.
pub fn ghost_with(p: &Pal, key: &str, label: El, chevron: bool, open: bool) -> El {
    row()
        .key(key)
        .role("button")
        .h(28.0)
        .pl(11.0)
        .pr(if chevron { 7.0 } else { 11.0 })
        .gap(4.0)
        .center_y()
        .radius(6.0)
        .hover(p.hover)
        .press(p.pressed)
        .child(label)
        .children(chevron.then(|| icon(&ic::CHEVRON_DOWN, 16.0).key("chevron").color(if open { p.icon } else { p.icon_muted })))
}

/// The `/` between a provider and its model (`slash.css`): the neighbouring text's size in
/// its regular weight, `icon-icon-faint`, 6 pt of space to its ink on each side.
pub fn slash(p: &Pal, gap: f32) -> El {
    // The margins less the glyph's side bearings (`margin-inline: -0.25px -0.34px`).
    text("/").font(SLASH).color(p.icon_faint).ml((gap - 0.5).max(0.0) - 0.25).mr((gap - 0.5).max(0.0) - 0.34)
}

const SLASH: TextStyle = TextStyle::new(13.0, 400.0).lh(16.0).tracking(-0.04);

/// Default `+` attach button at the start of the toolbar (`⌘U`). The pickers start 4 after it
/// (`ms-1`), the toolbar's 2 pt gap and its own 2.
pub fn attach_button<S: ComposerHost>(p: &Pal) -> El {
    el()
        .key("add")
        .role("button")
        .label(opencode_i18n::t!("ui.promptInput.add"))
        .help(format!("{} {}", opencode_i18n::t!("ui.promptInput.add"), crate::keymap::Bind::Attach.label()))
        .size(28.0, 28.0)
        .mr(2.0)
        .radius(6.0)
        .center()
        .hover(p.hover)
        .press(p.pressed)
        .on_press_tx(S::composer_pick_attachment)
        .child(icon(&ic::PLUS, 16.0).color(p.icon_muted))
}

/// Per-window composer state (`ViewState::composer`) for hosts that manage multiple sessions by ID.
#[derive(Clone, Debug, Default)]
pub struct ComposerState {
    pub files: std::collections::HashMap<String, Vec<Attachment>>,
    pub paths: std::collections::HashMap<String, Vec<PathAttachment>>,
    pub uploading: std::collections::HashMap<String, Vec<String>>,
    pub history: history::PromptHistory<String>,
    pub stopped: std::collections::HashSet<String>,
}

impl ComposerState {
    pub fn files(&self, session: &str) -> &[Attachment] {
        self.files.get(session).map_or(&[], Vec::as_slice)
    }

    pub fn paths(&self, session: &str) -> &[PathAttachment] {
        self.paths.get(session).map_or(&[], Vec::as_slice)
    }

    pub fn uploading(&self, session: &str) -> &[String] {
        self.uploading.get(session).map_or(&[], Vec::as_slice)
    }

    pub fn add_file(&mut self, session: &str, a: Attachment) {
        attachments::push_unique(self.files.entry(session.to_owned()).or_default(), a);
    }

    pub fn remove_file(&mut self, session: &str, id: &str) {
        if let Some(list) = self.files.get_mut(session) {
            list.retain(|f| crate::images::identity(f) != id);
        }
    }

    pub fn add_path(&mut self, session: &str, p: PathAttachment) {
        let list = self.paths.entry(session.to_owned()).or_default();
        if !list.iter().any(|x| x.path == p.path) {
            list.push(p);
        }
    }

    pub fn remove_path(&mut self, session: &str, path: &str) {
        if let Some(list) = self.paths.get_mut(session) {
            list.retain(|f| f.path != path);
        }
    }

    pub fn take(&mut self, session: &str) -> (Vec<Attachment>, Vec<PathAttachment>) {
        (
            self.files.remove(session).unwrap_or_default(),
            self.paths.remove(session).unwrap_or_default(),
        )
    }

    pub fn restore(&mut self, session: &str, files: Vec<Attachment>, paths: Vec<PathAttachment>) {
        for a in files {
            self.add_file(session, a);
        }
        for p in paths {
            self.add_path(session, p);
        }
    }
}

/// Host hooks for the shared session composer. Defaults give a minimal host (like Seer) auto-grow,
/// attachment chips, image paste, `+` attach button, and Send / Stop with ~20 lines of glue,
/// while letting OpenCode (Rune) plug in its pickers, slash/mention menus, queue face, and dictation.
pub trait ComposerHost: TimelineHost {
    fn composer_edit(&self) -> &TextEdit;
    fn composer_edit_mut(&mut self) -> &mut TextEdit;

    fn composer_busy(&self) -> bool {
        self.active_session_id().is_some_and(|id| self.is_session_live(id))
    }
    fn composer_send(&mut self, tx: &mut Tx);
    fn composer_stop(&mut self, tx: &mut Tx);

    fn composer_attachments(&self) -> &[Attachment] {
        &[]
    }
    fn composer_remove_attachment(&mut self, _identity: &str) {}
    fn composer_paste_attachment(&mut self, _attachment: Attachment) -> bool {
        false
    }
    fn composer_pick_attachment(&mut self, _tx: &mut Tx) {}

    fn composer_path_attachments(&self) -> &[PathAttachment] {
        &[]
    }
    fn composer_remove_path_attachment(&mut self, _path: &str) {}
    fn composer_uploading(&self) -> &[String] {
        &[]
    }
    fn composer_quote_chips(&self, _p: &Pal) -> Vec<El> {
        Vec::new()
    }
    fn composer_has_extra_payload(&self) -> bool {
        false
    }

    fn composer_placeholder(&self) -> String {
        "Ask anything…".into()
    }
    fn composer_shell_mode(&self) -> bool {
        false
    }
    /// The app's spell checking setting (Desktop's `spellcheck`): the prompt marks misspelled
    /// words while it is on, outside shell mode (`editor.tsx:318`).
    fn composer_spellcheck(&self) -> bool {
        false
    }
    fn composer_dictating(&self) -> bool {
        false
    }
    fn composer_docked_top(&self) -> bool {
        false
    }
    fn composer_face(&self, _p: &Pal) -> Option<Vec<El>> {
        None
    }
    fn composer_face_transition(&self) -> Transition {
        Transition::none()
    }
    /// Whether the face draws its own surfaces (the Classic question's raised body over its
    /// footer tray), so the composer's card steps back while it shows.
    fn composer_face_bare(&self) -> bool {
        false
    }
    fn composer_toolbar_items(&self, p: &Pal, _scope: &str, _queues: bool) -> Vec<El> {
        vec![
            attach_button::<Self>(p),
            spacer().key("room").h(28.0).min_w(0.0),
        ]
    }
    fn composer_below_toolbar(&self, _p: &Pal) -> Option<El> {
        None
    }
    fn composer_on_change(&mut self, _tx: &mut Tx) {}
    fn composer_on_key(&mut self, _ev: &KeyEvent, _tx: &mut Tx) -> bool {
        false
    }
}

/// Whether the composer holds something to send: words (whitespace doesn't count), or files,
/// paths and quotes outside shell mode.
pub fn ready<S: ComposerHost>(s: &S) -> bool {
    let has_extra = !s.composer_attachments().is_empty()
        || !s.composer_path_attachments().is_empty()
        || s.composer_has_extra_payload();
    !s.composer_edit().text.trim().is_empty() || (has_extra && !s.composer_shell_mode())
}

/// Whether Send shows as Stop: the open session is busy and there's nothing to send.
pub fn stopping<S: ComposerHost>(s: &S) -> bool {
    s.composer_busy() && !ready(s) && !s.composer_dictating()
}

/// ↵ (⌘↵ too): sends, or stops the run when Send shows as Stop, as Desktop does. A held ↵'s
/// repeats never stop it.
pub fn submit_or_stop<S: ComposerHost>(s: &mut S, tx: &mut Tx) {
    if !stopping(s) {
        return s.composer_send(tx);
    }
    if !tx.key_repeat() {
        s.composer_stop(tx);
    }
}

/// Desktop's composer submit (`editor.tsx:1483-1497`): `icon-button-v2` in the `submit`
/// variant at `size-7 rounded-md`, as `submit.css` paints it. `bg-icon-button-contrast` under
/// the top-lit `--submit-highlight` (white at 20%, 60% on dark's light fill), ringed and
/// lifted by `--v2-elevation-button-contrast` (`theme.css:358-361,507-510`); hover and press
/// lay `--submit-hover` and `overlay-contrast-pressed` over it. Without anything to send it
/// is disabled: the same button at 40% (`submit.css:74-80`), with no hover.
pub fn submit_button(p: &Pal, live: bool) -> El {
    let shadow = |y: f32, blur: f32, spread: f32, color: Color, inset: bool| Shadow { y, blur, spread, color, inset, ..Shadow::default() };
    let (white, black) = (Color::WHITE, Color::BLACK);
    let elevation = if p.dark {
        vec![
            shadow(1.0, 2.0, 0.0, black.alpha(0.4), false),
            shadow(0.0, 0.0, 0.5, p.q.submit_ring, false),
            shadow(-0.5, 0.0, 0.0, white.alpha(0.3), false),
        ]
    } else {
        vec![
            shadow(1.0, 1.5, 0.0, black.alpha(0.2), false),
            shadow(0.0, 0.0, 0.5, p.q.submit_ring, false),
            shadow(1.0, 2.0, 0.0, white.alpha(0.14), true),
            shadow(-1.0, 2.0, 0.0, black.alpha(0.06), true),
        ]
    };
    let b = elevation
        .into_iter()
        .fold(el().key("submit").role("button").size(28.0, 28.0).radius(6.0).center().fill(p.q.submit), El::box_shadow)
        .gradient(white.alpha(if p.dark { 0.6 } else { 0.2 }), white.alpha(0.0))
        .motion(Spring::NONE)
        .motion_intent(MotionIntent::CUT);
    if !live {
        return b.opacity(0.4);
    }
    let (hover, pressed) = if p.dark { (black.alpha(0.08), black.alpha(0.4)) } else { (white.alpha(0.12), white.alpha(0.24)) };
    b.hover(hover).press(pressed)
}

/// Renders the shared composer (`key("composer")`).
pub fn composer<S: ComposerHost>(s: &S, p: &Pal) -> El {
    composer_in(s, p, "")
}

/// Renders the shared composer, keyed `{scope}-composer` when `scope` isn't empty.
pub fn composer_in<S: ComposerHost>(s: &S, p: &Pal, scope: &str) -> El {
    let (pad, shown) = (
        rune::memo::layout_with("editor-pad", |r| r[3]),
        rune::memo::layout_with("editor-scroll", |r| r[3]),
    );
    WHOLE.set(!matches!((pad, shown), (Some(pad), Some(shown)) if pad > shown + 0.5));
    let ready = ready(s);
    let busy = s.composer_busy();
    let stop = stopping(s);
    let glyph = match (stop, s.composer_shell_mode()) {
        (true, _) => icon(&ic::STOP, 16.0).key("stop-glyph"),
        (false, true) => icon(&ic::ARROW_UNDO_DOWN, 16.0).key("send-glyph"),
        (false, false) => icon(&ic::ARROW_UP, 16.0).key("send-glyph"),
    };
    // Send finishes a dictation and sends its words, so it stays live while one runs.
    let submit = submit_button(p, ready || stop || s.composer_dictating())
        .label(if stop { opencode_i18n::t!("ui.promptInput.stop") } else { opencode_i18n::t!("ui.promptInput.send") })
        .on_press_tx(move |s: &mut S, tx: &mut Tx| {
            if stop {
                s.composer_stop(tx);
            } else {
                s.composer_send(tx);
            }
        })
        .child(glyph.color(p.q.submit_text).transition(stop_glyph_swap()));
    let key = if scope.is_empty() {
        "composer".to_owned()
    } else {
        format!("{scope}-composer")
    };
    let face = s.composer_face(p);
    // A hover group: the default effort shows only while the pointer is over the composer
    // (`editor.css:75-80`), at once both ways.
    let box_el = col()
        .key(key)
        .w_full()
        .max_w(crate::style::dock_w())
        .min_h(88.0)
        .corners(if s.composer_docked_top() { [0.0, 0.0, 12.0, 12.0] } else { [12.0; 4] })
        .hover_group()
        .hover_motion(rune::Anim::INSTANT, rune::Anim::INSTANT);
    // A face with surfaces of its own keeps its shadows: no card under it and no clip over it.
    // Otherwise `--v2-elevation-raised` (`editor.tsx:248-251`): two drops, a 0.5 pt ring and
    // dark's lit top edge.
    let box_el = if face.is_some() && s.composer_face_bare() {
        box_el
    } else {
        box_el.fill(composer_fill(p)).elevate(p.raised).clip_soft(0.0)
    };
    match face {
        Some(face) => box_el.children(face),
        None => box_el
            .children(attachments::chips(s, p))
            .child(
                col()
                    .key("editor")
                    .transition(s.composer_face_transition())
                    .w_full()
                    .shrink()
                    .pb(8.0)
                    .child(
                        col()
                            .key("editor-scroll")
                            .w_full()
                            .shrink()
                            .max_h(COMPOSER_MAX_H - 8.0)
                            .scroll(ScrollSpec { stick_bottom: false })
                            // `editor.css:9-20`: a fixed 16 fade over the top padding, and the
                            // bottom feathering by as much as is left to scroll, up to 16.
                            .clip_fade_linear(0.0, 16.0, 0.0, 0.0)
                            .scroll_fade(rune::ScrollFade::feather(16.0).end_only())
                            .scroll_margin(16.0)
                            .child(
                                // `ps-4 pt-4`, and 12 at the end (`editor.css:29-34`).
                                col().key("editor-pad").w_full().pl(16.0).pr(12.0).pt(16.0).child(
                                    text_input(
                                        s.composer_edit(),
                                        if s.composer_shell_mode() { SHELL_PROMPT } else { PROMPT },
                                    )
                                    .key("prompt")
                                    .color(p.text)
                                    .w_full()
                                    .min_h(20.0)
                                    .animate_edit_layout()
                                    .placeholder(s.composer_placeholder(), p.faint)
                                    .caret(p.text, p.selection)
                                    .spellcheck(s.composer_spellcheck() && !s.composer_shell_mode())
                                    .on_edit(S::composer_edit_mut)
                                    .pill_style(mention_pill_style(p))
                                    .on_input_change(S::composer_on_change)
                                    .on_input_key(S::composer_on_key)
                                    .on_input_paste(attachments::paste::<S>)
                                    .on_submit(submit_or_stop::<S>),
                                ),
                            ),
                    ),
            )
            .child(
                row()
                    .key("toolbar")
                    .transition(s.composer_face_transition())
                    // `editor.tsx:424-462`: 44 tall, 8 in, the pickers 2 apart.
                    .h(44.0)
                    .px(8.0)
                    .gap(2.0)
                    .center_y()
                    .children(s.composer_toolbar_items(p, scope, busy && ready))
                    // The slot rides the composer's edge as a panel's spring moves it, as
                    // Desktop's follows the chat's width; the button's own swaps land at once.
                    .child(el().size(28.0, 28.0).child(submit)),
            )
            .children(s.composer_below_toolbar(p)),
    }
}

#[cfg(test)]
#[path = "composer_tests.rs"]
mod tests;
