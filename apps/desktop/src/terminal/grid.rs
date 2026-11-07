//! High-throughput Rune terminal grid renderer (`120 fps`, per-row `RowId` `memo` + `GlyphCache`,
//! box-drawing vector shapes, selection, interactive scrollback scrollbar, and phase-locked cursor).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use rune::{
    Anim, Color, Cursor, Drag, El, Family, LineCap, Phase, Shape, Span, TextStyle, Tx, WheelEvent,
    canvas, col, el, memo, rich, row, spacer, text,
};
use rune_knobs::Knobs;
use terminal::{
    CellFlags, CursorShape, GridPoint, MouseBtn, MouseInput, MouseKind, RenderedCell, ScrollOp,
    SelectionOp, TerminalRow, UnderlineKind,
};

use super::draw::{detect_url_in_row, draw_box_or_block, draw_special_underline};
use super::{TERM_FONT, TERM_SIZE, TerminalTab};
use crate::App;
use crate::palette::Pal;
use crate::view::{UI, UI_SMALL};

const INSTANT: Anim = Anim::INSTANT;
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
    static METRIC_CACHE: RefCell<HashMap<(u8, u32, u64), (f32, f32)>> = RefCell::new(HashMap::new());
}

pub fn terminal_font_style(knobs: &Knobs) -> (u8, TextStyle) {
    let choice = knobs.get(&TERM_FONT).round() as u8;
    let size = knobs.get(&TERM_SIZE).clamp(10.0, 22.0);
    // ghostty-web's cell: the face's ascent and descent, 18 px at Desktop's 14 px.
    let lh = (size * 1.3).round();
    let family = match choice {
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
    // A font switch (Settings › Appearance) can change what a style's family resolves to.
    let key = (choice, style.size.to_bits(), rune::text::font_epoch());
    METRIC_CACHE.with(|c| {
        if let Some(&m) = c.borrow().get(&key) {
            return m;
        }
        let cw = rune::text::advance("MMMMMMMMMM", style) / 10.0;
        let cw = if cw > 1.0 { cw } else { (style.size * 0.6).max(6.0) };
        let ch = style.line_px().max(style.size.ceil() + 2.0);
        let pair = (cw, ch);
        c.borrow_mut().insert(key, pair);
        pair
    })
}

/// How many cell sizes `cell_metrics` holds on this thread.
#[cfg(test)]
pub fn cached_metrics() -> usize {
    METRIC_CACHE.with(|c| c.borrow().len())
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
) -> El {
    let (font_choice, style) = terminal_font_style(&s.knobs);
    let (cell_w, cell_h) = cell_metrics(font_choice, style);
    let (pane_w, pane_h) = (pane_size.0.max(200.0), pane_size.1.max(100.0));
    let body_w = (pane_w - PAD_X * 2.0).max(cell_w * 10.0);
    let body_h = (pane_h - PAD_Y * 2.0).max(cell_h * 2.0);
    let fit_cols = ((body_w / cell_w).floor() as u16).clamp(10, 500);
    let fit_rows = ((body_h / cell_h).floor() as u16).clamp(2, 240);
    let cw_px = ((cell_w * s.zoom * env_scale).round() as u32).max(1);
    let ch_px = ((cell_h * s.zoom * env_scale).round() as u32).max(1);

    let Some(ws) = s.terminal.workspaces.get(ws_key) else {
        return col().key("terminal-empty").w_full().h_full().fill(p.bg_base).child(notice(p, "Loading terminal…", None));
    };
    if ws.tabs.is_empty()
        && let Some(err) = &ws.failure
    {
        return col().key("terminal-failed").w_full().h_full().fill(p.bg_base).child(notice(p, "Couldn't load terminals", Some(err)));
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

    let snap = tab.session.snapshot();
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
    let body = col().key("terminal-body").w_full().grow(1.0).min_h(0.0).child(viewport).children(failure.map(|why| {
        el().key("terminal-failure")
            .absolute(0.0, 0.0)
            .w_full()
            .px(24.0)
            .py(12.0)
            .fill(p.bg_base)
            .inert()
            .child(text(why).font(UI_SMALL).color(p.danger))
    }));

    // A bell flashes the pane's top edge in the accent.
    let bell = s.clock < s.terminal.bell_until;
    col()
        .key("terminal-pane")
        .role("region")
        .label("Terminal")
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

    let mut rows_col = col().key("terminal-rows").w(clip_w).h(grid_h);
    for idx in 0..slot_rows {
        let memo_id = format!("tr-{idx}");
        if let Some(row_arc) = snap.screen_rows.get(idx) {
            let version = (row_arc.id, row_arc.content_hash, style_key, p.dark, clip_w.to_bits());
            let cloned: Arc<TerminalRow> = row_arc.clone();
            let row_el = memo(memo_id, version, move || {
                render_row(&cloned, p, cell_w, cell_h, style)
            });
            rows_col = rows_col.child(row_el);
        } else {
            let empty_ver = (0u128, 0u64, style_key, p.dark, clip_w.to_bits());
            let w = clip_w;
            let row_el = memo(memo_id, empty_ver, move || {
                el().w(w)
                    .h(cell_h)
                    .child(el().key("line").absolute(0.0, 0.0).size(w, cell_h).child(rich(String::new(), Vec::new()).font(style)))
                    .child(canvas(Shape::new()).key("vec").absolute(0.0, 0.0).size(w, cell_h).inert())
            });
            rows_col = rows_col.child(row_el);
        }
    }

    let mut canvas_layer = el()
        .key(format!("terminal-canvas-{}", tab.id))
        .w(clip_w)
        .h(avail_h)
        .clip_soft(0.0)
        .child(rows_col);

    // Phase-locked cursor overlay (kept mounted with transparent fill during off-half so motion
    // audits never see an element pop-in/pop-out on 500 ms blink edges).
    if snap.cursor.visible && snap.cursor.in_viewport {
        let lit = if focused && snap.cursor.blinking {
            s.terminal.cursor_lit(s.clock)
        } else {
            true
        };
        canvas_layer = canvas_layer.child(render_cursor(p, snap, cell_w, cell_h, focused, lit));
    }

    let wk_drag = ws_key.to_owned();
    let tid_drag = tab.id.clone();
    let cols_cap = snap.cols.max(1);
    let rows_cap = snap.rows.max(1);
    let alt_mouse = snap.alt_screen && snap.mouse_tracking;

    let wk_wheel = ws_key.to_owned();
    let tid_wheel = tab.id.clone();

    let mut outer = row()
        .key("terminal-viewport")
        .role("document")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .pl(PAD_X)
        .pr(PAD_X - SCROLLBAR_W)
        .pt(PAD_Y)
        .pb(PAD_Y)
        .cursor(Cursor::Text)
        .on_drag(move |app: &mut App, d: &mut Drag, tx: &mut Tx| {
            tx.instant();
            app.focus_terminal_surface();
            let lx = (d.x - PAD_X).max(0.0);
            let ly = (d.y - PAD_Y).max(0.0);
            let col = ((lx / cell_w).floor() as u16).min(cols_cap.saturating_sub(1));
            let row = ((ly / cell_h).floor() as u16).min(rows_cap.saturating_sub(1));

            let Some(t) = app.terminal_tab_mut(&wk_drag, &tid_drag) else {
                return;
            };
            if alt_mouse {
                let kind = match d.phase {
                    Phase::Down => MouseKind::Press,
                    Phase::Move => MouseKind::Move,
                    Phase::Up => MouseKind::Release,
                };
                t.session.send_mouse(MouseInput {
                    kind,
                    button: MouseBtn::Left,
                    x_px: lx,
                    y_px: ly,
                    shift: false,
                    ctrl: false,
                    alt: false,
                    cmd: false,
                });
                return;
            }

            match d.phase {
                Phase::Down => {
                    t.drag_anchor = Some((col, row));
                    match d.clicks {
                        2 => t.session.select(SelectionOp::Word(GridPoint::Viewport { col, row })),
                        3.. => t.session.select(SelectionOp::Line(GridPoint::Viewport { col, row })),
                        _ => t.session.select(SelectionOp::Clear),
                    }
                }
                Phase::Move => {
                    if (d.dx.hypot(d.dy)) > 3.0
                        && let Some(start) = t.drag_anchor
                    {
                        t.session.select_range_viewport(start, (col, row), false);
                    }
                }
                Phase::Up => {
                    if d.clicks == 1 && d.dx.hypot(d.dy) <= 3.0 {
                        if let Some(url) = detect_url_in_row(&t.session.snapshot(), col, row)
                            && tx.mods().cmd
                        {
                            let _ = tx.open_url(&url);
                        }
                    }
                    t.drag_anchor = None;
                }
            }
        })
        .on_wheel(move |app: &mut App, ev: WheelEvent, tx: &mut Tx| {
            tx.instant();
            let Some(t) = app.terminal_tab_mut(&wk_wheel, &tid_wheel) else {
                return false;
            };
            let snap = t.session.snapshot();
            if snap.alt_screen && snap.mouse_tracking {
                if ev.dy.abs() < 0.5 {
                    return true;
                }
                let btn = if ev.dy > 0.0 {
                    MouseBtn::WheelUp
                } else {
                    MouseBtn::WheelDown
                };
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

            let step = if ev.precise { cell_h } else { cell_h * 0.5 }.max(4.0);
            t.scroll_px_accum += ev.dy;
            let lines = (t.scroll_px_accum / step).trunc() as isize;
            if lines != 0 {
                t.scroll_px_accum -= (lines as f32) * step;
                // Natural scrolling: positive `dy` moves content down, revealing older lines (`-lines`).
                t.session.scroll(ScrollOp::Delta(-lines));
            }
            true
        })
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

/// Renders one terminal row with coalesced background spans, vector box-drawing, and merged `rich` runs.
fn render_row(row_data: &TerminalRow, p: &Pal, cell_w: f32, cell_h: f32, base_style: TextStyle) -> El {
    let row_w = (row_data.cells.len() as f32) * cell_w;
    let mut container = el().w(row_w).h(cell_h);

    // 1. Coalesced cell background & selection spans.
    for span in &row_data.bg_spans {
        let x = f32::from(span.start_col) * cell_w;
        let w = f32::from(span.end_col.saturating_sub(span.start_col)) * cell_w;
        if let Some(bg) = span.color {
            container = container.child(el().absolute(x, 0.0).size(w, cell_h).fill(bg.to_rune()));
        }
        if span.selected {
            container = container.child(el().absolute(x, 0.0).size(w, cell_h).fill(p.selection));
        }
    }

    // 2. Single stable `line` rich-text node per row + pinned wide/emoji cells + vector `Shape`.
    // Keeping `line` always mounted prevents element pop-out/pop-in when rows wrap, scroll, or clear.
    let mut shape = Shape::new();

    let mut line_text = String::with_capacity(row_data.cells.len());
    let mut line_spans: Vec<Span> = Vec::new();
    let mut pinned_children: Vec<El> = Vec::new();

    for (idx, cell) in row_data.cells.iter().enumerate() {
        let col_idx = idx as u16;
        if cell.flags.contains(CellFlags::WIDE_SPACER_TAIL) || cell.flags.contains(CellFlags::INVISIBLE) {
            line_text.push(' ');
            continue;
        }

        let fg = cell_fg_color(cell);

        // Decorations (strikethrough + non-single underlines) go into the row's vector `Shape`.
        if cell.flags.contains(CellFlags::STRIKETHROUGH) {
            let x0 = f32::from(col_idx) * cell_w;
            let x1 = x0 + f32::from(cell.width.max(1)) * cell_w;
            let ym = (cell_h * 0.52).round();
            shape = shape.line([x0, ym], [x1, ym], 1.0, LineCap::Butt, fg);
        }
        if !matches!(cell.underline, UnderlineKind::None | UnderlineKind::Single) {
            let ul_c = cell.underline_color.map_or(fg, |c| c.to_rune());
            shape = draw_special_underline(
                shape,
                cell.underline,
                f32::from(col_idx) * cell_w,
                f32::from(cell.width.max(1)) * cell_w,
                cell_h,
                ul_c,
            );
        }

        if let Some(ch) = single_char(&cell.grapheme)
            && draw_box_or_block(&mut shape, ch, f32::from(col_idx) * cell_w, cell_w, cell_h, fg)
        {
            line_text.push(' ');
            continue;
        }

        // Wide glyphs, emoji, and non-Latin scripts get exact cell-pinned boxes so fallback font
        // advances can never drift subsequent columns off the monospace grid.
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
                let w = f32::from(cell.width.max(1)) * cell_w;
                pinned_children.push(
                    el().key(format!("g-{col_idx}"))
                        .absolute(f32::from(col_idx) * cell_w, 0.0)
                        .size(w, cell_h)
                        .child(text(cell.grapheme.clone()).font(cell_style).color(fg)),
                );
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
        let underline = (cell.underline == UnderlineKind::Single)
            .then(|| cell.underline_color.map_or(fg, |c| c.to_rune()));

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
            false
        } else {
            sp.end = sp.end.min(trimmed_len as u32);
            sp.end > sp.start
        }
    });

    container = container.child(
        el().key("line")
            .absolute(0.0, 0.0)
            .size(row_w, cell_h)
            .child(rich(line_text, line_spans).font(base_style).truncate_fade_width(0.0)),
    );

    for child in pinned_children {
        container = container.child(child);
    }
    container.child(
        canvas(shape)
            .key("vec")
            .absolute(0.0, 0.0)
            .size(row_w, cell_h)
            .inert(),
    )
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

