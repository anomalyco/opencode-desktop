//! Storybook stage (`⌘⇧D`) for `"Terminal/Pane"` with recorded streams (`Build output`, `htop`,
//! `Colors & Unicode`, and `Scrollback`).

use rune::{El, Play, col, el};
use rune_knobs::Group;
use terminal::{RestoreState, ScrollOp, TerminalConfig};

use super::grid::{cell_metrics, render_terminal_pane, terminal_font_style};
use super::streams::{
    BUILD_BURST, BUILD_STREAM, COLORS_UNICODE_STREAM, HTOP_STREAM, HTOP_TICK_STREAM,
    cat_lines_stream,
};
use super::{TERMINAL_KNOBS, theme_from_pal};
use crate::App;
use crate::palette::{Pal, pal};
use crate::stories::{Story, Trigger, replay, stage};

pub const TERMINAL_VARIANTS: &[(&str, usize)] = &[
    ("Build output", 0),
    ("htop", 1),
    ("Colors & Unicode", 2),
    ("Scrollback", 3),
];

const STORY_GROUPS: &[Group] = &[Group::new("Terminal", "stage", TERMINAL_KNOBS)];

pub const TERMINAL_KEYS: &[Trigger] = &[
    replay(setup_story),
    Trigger {
        key: 'u',
        label: "Stream update",
        run: stream_update,
    },
    Trigger {
        key: 's',
        label: "Scroll up",
        run: scroll_up,
    },
    Trigger {
        key: 'b',
        label: "Scroll to bottom",
        run: scroll_bottom,
    },
    Trigger {
        key: 'n',
        label: "New terminal tab",
        run: new_tab,
    },
];

const STORY_WS: &str = "story\0/workspace/seer";
const STORY_MAX_W: f32 = 720.0;
const STORY_MAX_H: f32 = 440.0;

fn story_card_size(zoom: f32) -> (f32, f32) {
    let z = zoom.max(0.5);
    let w = ((814.0 / z) - 48.0).clamp(320.0, STORY_MAX_W);
    let h = ((820.0 / z) - 48.0).clamp(240.0, STORY_MAX_H);
    (w, h)
}

pub fn init_story(s: &mut App) {
    crate::open_scene(s);
    s.finish_stream();
    s.terminal.workspaces.clear();
    s.terminal.lru_order.clear();
    s.terminal.story_focused = true;
    s.terminal.story_built_variant = Some(s.variant);
    s.terminal.reset_blink(s.clock);

    let theme = theme_from_pal(pal(s.terminal_dark()));
    let (choice, style) = terminal_font_style(&s.knobs);
    let (cw, ch) = cell_metrics(choice, style);
    let (card_w, card_h) = story_card_size(s.zoom);
    let cols = (((card_w - 32.0) / cw).floor() as u16).clamp(36, 200);
    let rows = (((card_h - 54.0) / ch).floor() as u16).clamp(8, 80);

    let (title, bytes): (&str, Vec<u8>) = match s.variant {
        1 => ("htop", HTOP_STREAM.to_vec()),
        2 => ("Colors & Unicode", COLORS_UNICODE_STREAM.to_vec()),
        3 => ("build.log", cat_lines_stream(1_500)),
        _ => ("Terminal 1", BUILD_STREAM.to_vec()),
    };

    let restore = RestoreState {
        buffer: bytes,
        cols: Some(cols),
        rows: Some(rows),
        scroll_y: (s.variant == 3).then_some(720),
        cursor: None,
    };

    s.insert_stream_terminal(
        STORY_WS,
        "term_1",
        title,
        1,
        TerminalConfig {
            cols,
            rows,
            cell_w_px: (cw * 2.0).round() as u32,
            cell_h_px: (ch * 2.0).round() as u32,
            scrollback_lines: 10_000,
            theme: theme.clone(),
        },
        Some(restore),
        b"",
    );

    if s.variant == 0 {
        let log_restore = RestoreState {
            buffer: b"\x1b[32mINFO\x1b[0m listening on http://127.0.0.1:4096\r\n".to_vec(),
            cols: Some(cols),
            rows: Some(rows),
            scroll_y: None,
            cursor: None,
        };
        s.insert_stream_terminal(
            STORY_WS,
            "term_2",
            "Server log",
            2,
            TerminalConfig {
                cols,
                rows,
                cell_w_px: (cw * 2.0).round() as u32,
                cell_h_px: (ch * 2.0).round() as u32,
                scrollback_lines: 10_000,
                theme,
            },
            Some(log_restore),
            b"",
        );
        s.select_terminal_tab(STORY_WS, "term_1");
    }
}

fn setup_story(s: &mut App, _play: &mut Play<'_, App>) {
    init_story(s);
}

fn stream_update(s: &mut App, _play: &mut Play<'_, App>) {
    let payload = if s.variant == 1 {
        HTOP_TICK_STREAM
    } else {
        BUILD_BURST
    };
    if let Some(ws) = s.terminal.workspaces.get(STORY_WS)
        && let Some(active) = ws.active.as_deref()
        && let Some(tab) = ws.tabs.iter().find(|t| t.id == active)
        && let Some(stream) = &tab.stream
    {
        stream.feed(payload);
    }
}

fn scroll_up(s: &mut App, _play: &mut Play<'_, App>) {
    if let Some(ws) = s.terminal.workspaces.get(STORY_WS)
        && let Some(active) = ws.active.as_deref()
        && let Some(tab) = ws.tabs.iter().find(|t| t.id == active)
    {
        tab.session.scroll(ScrollOp::Delta(-12));
    }
}

fn scroll_bottom(s: &mut App, _play: &mut Play<'_, App>) {
    if let Some(ws) = s.terminal.workspaces.get(STORY_WS)
        && let Some(active) = ws.active.as_deref()
        && let Some(tab) = ws.tabs.iter().find(|t| t.id == active)
    {
        tab.session.scroll(ScrollOp::Bottom);
    }
}

fn new_tab(s: &mut App, _play: &mut Play<'_, App>) {
    s.new_terminal_in(STORY_WS);
}

pub fn view_card(s: &App, p: &Pal) -> El {
    s.terminal.painted_dark.set(Some(p.dark));
    let (card_w, card_h) = story_card_size(s.zoom);
    let inner = render_terminal_pane(
        s,
        p,
        STORY_WS,
        (card_w, card_h),
        2.0,
        s.terminal.story_focused,
        false,
        false,
    );
    col()
        .key("story-terminal")
        .w_full()
        .max_w(card_w)
        .h_full()
        .max_h(card_h)
        .radius(10.0)
        .fill(p.bg_base)
        .shadow(2.0, 4.0, p.shadow)
        .shadow(1.0, 2.0, p.shadow)
        .child(
            col()
                .w_full()
                .h_full()
                .clip_rounded(10.0)
                .child(inner),
        )
        .child(
            el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .radius(10.0)
                .border(0.5, p.ring)
                .inert(),
        )
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story {
        path: "Terminal/Pane",
        scene: |s| {
            init_story(s);
            stage(s, view_card);
        },
        knobs: STORY_GROUPS,
        keys: TERMINAL_KEYS,
        variants: TERMINAL_VARIANTS,
    },
]);
