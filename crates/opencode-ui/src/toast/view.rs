//! Rendering for the toast notification stack (`Toast.Region` / `.toast-v2`), composer/dock
//! clearance placement, card chrome, and action buttons.

use rune::{Anim, Color, El, Icon, Shadow, Spring, TextStyle, Transition, Tx, col, el, icon, row, spacer, text};
use rune_kit::{ToastEntry, ToastEvent};
use crate::palette::{Elevate as _, Pal};

use super::state::{dismiss_id, stack_key, step};
use super::{CONTENT_FADE, ComposerClearance, Entry, FADE, MOVE, STACK, Shown, ToastAction, ToastButton, ToastHost};

const INSTANT: Anim = Anim::INSTANT;
const TITLE: TextStyle = TextStyle::new(13.0, 530.0).lh(20.0).tracking(-0.04);
const DESC: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0).tracking(-0.04);

/// Sonner's close glyph, which `toast.css` draws at 16 px: two strokes 6 to 18 across 24, 1.5
/// wide with round caps.
static CLOSE: Icon = Icon {
    name: "toast-close",
    view: 24.0,
    body: r#"<path d="M18 6L6 18M6 6L18 18" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};

/// The stack (Sonner's `ol`): 32 in from the window's right and 48 up from its bottom
/// (`packages/ui/src/feedback/toast/toast.tsx:48-51`), at 600 pt or narrower 16 in from both
/// sides (`max-width: calc(100vw - 32px)`, `toast.css:226-233`, `mobileOffset={16}`). Official's
/// `<Toast.Region />` (`packages/app/src/shell/notifications/toast.tsx:26`) stays at `bottom: 16`
/// on narrow screens and covers the composer; here [`toast_bottom`] lifts the stack 8 pt above
/// `"dock"` / `"composer"` whenever the two overlap horizontally (deliberate difference).
pub fn view<S: ToastHost>(s: &S, p: &Pal) -> Option<El> {
    let entries = s.toast().stack.entries();
    if entries.is_empty() {
        return None;
    }
    let key_of = |e: &ToastEntry<Shown<S::CustomAction>>| match e.options {
        Shown::Status(_) => format!("status-toast-{}", e.id),
        Shown::Uploads => "upload-toast".to_owned(),
    };
    let height = |id: u64| {
        entries.iter().find(|e| e.id == id).and_then(|e| rune::memo::layout_size(&key_of(e))).map_or(0.0, |[_, h]| h)
    };
    let width = s.viewport_width();
    let narrow = width > 0.0 && width <= 600.0;
    let w = if narrow { (width - 32.0).max(0.0) } else { 320.0 };
    let mut reach = 0.0f32;
    // Focusable outside the Tab order, so the hotkey can focus it and Escape there reaches it;
    // a press on a toast focuses it as one on a toast's `li` does.
    let mut stack = el()
        .key(STACK)
        .role("region")
        .label("Notifications alt+T")
        .w(w)
        .focusable(true)
        .tab_index(-1)
        .on_key(stack_key::<S>);
    for (order, e) in entries.iter().enumerate() {
        let Some(at) = s.toast().stack.place(e.id, height) else { continue };
        let card = match &e.options {
            Shown::Status(entry) => card(s, p, entry, at.content),
            Shown::Uploads => {
                let Some(card) = s.uploads_card(p, at.content) else { continue };
                card
            }
        };
        if at.visible {
            reach = reach.max(at.lift + if at.content { at.height } else { at.height * at.scale + (1.0 - at.scale) * at.height * 0.5 });
        }
        let mut card = card.key(key_of(e)).w(w).pin(Some(0.0), None, None, Some(0.0)).paint_order(order as i16);
        if !at.content {
            // Behind the front one, as tall as it (`--front-toast-height`).
            card = card.h(at.height).clip_soft(0.0);
        }
        let (pulse, scale_motion) = s.toast().pulse.of(e.id).unwrap_or((1.0, MOVE));
        stack = stack.child(
            card.offset(0.0, -at.lift)
                .offset_motion(MOVE)
                .scale(at.scale * pulse)
                .scale_motion(scale_motion)
                .opacity(if at.visible { 1.0 } else { 0.0 })
                .opacity_motion(FADE),
        );
    }
    let bottom = toast_bottom(s, narrow, w, reach);
    let stack = stack.h(reach.max(1.0)).on_hover_change(|s: &mut S, inside, tx: &mut Tx| {
        let now = s.clock();
        let event = if inside { ToastEvent::PointerOver { now } } else { ToastEvent::PointerLeft { now } };
        let was = s.toast().stack.stack();
        step(s, event, Some(tx));
        if s.toast().stack.stack() != was {
            // Every toast moves and resizes on Sonner's curve (`toast.css`).
            tx.motion_curve(MOVE);
        }
    });
    Some(if narrow { stack.pin(Some(16.0), None, None, Some(bottom)).raise() } else { stack.pin(None, None, Some(32.0), Some(bottom)).raise() })
}

fn rect_identity(r: [f32; 4]) -> [f32; 4] {
    r
}

fn rect_top(r: [f32; 4]) -> f32 {
    r[1]
}

fn toast_bottom<S: ToastHost>(s: &S, narrow: bool, w: f32, reach: f32) -> f32 {
    let base = if narrow { 16.0 } else { 48.0 };
    let clearance = s.composer_clearance();
    if clearance == ComposerClearance::None {
        return base;
    }
    let Some([wx, wy, ww, wh]) = rune::memo::layout_with("window", rect_identity) else {
        return base;
    };
    let Some([cx, cy, cw, ch]) = rune::memo::layout_with("composer", rect_identity) else {
        return base;
    };
    let (tx0, tx1) = if narrow {
        (wx + 16.0, wx + 16.0 + w)
    } else if opencode_i18n::direction() == opencode_i18n::Direction::Rtl {
        (wx + 32.0, wx + 32.0 + w)
    } else {
        (wx + ww - 32.0 - w, wx + ww - 32.0)
    };
    if !(tx0 < cx + cw && tx1 > cx) {
        return base;
    }
    let top_y = if clearance == ComposerClearance::Session {
        rune::memo::layout_with("composer-dock", rect_top)
            .or_else(|| rune::memo::layout_with("dock", rect_top))
            .map_or(cy, |dy| cy.min(dy))
    } else {
        cy
    };
    if clearance == ComposerClearance::Draft {
        let toast_h = reach.max(64.0);
        if !((wy + wh - base) > top_y && (wy + wh - base - toast_h) < cy + ch) {
            return base;
        }
    }
    base.max(wy + wh - top_y + 8.0)
}

/// The card (`.toast-v2`): 12 in, its icon's, content's and close button's columns 12 apart,
/// the actions 12 under the content in its column. Behind the front toast of a collapsed stack
/// its contents hide (`[data-expanded="false"][data-front="false"] > *`).
fn card<S: ToastHost>(s: &S, p: &Pal, entry: &Entry<S::CustomAction>, content_shows: bool) -> El {
    let id = entry.id;
    let close = row()
        .key("toast-dismiss")
        .role("button")
        .label_static(opencode_i18n::t!("ui.common.dismiss"))
        .help(opencode_i18n::t!("ui.common.dismiss"))
        .size(20.0, 20.0)
        .radius(4.0)
        .center()
        .hover(p.hover)
        .press(p.pressed)
        .hover_group()
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(move |s: &mut S, tx: &mut Tx| {
            tx.instant();
            dismiss_id(s, id);
            // `ToastOptions.onDismiss`: the person closed it.
            s.on_dismissed(id, tx);
        })
        // `icon-muted` at rest, `icon-base` under the pointer.
        .child(icon(&CLOSE, 16.0).color(p.icon_muted))
        .child(icon(&CLOSE, 16.0).color(p.icon).absolute(2.0, 2.0).group_opacity(0.0, 1.0));

    let action_btn = entry.action.as_ref().map(|action| action_button(s, p, id, action));

    let mut content = col()
        .key("toast-content")
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .gap(4.0)
        .child(text(entry.title.clone()).key("toast-title").font(TITLE).color(p.text).truncate().w_full())
        .children(entry.description.as_ref().map(|desc| text(desc.clone()).key("toast-description").font(DESC).color(p.muted).wrap().w_full()))
        // The server's words, set apart by a rule as a quotation is, in the text colour: theirs.
        .children(entry.quote.as_ref().map(|q| {
            el().key("toast-quote")
                .role("note")
                .w_full()
                .pl(9.0)
                .py(1.0)
                .border_sides([0.0, 0.0, 0.0, 2.0], p.border)
                .child(text(q.clone()).font(DESC).color(p.text).wrap().w_full())
        }))
        .children(entry.next.as_ref().map(|next| text(next.clone()).key("toast-next").font(DESC).color(p.muted).wrap().w_full().mt(4.0)));
    if let Some(btn) = action_btn {
        content = content.child(row().key("toast-actions").mt(8.0).gap(8.0).child(btn).child(spacer().fill(Color::TRANSPARENT)));
    } else if !entry.buttons.is_empty() {
        let buttons = entry.buttons.iter().enumerate().map(|(i, b)| extra_button::<S>(p, id, i, b));
        content = content.child(row().key("toast-actions").mt(8.0).gap(8.0).wrap().children(buttons));
    }
    let lead = entry.icon.map(|glyph| el().key("toast-icon").w(16.0).h(20.0).center().child(icon(glyph, 16.0).color(p.icon)));
    let body = row()
        .key("toast-head")
        .w_full()
        .gap(12.0)
        .opacity(if content_shows { 1.0 } else { 0.0 })
        .opacity_motion(CONTENT_FADE)
        .children(lead)
        .child(content)
        .child(close);

    // A group named by its title: what it says reaches a reader once, through the announcement
    // `show` makes, not as a live region that also reads its buttons as part of the message.
    col()
        .role("group")
        .label(entry.title.clone())
        .padding(12.0)
        .radius(8.0)
        .fill(p.layer1)
        .elevate(p.floating)
        .transition(Transition::none())
        .motion(Spring::NONE)
        .on_press(|_: &mut S| {})
        .child(body)
}

fn action_button<S: ToastHost>(s: &S, p: &Pal, id: u64, action: &ToastAction<S::CustomAction>) -> El {
    let (key, label) = match action {
        ToastAction::Reveal(label, _) => ("toast-reveal", label.clone()),
        ToastAction::CopyDetails(_) => ("toast-copy-details", opencode_i18n::t!("rune.error.copyDetails").to_owned()),
        ToastAction::Custom(custom) => {
            let meta = s.custom_action_meta(custom);
            (meta.key, meta.label)
        }
    };
    let action = action.clone();
    row()
        .key(key)
        .role("button")
        .label(label.clone())
        .h(24.0)
        .px(9.0)
        .radius(4.0)
        .center()
        .fill(neutral_fill(p))
        .elevate(neutral_lift(p))
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(move |s: &mut S, tx: &mut Tx| match action.clone() {
            ToastAction::Reveal(_, path) => {
                dismiss_id(s, id);
                s.reveal_path(&path);
            }
            ToastAction::CopyDetails(details) => {
                dismiss_id(s, id);
                tx.copy_rich(rune::ClipboardContent::new(details));
            }
            ToastAction::Custom(custom) => s.on_custom_action(id, custom, tx),
        })
        .child(text(label).font(TITLE).color(p.text))
}

/// A button in a multi-button toast's action row (`button-v2` small, neutral or ghost, 13/530).
fn extra_button<S: ToastHost>(p: &Pal, id: u64, i: usize, button: &ToastButton<S::CustomAction>) -> El {
    let key = button.key.map(str::to_owned).unwrap_or_else(|| format!("toast-action-{i}"));
    let label = button.label.clone();
    let action = button.action.clone();
    let base = row().key(key).role("button").label(label.clone()).h(24.0).px(9.0).radius(4.0).center();
    let styled = if button.secondary { base } else { base.fill(neutral_fill(p)).elevate(neutral_lift(p)) };
    styled
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(move |s: &mut S, tx: &mut Tx| {
            tx.instant();
            s.on_custom_action(id, action.clone(), tx);
        })
        .child(text(label).font(TITLE).color(p.text))
}

/// `button-v2` neutral at size small (`button.css`). Dark's fill is `alpha-light-6` over the
/// toast, which the theme's token flattened over `bg-base` isn't.
fn neutral_fill(p: &Pal) -> Color {
    if p.dark && p.theme == 0 { Color::WHITE.alpha(0.06) } else { p.q.neutral }
}

/// `--v2-elevation-button-neutral`: a drop and the 0.5px ring outside the button, and in dark the
/// 0.5px lit top edge (`theme.css`).
pub fn neutral_lift(p: &Pal) -> [Shadow; 4] {
    let (y, blur) = if p.dark { (1.0, 2.0) } else { (1.0, 1.5) };
    let top = if p.dark { Shadow { y: -0.5, color: Color::WHITE.alpha(0.1), ..Shadow::default() } } else { Shadow::default() };
    [Shadow { y, blur, color: p.q.neutral_shadow, ..Shadow::default() }, Pal::ring_out(p.q.neutral_ring), top, Shadow::default()]
}
