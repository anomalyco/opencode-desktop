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

pub(crate) const BUTTON: TextStyle = TextStyle::new(13.0, 530.0).lh(20.0).tracking(-0.04);

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
    /// Whether questions take the new keyboard-first dock (Desktop's Settings › Experimental ›
    /// New question UI) instead of its default Classic one.
    fn dock_new_question_ui(&self) -> bool {
        false
    }
    /// Answers permission `id` with `decision` (`once`, `always`, `reject`).
    fn dock_reply_permission(&mut self, id: &str, decision: &str);
    /// Sends form `id`'s answer (`Dock::answer`).
    fn dock_submit_form(&mut self, id: &str);
    fn dock_dismiss_form(&mut self, id: &str);
    /// Whether request `id` still waits.
    fn dock_waiting(&self, id: &str) -> bool;
    /// Whether request `id`'s reply is on its way or taken (Desktop's `isPermissionResponding`,
    /// `isQuestionBusy`): the dock shows it with its buttons off and takes no answers.
    fn dock_busy(&self, _id: &str) -> bool {
        false
    }
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

/// Whether the face draws its own surfaces instead of the composer's card: the Classic question
/// (a raised body over its footer tray) and the permission (a raised card).
pub fn bare<S: DockHost>(s: &S) -> bool {
    match question::id(s) {
        Some(_) => question::classic(s),
        None => permission::asked(s).is_some(),
    }
}

/// `top` over the opaque `bottom`, blended in sRGB as a browser does, as one opaque colour.
pub fn flat(top: Color, bottom: Color) -> Color {
    let srgb = |c: Color| {
        let h = c.hex();
        [1, 3, 5].map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0) as f32)
    };
    let (t, b) = (srgb(top), srgb(bottom));
    let m = |i: usize| (t[i] * top.a + b[i] * (1.0 - top.a)).round() as u8;
    rune::color::hex(&format!("#{:02x}{:02x}{:02x}", m(0), m(1), m(2))).unwrap_or(bottom)
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
    /// A ghost button in a question's footer, which mutes it (`question-footer > ghost`).
    GhostMuted,
    Neutral,
    Submit,
    /// `contrast`: the Classic question's Submit, on `bg-contrast` rather than Submit's
    /// `bg-icon-button-contrast` (the two differ in dark).
    Contrast,
}

/// `button-v2` at size large.
pub fn button(p: &Pal, key: &str, label: &str, variant: Variant, hint: Option<&str>, live: bool) -> El {
    let (fill, fg) = match variant {
        Variant::Ghost => (Color::TRANSPARENT, p.text),
        Variant::GhostMuted => (Color::TRANSPARENT, p.muted),
        // Dark's neutral fill is translucent (`alpha-light-6`), and the theme flattens it over
        // `bg-base`; the docks it sits on are `layer-01`, so it's laid over that instead.
        Variant::Neutral if p.dark && p.theme == 0 => (flat(Color::WHITE.alpha(0.06), p.layer1), p.text),
        Variant::Neutral => (p.q.neutral, p.text),
        Variant::Submit => (p.q.submit, p.q.submit_text),
        Variant::Contrast => (p.contrast, p.q.contrast_text),
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
    // Disabled, as `button-v2` dims it: 0.4 for Submit and Contrast (`submit.css`), else 0.5.
    let dim = if matches!(variant, Variant::Submit | Variant::Contrast) { 0.4 } else { 0.5 };
    b = if live { b.hover(p.hover).press(p.pressed) } else { b.opacity(dim) };
    match variant {
        Variant::Ghost | Variant::GhostMuted => {}
        Variant::Neutral => b = b.box_shadow(Pal::ring_out(p.q.neutral_ring)).shadow(1.0, 1.5, p.q.neutral_shadow),
        // `--v2-elevation-button-contrast` under the top-lit highlight (`--submit-highlight`,
        // brighter on dark's light Submit fill).
        Variant::Submit | Variant::Contrast => {
            let lit = if variant == Variant::Submit && p.dark { 0.6 } else { 0.2 };
            let (y, blur, drop) = if p.dark { (1.0, 2.0, 0.4) } else { (1.0, 1.5, 0.2) };
            b = b.gradient(Color::WHITE.alpha(lit), Color::WHITE.alpha(0.0)).box_shadow(Pal::ring_out(p.q.submit_ring)).shadow(y, blur, Color::BLACK.alpha(drop))
        }
    }
    b
}
