//! Non-generic viewport pointer, drag, wheel, scrollbar, and quote handlers with thin `El`
//! callback adapters.

use opencode_ui::keymap::{logo_held, modifier_label};
use rune::{
    Anim, Cursor, Drag, El, KeyEvent, Phase, PointerButton, PointerEvent, Spring, TextStyle,
    Transition, Tx, WheelEvent, el, icon, row, text,
};
use opencode_ui::style::composer_fill;
use opencode_ui::palette::Pal;
use opencode_ui::style::OUTLINE_QUOTE;
use terminal::{
    GhosttyKey, GridPoint, KeyInput, MouseBtn, MouseInput, MouseKind, ScrollOp, ScrollbarSnapshot,
    SelectionOp,
};

use super::{PAD_X, PAD_Y, SCROLLBAR_W, edge};
use crate::mount::{EdgeScroll, TextDrag};
use crate::{HoverLink, TerminalSurfaceHost, links, menu, ops, side_ops};

const INSTANT: Anim = Anim::INSTANT;
const QUOTE_BUTTON: TextStyle = TextStyle::new(13.0, 530.0).lh(20.0).tracking(-0.04);

#[derive(Clone)]
pub(super) struct ViewportInput {
    pub ws_key: String,
    pub tab_id: String,
    pub cell_w: f32,
    pub cell_h: f32,
    pub cols_cap: u16,
    pub rows_cap: u16,
    pub reports: bool,
    /// Where the text starts in the handling element: the viewport's padding, or zero on the
    /// screen text itself.
    pub origin: (f32, f32),
}

impl ViewportInput {
    fn cell(&self, x: f32, y: f32) -> (f32, f32, u16, u16) {
        super::cell::cell_at(x, y, self.origin, (self.cell_w, self.cell_h), (self.cols_cap, self.rows_cap))
    }
}

fn primary_link_mod(mods: rune::Mods) -> bool {
    if cfg!(target_os = "macos") { mods.cmd } else { mods.ctrl }
}

fn handle_key(host: &mut dyn TerminalSurfaceHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if side_ops::terminal_focus(host).is_none() {
        ops::focus_terminal_surface(host);
    }
    side_ops::key_terminal(host, ev, tx)
}

fn handle_drag(host: &mut dyn TerminalSurfaceHost, input: &ViewportInput, d: &mut Drag, tx: &mut Tx) {
    tx.instant();
    ops::focus_terminal_surface(host);
    let (lx, ly, col, row) = input.cell(d.x, d.y);
    let mods = tx.mods();
    let button = tx.button();
    let mut follow_link = false;
    {
        let Some(t) = ops::terminal_tab_mut(host, &input.ws_key, &input.tab_id) else {
            return;
        };
        // A press takes control of a shared terminal at this pane's size (interacted).
        if matches!(d.phase, Phase::Down) {
            t.session.interact();
        }
        if input.reports && !mods.shift {
            let kind = match d.phase {
                Phase::Down => MouseKind::Press,
                Phase::Move => MouseKind::Move,
                Phase::Up => MouseKind::Release,
            };
            let button = if button == PointerButton::Middle { MouseBtn::Middle } else { MouseBtn::Left };
            t.session.send_mouse(MouseInput {
                kind,
                button,
                x_px: lx,
                y_px: ly,
                shift: false,
                ctrl: mods.ctrl,
                alt: mods.alt,
                cmd: logo_held(mods),
            });
            return;
        }
        if button != PointerButton::Primary {
            return;
        }
        let mut held = false;
        match d.phase {
            // Shift+press (without Ctrl/⌘/Alt) moves the end of the selection there, and dragging
            // keeps moving it; on a link it clears the selection and the release opens the link,
            // as Desktop's click handler does whatever is selected (`terminal.tsx:312-340`).
            Phase::Down
                if mods.shift
                    && !primary_link_mod(mods)
                    && !mods.alt
                    && d.clicks == 1
                    && t.session.selected_text().is_some()
                    && !links::is_url_at(&t.session.peek(), col, row) =>
            {
                t.drag = TextDrag::Extending;
                t.session.select(SelectionOp::Extend(GridPoint::Viewport { col, row }));
            }
            Phase::Move if t.drag == TextDrag::Extending => {
                t.session.select(SelectionOp::Extend(GridPoint::Viewport { col, row }));
                held = true;
            }
            Phase::Down => {
                let top = t.session.peek().scrollbar.offset as u32;
                t.drag = TextDrag::Selecting { anchor: (col, top + u32::from(row)) };
                match d.clicks {
                    2 => t.session.select(SelectionOp::Word(GridPoint::Viewport { col, row })),
                    3.. => t.session.select(SelectionOp::Line(GridPoint::Viewport { col, row })),
                    _ => t.session.select(SelectionOp::Clear),
                }
            }
            Phase::Move => {
                if (d.dx.hypot(d.dy)) > 3.0
                    && let TextDrag::Selecting { anchor } = t.drag
                {
                    // Alt drags a block, as in Windows Terminal, iTerm2 and Ghostty.
                    t.session.select(SelectionOp::Range {
                        start: GridPoint::Screen { col: anchor.0, row: anchor.1 },
                        end: GridPoint::Viewport { col, row },
                        rectangular: mods.alt,
                    });
                    held = true;
                }
            }
            Phase::Up => {
                let extending = t.drag == TextDrag::Extending;
                t.drag = TextDrag::Idle;
                t.edge_scroll = None;
                // Ctrl+click (⌘-click on macOS) or Shift+click follows a link: the cell's OSC 8
                // target, else an address (or a file reference with Ctrl/⌘) under the pointer.
                // Shift routes web links to the system browser (`terminal-key-event.ts:44,51`).
                let follow = !mods.alt && (primary_link_mod(mods) || mods.shift);
                if !extending && d.clicks == 1 && d.dx.hypot(d.dy) <= 3.0 && follow {
                    follow_link = true;
                }
            }
        }
        // A drag held at or past the text's top or bottom edge scrolls the terminal
        // ([`poll_edge_scroll`]) until it moves back inside or lets go.
        if held {
            let dir = edge::edge_dir(d.y - input.origin.1, f32::from(input.rows_cap) * input.cell_h);
            t.edge_scroll = match t.edge_scroll {
                _ if dir == 0 => None,
                Some(e) if e.dir == dir => Some(e),
                _ => {
                    let due = tx.clock() + edge::STEP_SECS;
                    tx.wake_at(due);
                    Some(EdgeScroll { dir, due, block: mods.alt })
                }
            };
        }
    }
    if follow_link {
        links::follow(host, &input.ws_key, &input.tab_id, col, row, tx);
    }
}

/// Steps every terminal's due edge scroll: [`edge::LINES`] toward the edge, then the selection's
/// end to the edge row's far cell (the top row's first, the bottom row's last), as ghostty-web's
/// auto-scroll interval does (`selection-manager.ts:797-829`). The session runs the scroll before
/// the selection, so the edge row is the one the scroll brought into view.
pub(crate) fn poll_edge_scroll(host: &mut dyn TerminalSurfaceHost, tx: &mut Tx) -> bool {
    let now = tx.clock();
    let mut stepped = false;
    for ws in host.terminal_mut().workspaces.values_mut() {
        for t in &mut ws.tabs {
            let Some(mut e) = t.edge_scroll else { continue };
            if t.drag == TextDrag::Idle {
                t.edge_scroll = None;
                continue;
            }
            if now >= e.due {
                let snap = t.session.peek();
                let end = if e.dir < 0 {
                    GridPoint::Viewport { col: 0, row: 0 }
                } else {
                    GridPoint::Viewport { col: snap.cols.saturating_sub(1), row: snap.rows.saturating_sub(1) }
                };
                t.session.scroll(ScrollOp::Delta(isize::from(e.dir) * edge::LINES));
                match t.drag {
                    TextDrag::Selecting { anchor } => t.session.select(SelectionOp::Range {
                        start: GridPoint::Screen { col: anchor.0, row: anchor.1 },
                        end,
                        rectangular: e.block,
                    }),
                    _ => t.session.select(SelectionOp::Extend(end)),
                }
                e.due = now + edge::STEP_SECS;
                t.edge_scroll = Some(e);
                stepped = true;
            }
            tx.wake_at(e.due);
        }
    }
    stepped
}

fn handle_wheel(host: &mut dyn TerminalSurfaceHost, input: &ViewportInput, ev: WheelEvent, tx: &mut Tx) -> bool {
    tx.instant();
    let Some(t) = ops::terminal_tab_mut(host, &input.ws_key, &input.tab_id) else {
        return false;
    };
    let snap = t.session.snapshot();
    let step = if ev.precise { input.cell_h } else { input.cell_h * 0.5 }.max(4.0);
    if snap.mouse_tracking && !ev.mods.shift {
        if ev.dy.abs() < 0.5 {
            return true;
        }
        let btn = if ev.dy > 0.0 { MouseBtn::WheelUp } else { MouseBtn::WheelDown };
        t.session.send_mouse(MouseInput {
            kind: MouseKind::Press,
            button: btn,
            x_px: (ev.x - PAD_X).max(0.0),
            y_px: (ev.y - PAD_Y).max(0.0),
            shift: ev.mods.shift,
            ctrl: ev.mods.ctrl,
            alt: ev.mods.alt,
            cmd: logo_held(ev.mods),
        });
        return true;
    }
    t.scroll_px_accum += ev.dy;
    let lines = (t.scroll_px_accum / step).trunc() as isize;
    if lines == 0 {
        return true;
    }
    t.scroll_px_accum -= (lines as f32) * step;
    // The alternate screen has no scrollback: a pager or editor that didn't ask for the
    // mouse scrolls by arrow keys (xterm's alternate scroll, `?1007`), as every terminal does.
    if snap.alt_screen {
        let key = if lines > 0 { GhosttyKey::ArrowUp } else { GhosttyKey::ArrowDown };
        for _ in 0..lines.unsigned_abs().min(50) {
            t.session.send_key(KeyInput::press(key));
        }
        return true;
    }
    // Natural scrolling: positive `dy` moves content down, revealing older lines (`-lines`).
    t.session.scroll(ScrollOp::Delta(-lines));
    true
}

fn handle_secondary_press(host: &mut dyn TerminalSurfaceHost, input: &ViewportInput, p: PointerEvent, tx: &mut Tx) {
    let (lx, ly, _, _) = input.cell(p.x, p.y);
    let forwarded = {
        let Some(t) = ops::terminal_tab_mut(host, &input.ws_key, &input.tab_id) else { return };
        // A program reading the mouse gets the right button (press and release together: the
        // release isn't heard apart); otherwise the terminal's menu.
        if t.session.snapshot().mouse_tracking && !tx.mods().shift {
            for kind in [MouseKind::Press, MouseKind::Release] {
                t.session.send_mouse(MouseInput {
                    kind,
                    button: MouseBtn::Right,
                    x_px: lx,
                    y_px: ly,
                    shift: false,
                    ctrl: false,
                    alt: false,
                    cmd: false,
                });
            }
            true
        } else {
            false
        }
    };
    if !forwarded {
        menu::open(host, &input.ws_key, &input.tab_id, p.window(), tx);
    }
}

fn handle_pointer_move(host: &mut dyn TerminalSurfaceHost, input: &ViewportInput, p: PointerEvent, tx: &mut Tx) {
    let (lx, ly, col, row) = input.cell(p.x, p.y);
    let mods = tx.mods();
    let Some(t) = ops::terminal_tab_mut(host, &input.ws_key, &input.tab_id) else { return };
    if input.reports && !mods.shift {
        t.session.send_mouse(MouseInput {
            kind: MouseKind::Move,
            button: MouseBtn::None,
            x_px: lx,
            y_px: ly,
            shift: false,
            ctrl: mods.ctrl,
            alt: mods.alt,
            cmd: false,
        });
        tx.live_only();
        return;
    }
    let snap = t.session.peek();
    let follow = !mods.alt
        && (primary_link_mod(mods) || (mods.shift && links::is_url_at(&snap, col, row)));
    let next = HoverLink::at(col, row, follow);
    let now = links::span(&snap, col, row);
    let (was, was_follow) = match t.hover {
        HoverLink::None => (Vec::new(), None),
        HoverLink::Plain { col: c, row: r } => (links::span(&snap, c, r), Some(false)),
        HoverLink::Followable { col: c, row: r } => (links::span(&snap, c, r), Some(true)),
    };
    let drawn_same = now == was && (now.is_empty() || was_follow == Some(follow));
    t.hover = next;
    if drawn_same {
        tx.live_only();
    }
}

fn handle_hover_change(host: &mut dyn TerminalSurfaceHost, ws_key: &str, tab_id: &str, inside: bool) {
    if !inside && let Some(t) = ops::terminal_tab_mut(host, ws_key, tab_id) {
        t.hover = HoverLink::None;
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_scrollbar_drag(
    host: &mut dyn TerminalSurfaceHost,
    ws_key: &str,
    tab_id: &str,
    thumb_h: f32,
    travel: f32,
    max_row: u64,
    d: &mut Drag,
    tx: &mut Tx,
) {
    tx.instant();
    match d.phase {
        Phase::Down => {
            let frac = ((d.y - thumb_h * 0.5) / travel.max(1.0)).clamp(0.0, 1.0);
            let target = (frac * max_row as f32).round() as usize;
            if let Some(t) = ops::terminal_tab_mut(host, ws_key, tab_id) {
                t.session.scroll(ScrollOp::Row(target));
            }
            d.data = target as f32;
        }
        Phase::Move => {
            let frac = ((d.y - thumb_h * 0.5) / travel.max(1.0)).clamp(0.0, 1.0);
            let target = (frac * max_row as f32).round() as usize;
            if let Some(t) = ops::terminal_tab_mut(host, ws_key, tab_id) {
                t.session.scroll(ScrollOp::Row(target));
            }
        }
        Phase::Up => {}
    }
}

fn handle_quote_press(host: &mut dyn TerminalSurfaceHost, ws_key: &str, tab_id: &str, tx: &mut Tx) {
    let text = {
        let Some(t) = ops::terminal_tab_mut(host, ws_key, tab_id) else { return };
        let Some(text) = t.session.selected_text() else { return };
        t.session.select(SelectionOp::Clear);
        text
    };
    host.quote_terminal_selection(&text, tx);
}

pub(super) fn attach_viewport_handlers<H: TerminalSurfaceHost + 'static>(outer: El, input: ViewportInput) -> El {
    let drag_in = input.clone();
    let wheel_in = input.clone();
    let menu_in = input.clone();
    let move_in = input.clone();
    let (wk_leave, tid_leave) = (input.ws_key, input.tab_id);
    outer
        .on_key(|app: &mut H, ev: &KeyEvent, tx: &mut Tx| handle_key(app, ev, tx))
        .on_drag(move |app: &mut H, d: &mut Drag, tx: &mut Tx| handle_drag(app, &drag_in, d, tx))
        .on_wheel(move |app: &mut H, ev: WheelEvent, tx: &mut Tx| handle_wheel(app, &wheel_in, ev, tx))
        .on_secondary_press_local(move |app: &mut H, p: PointerEvent, tx: &mut Tx| handle_secondary_press(app, &menu_in, p, tx))
        .on_pointer_move_local(move |app: &mut H, p: PointerEvent, tx: &mut Tx| handle_pointer_move(app, &move_in, p, tx))
        .on_hover_change(move |app: &mut H, inside: bool, _tx: &mut Tx| handle_hover_change(app, &wk_leave, &tid_leave, inside))
}

/// The screen text's own drag: it is a focusable element (the terminal's Tab stop), and a press
/// goes to the topmost focusable element under it, so the viewport's `on_drag` never hears a
/// press that starts on the text. Selecting, a double-click's word, Shift+click and Ctrl+click on
/// a link start here; the viewport keeps its own for presses in its padding.
pub(super) fn attach_text_drag<H: TerminalSurfaceHost + 'static>(text: El, input: ViewportInput) -> El {
    text.on_drag(move |app: &mut H, d: &mut Drag, tx: &mut Tx| handle_drag(app, &input, d, tx))
}

pub(super) fn render_scrollbar<H: TerminalSurfaceHost + 'static>(
    p: &Pal,
    ws_key: &str,
    tab_id: &str,
    sb: &ScrollbarSnapshot,
    track_h: f32,
) -> El {
    let total = (sb.total as f32).max(1.0);
    let len = (sb.len as f32).min(total);
    let thumb_h = ((len / total) * track_h).clamp(20.0, track_h);
    let max_offset = sb.total.saturating_sub(sb.len).max(1) as f32;
    let travel = (track_h - thumb_h).max(0.0);
    let thumb_y = ((sb.offset as f32 / max_offset) * travel).clamp(0.0, travel);
    let max_row = sb.total.saturating_sub(sb.len);

    let (wk, tid) = (ws_key.to_owned(), tab_id.to_owned());
    scrollbar_shell(p, track_h, thumb_y, thumb_h)
        .on_drag(move |app: &mut H, d: &mut Drag, tx: &mut Tx| {
            handle_scrollbar_drag(app, &wk, &tid, thumb_h, travel, max_row, d, tx);
        })
}

fn scrollbar_shell(p: &Pal, track_h: f32, thumb_y: f32, thumb_h: f32) -> El {
    el().key("terminal-scrollbar")
        .w(SCROLLBAR_W)
        .h(track_h)
        .cursor(Cursor::Default)
        .child(
            el().key("terminal-scrollbar-thumb")
                .absolute(2.0, thumb_y)
                .size(4.0, thumb_h)
                .radius(2.0)
                .fill(p.icon_muted.alpha(0.55))
                .hover(p.icon.alpha(0.8))
                .hover_motion(INSTANT, INSTANT),
        )
}

/// Floating Quote button matching Desktop's `TerminalQuote` toolbar.
pub(super) fn render_quote_bar<H: TerminalSurfaceHost + 'static>(p: &Pal, ws_key: &str, tab_id: &str) -> El {
    let (wk, tid) = (ws_key.to_owned(), tab_id.to_owned());
    let button = quote_button_shell(p)
        .on_press_tx(move |app: &mut H, tx: &mut Tx| handle_quote_press(app, &wk, &tid, tx));
    quote_toolbar_shell(p, button)
}

fn quote_button_shell(p: &Pal) -> El {
    row()
        .key("quote-add")
        .role("button")
        .label_static(opencode_i18n::t!("session.quote.add"))
        .help(opencode_i18n::t!("session.quote.addWithoutComment"))
        .help_keys([modifier_label(rune::ChordMod::Alt)])
        .h(24.0)
        .px(9.0)
        .gap(6.0)
        .center_y()
        .radius(4.0)
        .hover(p.hover)
        .press(p.pressed)
        .child(icon(&OUTLINE_QUOTE, 16.0).color(p.text))
        .child(text(opencode_i18n::t!("session.quote.add")).font(QUOTE_BUTTON).color(p.text))
}

fn quote_toolbar_shell(p: &Pal, button: El) -> El {
    row()
        .key("quote-toolbar")
        .role("toolbar")
        .label_static(opencode_i18n::t!("session.quote.actions"))
        .padding(2.0)
        .gap(2.0)
        .center_y()
        .radius(8.0)
        .fill(composer_fill(p))
        .box_shadow(Pal::ring_out(p.ring))
        .shadow(2.0, 4.0, p.shadow_far)
        .shadow(1.0, 2.0, p.shadow)
        .transition(Transition::none())
        .motion(Spring::NONE)
        .opacity_motion(INSTANT)
        .keeps_text_selection()
        .child(button)
}
