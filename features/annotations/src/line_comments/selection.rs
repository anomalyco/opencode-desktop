//! `pierre/comment-selection-machine.ts` at a9a35b4b4c. Rune reports a native selection
//! again when the pointer releases. Layout reads track scrolling, resizing and unmounting.

use rune::{Cx, El, Tx, row, text};
use session_view::palette::{Elevate as _, Pal};
use session_view::style::UI_MED;

use super::{LineCommentsHost, Range, Spot, machine};

pub struct Offer {
    pub viewer: String,
    pub spot: Spot,
    pub range: Range,
    pub backward: bool,
}

#[derive(Default)]
pub enum State {
    #[default]
    Idle,
    Pressing,
    Settling { due: f64, offer: Offer },
    Shown(Offer),
}

impl State {
    /// Whether Escape should dismiss the selection toolbar (`Settling` or `Shown`).
    pub fn is_active(&self) -> bool {
        matches!(self, State::Settling { .. } | State::Shown(_))
    }
}

pub enum Event {
    Press,
    Release(Offer, f64),
    Tick(f64),
    Clear,
    Add,
}

enum Effect {
    None,
    Wait(f64),
    Redraw,
    Open(Offer),
}

fn step(state: &mut State, event: Event) -> Effect {
    match event {
        Event::Press => *state = State::Pressing,
        Event::Clear => *state = State::Idle,
        Event::Release(offer, now) => {
            let due = now + 0.15;
            *state = State::Settling { due, offer };
            return Effect::Wait(due);
        }
        Event::Tick(now) => {
            if !matches!(state, State::Settling { due, .. } if now >= *due) {
                return Effect::None;
            }
            if let State::Settling { offer, .. } = std::mem::take(state) {
                *state = State::Shown(offer);
            }
        }
        Event::Add => {
            if let State::Shown(offer) = std::mem::take(state) {
                return Effect::Open(offer);
            }
        }
    }
    Effect::Redraw
}

pub fn dispatch(host: &mut dyn LineCommentsHost, event: Event, tx: &mut Tx) -> bool {
    // Polls without a deadline leave the tracked store untouched.
    if let Event::Tick(now) = event
        && !matches!(&host.line_comments().selection, State::Settling { due, .. } if now >= *due)
    {
        return false;
    }
    match step(&mut host.line_comments_mut().selection, event) {
        Effect::Wait(due) => tx.wake_at(due),
        Effect::Open(offer) => {
            tx.clear_text_selection();
            super::line_comment(host, &offer.spot, machine::Event::DraftOpened(offer.range), tx);
        }
        Effect::Redraw => (),
        Effect::None => return false,
    }
    true
}

fn place_toolbar(
    rects: &[[f32; 4]],
    viewer: [f32; 4],
    zoomed: [f32; 2],
    size: Option<[f32; 2]>,
    backward: bool,
    hover_none: bool,
) -> Option<(f32, f32, bool)> {
    let [left, top, width, height] = viewer;
    let bottom = top + height;
    let first = rects.first()?;
    let last = rects.last()?;
    if last[1] + last[3] <= top || first[1] >= bottom {
        return None;
    }
    let [ox, oy] = zoomed;
    let Some([w, h]) = size else {
        return Some((0.0, 0.0, false));
    };
    let single = (first[1] - last[1]).abs() < 0.5;
    let end = last[0] + last[2];
    let center = if single {
        (first[0] + end) / 2.0
    } else if backward {
        first[0]
    } else {
        end
    };
    let above = first[1] - h - 8.0;
    let below = last[1] + last[3] + 8.0;
    let sides = if single || backward { [above, below] } else { [below, above] };
    let pinned = bottom - h - 8.0;
    let (x, y) = if hover_none {
        (left + (width - w) / 2.0, pinned)
    } else {
        let x = (center - w / 2.0).min(left + width - w - 8.0).max(left + 8.0);
        (x, sides.into_iter().find(|y| *y >= top + 8.0 && *y <= pinned).unwrap_or(pinned))
    };
    Some((x - ox, y - oy, true))
}

fn button_shell(p: &Pal) -> El {
    let label = opencode_i18n::t!("ui.lineComment.add");
    row()
        .key("line-comment-add")
        .role("button")
        .label_static(label)
        .px(8.0)
        .h(24.0)
        .center()
        .radius(6.0)
        .hover(p.hover)
        .press(p.pressed)
        .child(text(label).font(UI_MED).color(p.text))
}

fn toolbar_shell(p: &Pal, button: El) -> El {
    row()
        .key("line-comment-toolbar")
        .role("toolbar")
        .label_static(opencode_i18n::t!("ui.lineComment.actions"))
        .keeps_text_selection()
        .padding(2.0)
        .radius(8.0)
        .fill(p.layer1)
        .elevate(p.raised)
        .child(button)
}

pub fn toolbar<H: LineCommentsHost + 'static>(s: &H, p: &Pal, cx: &mut Cx<H>) -> Option<El> {
    let State::Shown(offer) = &s.line_comments().selection else { return None };
    let rects = cx.selection_rects();
    let viewer = cx.window_layout(&offer.viewer)?;
    let zoomed = cx.layout_with("zoomed", |r| [r[0], r[1]])?;
    let size = cx.layout_size("line-comment-toolbar");
    let (x, y, measured) = place_toolbar(&rects, viewer, zoomed, size, offer.backward, cx.hover_none())?;
    let button = button_shell(p).on_press_tx(|s: &mut H, tx: &mut Tx| {
        dispatch(s, Event::Add, tx);
    });
    let bar = toolbar_shell(p, button);
    Some(if measured { bar.absolute(x, y) } else { bar.absolute(0.0, 0.0).opacity(0.0) })
}
