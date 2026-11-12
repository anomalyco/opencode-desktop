//! High-throughput Rune terminal grid renderer (`120 fps`, per-row `RowId` `memo` + `GlyphCache`,
//! box-drawing vector shapes, selection, interactive scrollback scrollbar, and phase-locked cursor).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use rune::{
    Anim, Color, Cursor, Drag, El, Family, GridRow, KeyEvent, LineCap, Phase, PointerButton, PointerEvent, Shape, Span, TextStyle, Tx, WheelEvent,
    canvas, col, el, rich, row, spacer, text,
};
use rune_knobs::Knobs;
use terminal::{
    CellFlags, CursorShape, GhosttyKey, GridPoint, KeyInput, MouseBtn, MouseInput, MouseKind, RenderedCell, ScrollOp,
    SelectionOp, TerminalRow, UnderlineKind,
};

use super::find;
use super::mount::TextDrag;
use super::draw::{draw_box_or_block, draw_special_underline};
use super::{TERM_FONT, TERM_SIZE, TerminalTab};
use crate::App;
use crate::palette::Pal;
use crate::view::{UI, UI_SMALL};

/// The terminal's screen: its Tab stop and the key that tells the terminal it has the keyboard.
pub(crate) const VIEWPORT_KEY: &str = "terminal-viewport";

const INSTANT: Anim = Anim::INSTANT;

/// Rows key: the terminal row's id and content hash, the font and cell, and the theme.
type RowKey = (u128, u64, (u8, u32, u32, u32), bool);

thread_local! {
    /// Each terminal's shaped rows from its last frame, by tab: a row that is still on screen
    /// unchanged is handed back to the grid as it was.
    static GRID_ROWS: RefCell<HashMap<String, HashMap<RowKey, Rc<GridRow>>>> = RefCell::new(HashMap::new());
}
/// Desktop's panel strip (`--tabs-bar-height`).
pub const BAR_H: f32 = 48.0;
/// The terminal's inset in Desktop's panel (`px-[14px] py-3`).
pub const PAD_X: f32 = 14.0;
pub const PAD_Y: f32 = 12.0;
const SCROLLBAR_W: f32 = 8.0;

/// Desktop's `terminal` icon (`additional-icons.ts`): a prompt in a box.
pub(crate) static TERMINAL_ICON: rune::Icon = rune::Icon {
    name: "terminal-tab",
    view: 20.0,
    body: r#"<path d="M6.5 8L8.64286 10L6.5 12M10.9286 12H13.5M2 18H18V2H2V18Z" stroke="currentColor" stroke-linecap="square" fill="none"/>"#,
};

thread_local! {
    static METRIC_CACHE: RefCell<HashMap<(u8, u32, u64), (f32, f32, f32)>> = RefCell::new(HashMap::new());
}

thread_local! {
    /// Settings › Appearance › Terminal Font's installed family, before the default face, as a
    /// family list; `None` for one of the knob's faces.
    static CUSTOM: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}

/// Puts installed family `name` (empty for none) before the terminal's default face. Each
/// distinct name is kept for the app's life, as a text style names its family.
pub fn set_custom_font(name: &str) {
    thread_local! {
        static KEPT: RefCell<HashMap<String, &'static str>> = RefCell::new(HashMap::new());
    }
    let list = (!name.is_empty()).then(|| {
        KEPT.with_borrow_mut(|kept| *kept.entry(name.to_owned()).or_insert_with(|| {
            Box::leak(format!("{}, JetBrainsMono Nerd Font Mono", crate::appearance::css_family(name)).into_boxed_str())
        }))
    });
    CUSTOM.set(list);
}

pub fn terminal_font_style(knobs: &Knobs) -> (u8, TextStyle) {
    let custom = CUSTOM.get();
    // A typed family measures apart from the knob's faces.
    let choice = if custom.is_some() { 200 } else { knobs.get(&TERM_FONT).round() as u8 };
    let size = knobs.get(&TERM_SIZE).clamp(10.0, 22.0);
    // ghostty-web's cell: the face's ascent and descent, 18 px at Desktop's 14 px.
    let lh = (size * 1.3).round();
    let family = match choice {
        _ if custom.is_some() => Family::Named(custom.unwrap_or_default()),
        // By name, so the code font (the mono slot) doesn't change the terminal's.
        1 => Family::Named("IBM Plex Mono"),
        2 => Family::SystemMono,
        3 => Family::Named("JetBrainsMono Nerd Font Mono"),
        _ => Family::Named("CommitMono"),
    };
    let mut st = TextStyle::new(size, 400.0).lh(lh);
    st.family = family;
    // Disable ligatures/contextual alternates that could shift cell widths on grid runs.
    st.features = "'calt' 0, 'liga' 0";
    (choice, st)
}

pub fn cell_metrics(choice: u8, style: TextStyle) -> (f32, f32) {
    cell_metrics_at(choice, style, 1.0)
}

/// The cell at `scale` device pixels a point, as ghostty-web measures it (`renderer.ts`
/// `measureFont`): the advance of "M" and the face's ascent plus descent, each rounded to whole
/// device pixels, so the grid stays integral. At 14 pt on a 1x screen JetBrains Mono's 8.4 pt
/// advance makes an 8 pt cell, 8.67 at 1.5x.
pub fn cell_metrics_at(choice: u8, style: TextStyle, scale: f32) -> (f32, f32) {
    let (cw, ch, _) = measure(choice, style, scale);
    (cw, ch)
}

/// The spacing that sets the face's glyphs one cell apart: the cell less the face's advance.
pub fn cell_tracking(choice: u8, style: TextStyle, scale: f32) -> f32 {
    let (cw, _, advance) = measure(choice, style, scale);
    cw - advance
}

fn measure(choice: u8, style: TextStyle, scale: f32) -> (f32, f32, f32) {
    // A font switch (Settings › Appearance) can change what a style's family resolves to.
    // A typed family measures apart from the knob's faces and from another typed family.
    let family = match style.family { Family::Named(list) => list.as_ptr() as u64, _ => 0 };
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let key = (choice, style.size.to_bits() ^ scale.to_bits().rotate_left(16), rune::text::font_epoch() ^ family);
    METRIC_CACHE.with(|c| {
        if let Some(&m) = c.borrow().get(&key) {
            return m;
        }
        let style = style.tracking(0.0);
        let advance = rune::text::advance("MMMMMMMMMM", style) / 10.0;
        let advance = if advance > 1.0 { advance } else { (style.size * 0.6).max(6.0) };
        let cw = (advance * scale).round().max(1.0) / scale;
        // JetBrains Mono's font box (hhea 1020 up, 300 down), each side rounded as the canvas
        // reports it; another face keeps its line height.
        let ch = if choice == 3 {
            let px = style.size * scale;
            ((px * 1.02).round() + (px * 0.3).round()).max(1.0) / scale
        } else {
            style.line_px().max(style.size.ceil() + 2.0)
        };
        let m = (cw, ch, advance);
        c.borrow_mut().insert(key, m);
        m
    })
}

/// A terminal's view in its pane (Desktop's `TerminalPanel`): the selected terminal `tab_id` (the
/// workspace's active one without it) inset 14 by 12, or the notice while the session's terminals
/// load or failed to. Its tab lives in the side region's shared strip.
pub fn render_terminal_pane(
    s: &App,
    p: &Pal,
    ws_key: &str,
    tab_id: Option<&str>,
    pane_size: (f32, f32),
    env_scale: f32,
    focused: bool,
    shown: bool,
) -> El {
    let (font_choice, style) = terminal_font_style(&s.knobs);
    // ghostty-web's whole-device-pixel cell, the glyphs spaced one cell apart. `env_scale` is the
    // view's, at the app's zoom.
    let scale = env_scale;
    let (cell_w, cell_h) = cell_metrics_at(font_choice, style, scale);
    let style = style.tracking(cell_tracking(font_choice, style, scale)).lh(cell_h);
    let (pane_w, pane_h) = (pane_size.0.max(200.0), pane_size.1.max(100.0));
    let body_w = (pane_w - PAD_X * 2.0).max(cell_w * 10.0);
    let body_h = (pane_h - PAD_Y * 2.0).max(cell_h * 2.0);
    let fit_cols = ((body_w / cell_w).floor() as u16).clamp(10, 500);
    let fit_rows = ((body_h / cell_h).floor() as u16).clamp(2, 240);
    let cw_px = ((cell_w * scale).round() as u32).max(1);
    let ch_px = ((cell_h * scale).round() as u32).max(1);

    let Some(ws) = s.terminal.workspaces.get(ws_key) else {
        return col().key("terminal-empty").w_full().h_full().fill(p.bg_base).child(notice(p, opencode_i18n::t!("plugin.terminal.loading"), None));
    };
    if ws.tabs.is_empty()
        && let Some(err) = &ws.failure
    {
        return col().key("terminal-failed").w_full().h_full().fill(p.bg_base).child(notice(p, opencode_i18n::t!("plugin.terminal.list.failed"), Some(err)));
    }

    let Some(tab) = tab_id
        .or(ws.active.as_deref())
        .and_then(|id| ws.tabs.iter().find(|t| t.id == id))
        .or_else(|| ws.tabs.first())
    else {
        return col().key("terminal-no-active").w_full().h_full().fill(p.bg_base);
    };

    // Sync grid fit when the pane's layout dimensions change.
    tab.ensure_fit(fit_cols, fit_rows, cw_px, ch_px);

    // A pane kept mounted out of sight (a closed region keeps its card for the slide) draws the
    // last screen without reading a new one, so its terminal builds none (`publish.rs`).
    let snap = if shown { tab.session.snapshot() } else { tab.session.peek() };
    let viewport = render_viewport(
        s,
        p,
        ws_key,
        tab,
        &snap,
        style,
        font_choice,
        cell_w,
        cell_h,
        fit_cols,
        body_h,
        focused,
    );

    // A stopped attachment keeps its last screen under Desktop's notice (`text-12-regular
    // text-text-danger-base`, `px-6 py-3` on the terminal's background).
    let failure = match tab.session.status() {
        terminal::SessionStatus::Failed(why) => Some(why),
        _ => None,
    };
    let find = find::bar(p, ws_key, tab, &snap);
    let body = col().key("terminal-body").w_full().grow(1.0).min_h(0.0).child(viewport).children(find).children(failure.map(|why| {
        el().key("terminal-failure")
            .absolute(0.0, 0.0)
            .w_full()
            .px(24.0)
            .py(12.0)
            .fill(p.bg_base)
            .inert()
            // Desktop's `text-text-danger-base` names no token, so the notice keeps the pane's
            // strong ink.
            .child(text(why).font(UI_SMALL).color(p.text))
    }));

    // A bell flashes the pane's top edge in the accent.
    let bell = *s.clock < s.terminal.bell_until;
    col()
        .key("terminal-pane")
        .role("region")
        .label(opencode_i18n::t!("plugin.terminal.tab.title"))
        .w_full()
        .h_full()
        .fill(p.bg_base)
        .instant_changes()
        .children(bell.then(|| el().key("terminal-bell").absolute(0.0, 0.0).w_full().h(1.0).fill(p.accent)))
        .child(body)
}

/// Desktop's centred panel notice: a 13 px strong line over a 12 px weak one.
fn notice(p: &Pal, title: &str, detail: Option<&str>) -> El {
    col()
        .key("terminal-notice")
        .w_full()
        .grow(1.0)
        .center()
        .gap(4.0)
        .px(16.0)
        .child(text(title.to_owned()).font(UI).color(if detail.is_some() { p.text } else { p.faint }))
        .children(detail.map(|d| text(d.to_owned()).font(UI_SMALL).color(p.faint)))
}

#[allow(clippy::too_many_arguments)]
fn render_viewport(
    s: &App,
    p: &Pal,
    ws_key: &str,
    tab: &TerminalTab,
    snap: &terminal::TerminalSnapshot,
    style: TextStyle,
    font_choice: u8,
    cell_w: f32,
    cell_h: f32,
    fit_cols: u16,
    body_h: f32,
    focused: bool,
) -> El {
    let style_key = (font_choice, style.size.to_bits(), cell_w.to_bits(), cell_h.to_bits());
    let avail_h = body_h.max(cell_h);
    let slot_rows = (((avail_h / cell_h).ceil() as usize) + 1).max(snap.screen_rows.len());
    let clip_w = f32::from(fit_cols) * cell_w;
    let grid_h = (slot_rows as f32) * cell_h;

    // The screen is one element (`rune::grid`): each row is shaped once and kept while the
    // terminal's row (its id and content hash) is the same.
    let cols = snap.screen_rows.first().map_or(fit_cols, |r| r.cells.len() as u16);
    let rows = GRID_ROWS.with_borrow_mut(|cache| {
        // A closed terminal's rows go with it.
        cache.retain(|id, _| id == &tab.id || s.terminal.workspaces.values().any(|ws| ws.tabs.iter().any(|t| &t.id == id)));
        let cache = cache.entry(tab.id.clone()).or_default();
        let mut kept = HashMap::with_capacity(slot_rows);
        let key_of = |id: u128, hash: u64| (id, hash, style_key, p.dark);
        let rows: Vec<Rc<GridRow>> = (0..slot_rows)
            .map(|idx| {
                let key = snap.screen_rows.get(idx).map_or(key_of(0, 0), |r| key_of(r.id, r.content_hash));
                let row = cache.remove(&key).or_else(|| kept.get(&key).cloned()).unwrap_or_else(|| {
                    Rc::new(snap.screen_rows.get(idx).map_or_else(|| GridRow::new(String::new(), Vec::new(), style), |r| grid_row(r, p, cell_w, cell_h, style)))
                });
                kept.insert(key, row.clone());
                row
            })
            .collect();
        *cache = kept;
        rows
    });
    let rows_col = rune::grid(cols, cell_w, cell_h, rows).key("terminal-rows").w(clip_w).h(grid_h);
    let mut canvas_layer = el()
        .key(format!("terminal-canvas-{}", tab.id))
        .w(clip_w)
        .h(avail_h)
        .clip_soft(0.0)
        // Find's matches in view, under the text: the session's Find marks (`hit_bg`'s accent at 0.28), the
        // selected one in the accent at half strength so the terminal's text stays readable.
        .children(snap.find.iter().flat_map(|f| f.hits.iter()).enumerate().map(|(i, hit)| {
            let (x, y) = (f32::from(hit.start) * cell_w, f32::from(hit.row) * cell_h);
            let w = f32::from(hit.end.saturating_sub(hit.start) + 1) * cell_w;
            let fill = if hit.selected { p.accent.alpha(0.5) } else { p.accent.alpha(0.28) };
            el().key(format!("terminal-find-hit-{i}")).absolute(x, y).size(w, cell_h).fill(fill).inert()
        }))
        .child(rows_col);

    // Phase-locked cursor overlay (kept mounted with transparent fill during off-half so motion
    // audits never see an element pop-in/pop-out on 500 ms blink edges).
    if snap.cursor.visible && snap.cursor.in_viewport {
        let lit = if focused && snap.cursor.blinking {
            s.terminal.cursor_lit(*s.clock)
        } else {
            true
        };
        canvas_layer = canvas_layer.child(render_cursor(p, snap, cell_w, cell_h, focused, lit));
    }

    // The link under the pointer, underlined in its own colour (Windows Terminal and Ghostty do);
    // with Ctrl (⌘ on macOS) held the pointer says a click follows it.
    let hovered = if snap.mouse_tracking { Vec::new() } else { tab.hover.map(|(c, r, _)| super::links::span(snap, c, r)).unwrap_or_default() };
    let follows = !hovered.is_empty() && tab.hover.is_some_and(|h| h.2);
    canvas_layer = canvas_layer.children(hovered.iter().map(|&(r, a, b)| {
        let color = snap.screen_rows.get(usize::from(r)).and_then(|row| row.cells.get(usize::from(a))).map_or(p.text, cell_fg_color);
        let y = f32::from(r + 1) * cell_h - 2.0;
        el().key(format!("terminal-link-{r}")).absolute(f32::from(a) * cell_w, y).size(f32::from(b - a) * cell_w, 1.0).fill(color).inert()
    }));

    // Quote: a settled selection offers to go into the composer, under its last line (above
    // when that's the screen's bottom), as Desktop's `TerminalQuote` does.
    if matches!(tab.drag, TextDrag::Idle)
        && let Some(text) = tab.session.selected_text()
        && crate::quote::can_quote(s, &text)
        && let Some((left, last)) = selection_corner(snap)
    {
        let below = f32::from(last + 1) * cell_h + 6.0;
        let y = if below + 28.0 > avail_h { (f32::from(last) * cell_h - 34.0).max(0.0) } else { below };
        let x = (f32::from(left) * cell_w).min((clip_w - 90.0).max(0.0));
        let (wk, tid) = (ws_key.to_owned(), tab.id.clone());
        canvas_layer = canvas_layer.child(
            el().key("terminal-quote").absolute(x, y).child(crate::quote::bar_with(p, false, move |app: &mut App, tx: &mut Tx| {
                let Some(t) = app.terminal_tab_mut(&wk, &tid) else { return };
                let Some(text) = t.session.selected_text() else { return };
                t.session.select(SelectionOp::Clear);
                crate::quote::quote_selected(app, &text, tx);
            })),
        );
    }

    // The input method's caret (dead keys, CJK, the emoji panel, dictation) at the cursor's cell,
    // whether or not the program shows its cursor; a password prompt keeps the input method off.
    if focused && !snap.cursor.password_input {
        let (col, row) = if snap.cursor.in_viewport { (snap.cursor.col, snap.cursor.row) } else { (0, snap.rows.saturating_sub(1)) };
        let (x, y) = (f32::from(col) * cell_w, f32::from(row) * cell_h);
        canvas_layer = canvas_layer
            .child(el().key("terminal-ime").absolute(x, y).size(cell_w, cell_h).inert().ime_cursor())
            .children(tab.preedit.as_ref().map(|pre| render_preedit(pre, snap, x, y, cell_h, style)));
    }

    let wk_drag = ws_key.to_owned();
    let tid_drag = tab.id.clone();
    let cols_cap = snap.cols.max(1);
    let rows_cap = snap.rows.max(1);
    let cell = move |x: f32, y: f32| {
        let (lx, ly) = ((x - PAD_X).max(0.0), (y - PAD_Y).max(0.0));
        let col = ((lx / cell_w).floor() as u16).min(cols_cap.saturating_sub(1));
        let row = ((ly / cell_h).floor() as u16).min(rows_cap.saturating_sub(1));
        (lx, ly, col, row)
    };
    // A program that asked for the mouse (`?1000`-`?1003`, on either screen: tmux, fzf --height,
    // vim) gets it; Shift keeps the pointer for selecting, as in xterm and every terminal since.
    let reports = snap.mouse_tracking;

    let wk_wheel = ws_key.to_owned();
    let tid_wheel = tab.id.clone();
    let (wk_move, tid_move) = (ws_key.to_owned(), tab.id.clone());
    let (wk_menu, tid_menu) = (ws_key.to_owned(), tab.id.clone());

    let title = tab.title.clone();
    let mut outer = row()
        .key(VIEWPORT_KEY)
        .role("document")
        // A Tab stop that owns its keys while focused (Tab included); F6 leaves it.
        .focusable(true)
        .label(opencode_i18n::t!("rune.a11y.terminal", title = title))
        .description(opencode_i18n::t!("rune.a11y.terminal.leave"))
        .on_key(|app: &mut App, ev: &KeyEvent, tx: &mut Tx| {
            if app.terminal_focus().is_none() {
                app.focus_terminal_surface();
            }
            super::key_terminal(app, ev, tx)
        })
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .pl(PAD_X)
        .pr(PAD_X - SCROLLBAR_W)
        .pt(PAD_Y)
        .pb(PAD_Y)
        .cursor(if follows { Cursor::Pointer } else { Cursor::Text })
        .pointer_buttons([PointerButton::Middle])
        .on_drag(move |app: &mut App, d: &mut Drag, tx: &mut Tx| {
            tx.instant();
            app.focus_terminal_surface();
            let (lx, ly, col, row) = cell(d.x, d.y);
            let mods = tx.mods();
            let button = tx.button();
            let Some(t) = app.terminal_tab_mut(&wk_drag, &tid_drag) else {
                return;
            };
            // A press takes control of a shared terminal at this pane's size (interacted).
            if matches!(d.phase, Phase::Down) {
                t.session.interact();
            }
            if reports && !mods.shift {
                let kind = match d.phase {
                    Phase::Down => MouseKind::Press,
                    Phase::Move => MouseKind::Move,
                    Phase::Up => MouseKind::Release,
                };
                let button = if button == PointerButton::Middle { MouseBtn::Middle } else { MouseBtn::Left };
                t.session.send_mouse(MouseInput { kind, button, x_px: lx, y_px: ly, shift: false, ctrl: mods.ctrl, alt: mods.alt, cmd: crate::keymap::logo_held(mods) });
                return;
            }
            if button != PointerButton::Primary {
                return;
            }
            match d.phase {
                Phase::Down => {
                    t.drag = TextDrag::Selecting { anchor: (col, row) };
                    match d.clicks {
                        2 => t.session.select(SelectionOp::Word(GridPoint::Viewport { col, row })),
                        3.. => t.session.select(SelectionOp::Line(GridPoint::Viewport { col, row })),
                        _ => t.session.select(SelectionOp::Clear),
                    }
                }
                Phase::Move => {
                    if (d.dx.hypot(d.dy)) > 3.0
                        && let TextDrag::Selecting { anchor: start } = t.drag
                    {
                        // Alt drags a block, as in Windows Terminal, iTerm2 and Ghostty.
                        t.session.select_range_viewport(start, (col, row), mods.alt);
                    }
                }
                Phase::Up => {
                    t.drag = TextDrag::Idle;
                    // Ctrl+click (⌘-click on macOS) follows a link: the cell's OSC 8 target, else
                    // an address or a file reference in the line under the pointer.
                    let follow = if cfg!(target_os = "macos") { mods.cmd } else { mods.ctrl };
                    if d.clicks == 1 && d.dx.hypot(d.dy) <= 3.0 && follow {
                        super::links::follow(app, &wk_drag, &tid_drag, col, row, tx);
                    }
                }
            }
        })
        .on_wheel(move |app: &mut App, ev: WheelEvent, tx: &mut Tx| {
            tx.instant();
            let Some(t) = app.terminal_tab_mut(&wk_wheel, &tid_wheel) else {
                return false;
            };
            let snap = t.session.snapshot();
            let step = if ev.precise { cell_h } else { cell_h * 0.5 }.max(4.0);
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
                    cmd: crate::keymap::logo_held(ev.mods),
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
        })
        .on_secondary_press_local(move |app: &mut App, p: PointerEvent, tx: &mut Tx| {
            let (lx, ly, _, _) = cell(p.x, p.y);
            let Some(t) = app.terminal_tab_mut(&wk_menu, &tid_menu) else { return };
            // A program reading the mouse gets the right button (press and release together: the
            // release isn't heard apart); otherwise the terminal's menu.
            if t.session.snapshot().mouse_tracking && !tx.mods().shift {
                for kind in [MouseKind::Press, MouseKind::Release] {
                    t.session.send_mouse(MouseInput { kind, button: MouseBtn::Right, x_px: lx, y_px: ly, shift: false, ctrl: false, alt: false, cmd: false });
                }
                return;
            }
            super::menu::open(app, &wk_menu, &tid_menu, p.window(), tx);
        });
    // Pointer motion with no button held: for a program that asked for every move (`?1003`; the
    // encoder sends it only in that mode), else the link under the pointer, which the view
    // underlines. A move that leaves the same link (or none) under it draws nothing new.
    let (wk_leave, tid_leave) = (ws_key.to_owned(), tab.id.clone());
    outer = outer
        .on_pointer_move_local(move |app: &mut App, p: PointerEvent, tx: &mut Tx| {
            let (lx, ly, col, row) = cell(p.x, p.y);
            let mods = tx.mods();
            let Some(t) = app.terminal_tab_mut(&wk_move, &tid_move) else { return };
            if reports && !mods.shift {
                t.session.send_mouse(MouseInput { kind: MouseKind::Move, button: MouseBtn::None, x_px: lx, y_px: ly, shift: false, ctrl: mods.ctrl, alt: mods.alt, cmd: false });
                tx.live_only();
                return;
            }
            let follow = if cfg!(target_os = "macos") { mods.cmd } else { mods.ctrl };
            let snap = t.session.peek();
            let now = super::links::span(&snap, col, row);
            let was = t.hover.map(|(c, r, _)| super::links::span(&snap, c, r)).unwrap_or_default();
            let drawn_same = now == was && (now.is_empty() || t.hover.is_some_and(|h| h.2 == follow));
            t.hover = Some((col, row, follow));
            if drawn_same {
                tx.live_only();
            }
        })
        .on_hover_change(move |app: &mut App, inside: bool, _tx: &mut Tx| {
            if !inside && let Some(t) = app.terminal_tab_mut(&wk_leave, &tid_leave) {
                t.hover = None;
            }
        });
    // Finished commands (the shell's OSC 133 marks): a dot in the margin beside each one's prompt,
    // the success colour for exit 0 and the danger colour for any other, saying how it ended.
    let dot = 6.0;
    let marks = snap.marks.iter().map(|&(row, mark)| {
        let took = took(mark.ms);
        let (color, said) = match mark.exit {
            Some(0) => (p.success, opencode_i18n::t!("rune.terminal.mark.ok", duration = took)),
            Some(code) => (p.danger, opencode_i18n::t!("rune.terminal.mark.failed", code = code.to_string(), duration = took)),
            None => (p.muted, opencode_i18n::t!("rune.terminal.mark.finished", duration = took)),
        };
        el().key(format!("terminal-mark-{row}"))
            .absolute((PAD_X - dot) / 2.0, PAD_Y + f32::from(row) * cell_h + (cell_h - dot) / 2.0)
            .size(dot, dot)
            .radius(dot / 2.0)
            .fill(color)
            .role("image")
            .label(said.clone())
            .help(said)
    });
    let mut outer = outer
        .children(marks)
        .child(canvas_layer)
        .child(spacer());

    if snap.scrollbar.total > snap.scrollbar.len && snap.scrollbar.total > 0 {
        outer = outer.child(render_scrollbar(
            p,
            ws_key,
            &tab.id,
            &snap.scrollbar,
            body_h,
        ));
    } else {
        outer = outer.child(el().w(SCROLLBAR_W).h_full());
    }

    outer
}

/// A command's run time as the mark says it: `340 ms`, `2.4 s`, `3 min 5 s`.
fn took(ms: u32) -> String {
    match ms {
        0..1_000 => format!("{ms} ms"),
        1_000..60_000 => format!("{:.1} s", f64::from(ms) / 1_000.0),
        _ => format!("{} min {} s", ms / 60_000, ms / 1_000 % 60),
    }
}

fn render_cursor(
    p: &Pal,
    snap: &terminal::TerminalSnapshot,
    cell_w: f32,
    cell_h: f32,
    focused: bool,
    lit: bool,
) -> El {
    let x = f32::from(snap.cursor.col) * cell_w;
    let y = f32::from(snap.cursor.row) * cell_h;
    let cur_w = if snap.cursor.wide_tail { cell_w * 2.0 } else { cell_w };
    let color = snap.cursor_color.to_rune();

    let mut shape = Shape::new();
    if !focused || snap.cursor.shape == CursorShape::HollowBlock {
        if !focused || lit {
            let stroke = p.text.alpha(0.75);
            shape = shape.stroke_with(
                vec![
                    [0.5, 0.5],
                    [cur_w - 0.5, 0.5],
                    [cur_w - 0.5, cell_h - 0.5],
                    [0.5, cell_h - 0.5],
                ],
                true,
                1.0,
                LineCap::Butt,
                stroke,
            );
        }
    } else if lit {
        match snap.cursor.shape {
            CursorShape::Bar => {
                shape = shape.fill(
                    vec![[0.0, 0.0], [2.0, 0.0], [2.0, cell_h], [0.0, cell_h]],
                    color,
                );
            }
            CursorShape::Underline => {
                let uy = (cell_h - 2.0).max(0.0);
                shape = shape.fill(
                    vec![[0.0, uy], [cur_w, uy], [cur_w, cell_h], [0.0, cell_h]],
                    color,
                );
            }
            CursorShape::Block | CursorShape::HollowBlock => {
                shape = shape.fill(
                    vec![[0.0, 0.0], [cur_w, 0.0], [cur_w, cell_h], [0.0, cell_h]],
                    color.alpha(0.42),
                );
            }
        }
    }

    canvas(shape)
        .key("terminal-cursor")
        .absolute(x, y)
        .size(cur_w, cell_h)
        .instant_changes()
        .transition(rune::Transition::none())
        .inert()
}

/// The selection's last row and the first selected column on it.
fn selection_corner(snap: &terminal::TerminalSnapshot) -> Option<(u16, u16)> {
    snap.screen_rows.iter().enumerate().rev().find_map(|(r, row)| row.bg_spans.iter().find(|s| s.selected).map(|s| (s.start_col, r as u16)))
}

/// The composition over the cells from the cursor on, on the terminal's background so the cells
/// under it don't show through, underlined as marked text is, with a caret where the input
/// method puts it.
fn render_preedit(pre: &super::input::Preedit, snap: &terminal::TerminalSnapshot, x: f32, y: f32, cell_h: f32, style: TextStyle) -> El {
    let fg = snap.fg.to_rune();
    let caret = pre.cursor.map(|(start, _)| rune::text::advance(pre.text.get(..start).unwrap_or(&pre.text), style));
    let mut span = Span::new(0, pre.text.len()).color(fg);
    span.underline = Some(fg);
    el().key("terminal-preedit")
        .absolute(x, y)
        .h(cell_h)
        .fill(snap.bg.to_rune())
        .inert()
        .child(rich(pre.text.clone(), vec![span]).font(style))
        .children(caret.map(|cx| el().key("terminal-preedit-caret").absolute(cx, 0.0).size(1.0, cell_h).fill(snap.cursor_color.to_rune())))
}

fn render_scrollbar(
    p: &Pal,
    ws_key: &str,
    tab_id: &str,
    sb: &terminal::ScrollbarSnapshot,
    track_h: f32,
) -> El {
    let total = (sb.total as f32).max(1.0);
    let len = (sb.len as f32).min(total);
    let thumb_h = ((len / total) * track_h).clamp(20.0, track_h);
    let max_offset = sb.total.saturating_sub(sb.len).max(1) as f32;
    let travel = (track_h - thumb_h).max(0.0);
    let thumb_y = ((sb.offset as f32 / max_offset) * travel).clamp(0.0, travel);

    let wk = ws_key.to_owned();
    let tid = tab_id.to_owned();
    let max_row = sb.total.saturating_sub(sb.len);

    el().key("terminal-scrollbar")
        .w(SCROLLBAR_W)
        .h(track_h)
        .cursor(Cursor::Default)
        .on_drag(move |app: &mut App, d: &mut Drag, tx: &mut Tx| {
            tx.instant();
            match d.phase {
                Phase::Down => {
                    let frac = ((d.y - thumb_h * 0.5) / travel.max(1.0)).clamp(0.0, 1.0);
                    let target = (frac * max_row as f32).round() as usize;
                    if let Some(t) = app.terminal_tab_mut(&wk, &tid) {
                        t.session.scroll(ScrollOp::Row(target));
                    }
                    d.data = target as f32;
                }
                Phase::Move => {
                    let frac = ((d.y - thumb_h * 0.5) / travel.max(1.0)).clamp(0.0, 1.0);
                    let target = (frac * max_row as f32).round() as usize;
                    if let Some(t) = app.terminal_tab_mut(&wk, &tid) {
                        t.session.scroll(ScrollOp::Row(target));
                    }
                }
                Phase::Up => {}
            }
        })
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

/// A terminal row as the grid draws it: background and selection runs, the narrow cells' text
/// in one styled line, cells that must sit exactly on their column (wide, emoji, other scripts)
/// on their own, and box drawing, curly and double underlines and strikethrough as vectors.
fn grid_row(row_data: &TerminalRow, p: &Pal, cell_w: f32, cell_h: f32, base_style: TextStyle) -> GridRow {
    let mut shape = Shape::new();
    let mut line_text = String::with_capacity(row_data.cells.len());
    let mut line_spans: Vec<Span> = Vec::new();
    let mut pinned: Vec<(u16, u16, String, TextStyle, Color)> = Vec::new();

    for (idx, cell) in row_data.cells.iter().enumerate() {
        let col_idx = idx as u16;
        if cell.flags.contains(CellFlags::WIDE_SPACER_TAIL) || cell.flags.contains(CellFlags::INVISIBLE) {
            line_text.push(' ');
            continue;
        }
        let fg = cell_fg_color(cell);
        if cell.flags.contains(CellFlags::STRIKETHROUGH) {
            let x0 = f32::from(col_idx) * cell_w;
            let x1 = x0 + f32::from(cell.width.max(1)) * cell_w;
            let ym = (cell_h * 0.52).round();
            shape = shape.line([x0, ym], [x1, ym], 1.0, LineCap::Butt, fg);
        }
        if !matches!(cell.underline, UnderlineKind::None | UnderlineKind::Single) {
            let ul_c = cell.underline_color.map_or(fg, |c| c.to_rune());
            shape = draw_special_underline(shape, cell.underline, f32::from(col_idx) * cell_w, f32::from(cell.width.max(1)) * cell_w, cell_h, ul_c);
        }
        if let Some(ch) = single_char(&cell.grapheme)
            && draw_box_or_block(&mut shape, ch, f32::from(col_idx) * cell_w, cell_w, cell_h, fg)
        {
            line_text.push(' ');
            continue;
        }
        // Wide glyphs, emoji and other scripts sit exactly on their cells, so a fallback font's
        // advances can't move the columns after them.
        if cell.width != 1 || !is_grid_safe_monospaced(&cell.grapheme) {
            line_text.push(' ');
            if !cell.grapheme.is_empty() && cell.grapheme != " " {
                let mut cell_style = base_style;
                if cell.flags.contains(CellFlags::BOLD) {
                    cell_style.weight = 700.0;
                }
                if cell.flags.contains(CellFlags::ITALIC) {
                    cell_style.italic = true;
                }
                pinned.push((col_idx, u16::from(cell.width.max(1)), cell.grapheme.to_string(), cell_style, fg));
            }
            continue;
        }
        let byte_start = line_text.len();
        if cell.grapheme.is_empty() {
            line_text.push(' ');
        } else {
            line_text.push_str(&cell.grapheme);
        }
        let byte_end = line_text.len();
        let weight = cell.flags.contains(CellFlags::BOLD).then_some(700.0);
        let italic = cell.flags.contains(CellFlags::ITALIC);
        let underline = (cell.underline == UnderlineKind::Single).then(|| cell.underline_color.map_or(fg, |c| c.to_rune()));
        if let Some(last) = line_spans.last_mut()
            && last.end as usize == byte_start
            && last.color == Some(fg)
            && last.weight == weight
            && last.italic == italic
            && last.underline == underline
        {
            last.end = byte_end as u32;
        } else {
            let mut sp = Span::new(byte_start, byte_end).color(fg);
            sp.weight = weight;
            sp.italic = italic;
            sp.underline = underline;
            line_spans.push(sp);
        }
    }
    let trimmed_len = line_text.trim_end_matches(' ').len();
    line_text.truncate(trimmed_len);
    line_spans.retain_mut(|sp| {
        if sp.start as usize >= trimmed_len {
            return false;
        }
        sp.end = sp.end.min(trimmed_len as u32);
        sp.end > sp.start
    });

    let mut row = GridRow::new(line_text, line_spans, base_style);
    for span in &row_data.bg_spans {
        if let Some(bg) = span.color {
            row = row.fill(span.start_col, span.end_col, bg.to_rune());
        }
        if span.selected {
            row = row.fill(span.start_col, span.end_col, p.selection);
        }
    }
    for (col, width, text, style, fg) in pinned {
        row = row.cell(col, width, text, style, fg);
    }
    row.shape(shape)
}

fn cell_fg_color(cell: &RenderedCell) -> Color {
    let c = cell.fg.to_rune();
    if cell.flags.contains(CellFlags::FAINT) {
        c.alpha(0.55)
    } else {
        c
    }
}

fn single_char(s: &str) -> Option<char> {
    let mut it = s.chars();
    let ch = it.next()?;
    it.next().is_none().then_some(ch)
}

fn is_grid_safe_monospaced(s: &str) -> bool {
    let Some(ch) = single_char(s) else { return false };
    ch.is_ascii_graphic() || ch == ' ' || ('\u{00A0}'..='\u{024F}').contains(&ch)
}
