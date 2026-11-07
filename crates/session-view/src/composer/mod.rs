//! Shared OpenCode session composer (`p254`): OpenCode (Rune)'s auto-growing editor (`180pt` cap
//! with internal edge-faded scroll), attachment chips & image paste, prompt history (`history`),
//! `+` attach button, and instant Send / Stop button with `stop_glyph_swap()`.

pub mod attachments;
pub mod history;

use std::cell::Cell;

use opencode_kit::client::PathAttachment;
use opencode_kit::model::Attachment;
use rune::{
    Color, El, Key, KeyEvent, MotionIntent, ScrollSpec, Spring, SpringPair, TextEdit, TextStyle,
    Transition, Tx, col, el, icon, row, spacer, text, text_input,
};
use rune_theme_opencode::icons as ic;

use crate::TimelineHost;
use crate::palette::Pal;
use crate::style::{DOCK_SIDE, DOCK_W, UI};
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
    let mut r = row()
        .key(key)
        .role("button")
        .h(28.0)
        .px(8.0)
        .gap(2.0)
        .center_y()
        .radius(6.0)
        .hover(p.hover)
        .press(p.pressed)
        .child(text(label.to_owned()).font(UI).color(color).truncate_fade());
    if chevron {
        r = r.child(icon(&ic::CHEVRON_DOWN, 16.0).color(p.icon_faint));
    }
    r
}

/// Default `+` attach button at the start of the toolbar (`⌘U`).
pub fn attach_button<S: ComposerHost>(p: &Pal) -> El {
    el()
        .key("add")
        .role("button")
        .label("Add images and files")
        .help(format!("Add images and files {}", crate::keymap::Bind::Attach.label()))
        .size(28.0, 28.0)
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
    let submit = el()
        .key("submit")
        .role("button")
        .label("Send")
        .size(28.0, 28.0)
        .radius(6.0)
        .center()
        .fill(if ready { p.contrast } else { p.layer2 })
        .hover(p.hover)
        .motion(Spring::NONE)
        .motion_intent(MotionIntent::CUT)
        .on_press_tx(move |s: &mut S, tx: &mut Tx| {
            if stop {
                s.composer_stop(tx);
            } else {
                s.composer_send(tx);
            }
        })
        .child(
            if stop {
                el().key("stop-glyph").size(10.0, 10.0).radius(2.0).fill(p.text)
            } else {
                icon(&ic::ARROW_UP, 16.0).key("send-glyph").color(match (ready, p.dark) {
                    (false, _) => p.icon_faint,
                    (true, true) => p.text,
                    (true, false) => p.q.contrast_text,
                })
            }
            .transition(stop_glyph_swap()),
        );
    let submit = if stop { submit.label("Stop").fill(p.contrast) } else { submit };
    let key = if scope.is_empty() {
        "composer".to_owned()
    } else {
        format!("{scope}-composer")
    };
    let face = s.composer_face(p);
    let box_el = col().key(key).w_full().max_w(DOCK_W).min_h(88.0).corners(if s.composer_docked_top() { [0.0, 0.0, 12.0, 12.0] } else { [12.0; 4] });
    // A face with surfaces of its own keeps its shadows: no card under it and no clip over it.
    let box_el = if face.is_some() && s.composer_face_bare() {
        box_el
    } else {
        box_el.fill(composer_fill(p)).border(0.5, p.ring).shadow(1.0, 2.0, p.shadow).clip_soft(0.0)
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
                            .scroll_edge_fade(16.0)
                            .scroll_margin(16.0)
                            .child(
                                col().key("editor-pad").w_full().px(16.0).pt(16.0).child(
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
                    .h(44.0)
                    .px(8.0)
                    .gap(4.0)
                    .center_y()
                    .children(s.composer_toolbar_items(p, scope, busy && ready))
                    .child(submit),
            )
            .children(s.composer_below_toolbar(p)),
    }
}

#[cfg(test)]
#[path = "composer_tests.rs"]
mod tests;
