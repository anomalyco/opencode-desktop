//! What stands in the composer's place while the open session waits on someone: a question's
//! form (`question`), else a permission (`permission`). The composer's surface morphs into it
//! (`ComposerHost::composer_face`) and back once it's answered, here or anywhere else. Desktop
//! and Seer both draw it from here; the host says what's pending and sends the replies.

use opencode_kit::model::Pending;
use rune::{Anim, Color, El, KeyEvent, SpringPair, TextStyle, Transition, Tx, row, text};

use crate::composer::ComposerHost;
use crate::palette::Pal;

pub mod knobs;
pub mod permission;
pub mod question;

pub use question::Dock;

const BUTTON: TextStyle = TextStyle::new(13.0, 530.0).lh(20.0).tracking(-0.04);

/// A host that shows the dock: its open requests and where their replies go.
pub trait DockHost: ComposerHost {
    fn dock(&self) -> &Dock;
    fn dock_mut(&mut self) -> &mut Dock;
    /// The open requests in session `id`'s family (a subagent's show on its root).
    fn dock_pending(&self, id: &str) -> Vec<Pending>;
    /// Session `id`'s parent, when it's a subagent.
    fn dock_parent(&self, id: &str) -> Option<String>;
    /// Whether permissions are answered without asking.
    fn dock_auto_accept(&self) -> bool {
        false
    }
    /// Answers permission `id` with `decision` (`once`, `always`, `reject`).
    fn dock_reply_permission(&mut self, id: &str, decision: &str);
    /// Sends form `id`'s answer (`Dock::answer`).
    fn dock_submit_form(&mut self, id: &str);
    fn dock_dismiss_form(&mut self, id: &str);
    /// Whether request `id` still waits.
    fn dock_waiting(&self, id: &str) -> bool;
    /// Whether the composer takes the keyboard back when the dock closes.
    fn dock_refocus(&self) -> bool {
        true
    }
    /// A key the typed answer's field doesn't take, for the app's shortcuts.
    fn dock_shortcut(&mut self, _ev: &KeyEvent, _tx: &mut Tx) -> bool {
        false
    }
    /// The height of the pane the dock sits in, which a long question keeps to part of.
    fn dock_pane_h(&self) -> Option<f32> {
        ["story-session", "panel"].into_iter().find_map(|k| rune::memo::layout_with(k, |r| r[3]))
    }
}

/// The composer's face while a request waits: the question's form, else the permission.
pub fn face<S: DockHost>(s: &S, p: &Pal) -> Option<Vec<El>> {
    question::face(s, p).or_else(|| permission::face(s, p))
}

/// What stands in the composer's place, by id: the question's form, else a permission.
pub fn shown<S: DockHost>(s: &S) -> Option<String> {
    question::id(s).or_else(|| permission::asked(s).map(|p| p.id().to_owned()))
}

/// A dock (a question or a permission) opened or closed: the surface springs to its new face,
/// the timeline pinned above it rides its top edge, and the composer gets the keyboard back.
pub fn morph<S: DockHost>(s: &mut S, tx: &mut Tx) {
    tx.motion_curve(knobs::morph(s.knobs()));
    tx.follow_edge();
    if shown(s).is_none() && s.dock_refocus() {
        s.composer_edit_mut().focused = true;
    }
}

/// How the composer's face and the dock's swap inside the one surface: the face that leaves
/// fades out quickly, the one that arrives rises and fades up with the morph. The morph is
/// shared by opening and closing, so the dock leaves the way it came.
pub fn face_transition(k: &rune_knobs::Knobs) -> Transition {
    Transition::asymmetric(Transition::offset(0.0, k.get(&knobs::FACE_RISE)).combined(Transition::opacity()), Transition::opacity())
        .springs(SpringPair { enter: knobs::spring(k, &knobs::FACE_IN), exit: knobs::spring(k, &knobs::FADE_OUT) })
}

#[derive(Clone, Copy, PartialEq)]
pub enum Variant {
    Ghost,
    Neutral,
    Submit,
}

/// `button-v2` at size large.
pub fn button(p: &Pal, key: &str, label: &str, variant: Variant, hint: Option<&str>, live: bool) -> El {
    let (fill, fg) = match variant {
        Variant::Ghost => (Color::TRANSPARENT, p.muted),
        Variant::Neutral => (p.q.neutral, p.text),
        Variant::Submit => (p.q.submit, p.q.submit_text),
    };
    let mut b = row()
        .key(key)
        .role("button")
        .label(label.to_owned())
        .h(32.0)
        .px(15.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .fill(fill)
        .hover_motion(Anim::INSTANT, Anim::INSTANT)
        .child(text(label.to_owned()).font(BUTTON).color(fg))
        .children(hint.map(|h| text(h.to_owned()).key("hint").font(BUTTON).color(fg).opacity(0.6)));
    b = if live { b.hover(p.hover).press(p.pressed) } else { b.opacity(0.4) };
    match variant {
        Variant::Ghost => {}
        Variant::Neutral => b = b.border(0.5, p.q.neutral_ring).shadow(1.0, 1.5, p.q.neutral_shadow),
        Variant::Submit => b = b.border(0.5, p.q.submit_ring).shadow(1.0, 1.5, p.q.neutral_shadow),
    }
    b
}
