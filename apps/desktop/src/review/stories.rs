//! Storybook stages (`⌘⇧D`) for `Review/Changes` and `Review/Inline diff`.

use rune::{El, Play, col, el, row, text};
use rune_kit::diff::{DiffMode, FileDiff};

use super::ChangeMode;
use crate::App;
use crate::code_view::{DiffDocState, DiffViewer, ExpandMode};
use crate::palette::Pal;
use crate::panels::tabs::panel_key;
use crate::stories::{Story, Trigger, replay, stage};
use crate::view::UI_MED;

const REVIEW_VARIANTS: &[(&str, usize)] = &[
    ("Unified", 0),
    ("Split", 1),
    ("Expanded context", 2),
];

const INLINE_VARIANTS: &[(&str, usize)] = &[
    ("Unified edit step", 0),
    ("Split edit step", 1),
];

const REVIEW_KEYS: &[Trigger] = &[
    replay(setup_review),
    Trigger {
        key: 's',
        label: "Toggle split / unified",
        run: |s, _| {
            let next = match s.review.diff_mode {
                DiffMode::Unified => DiffMode::Split,
                DiffMode::Split => DiffMode::Unified,
            };
            s.set_review_diff_mode(next);
        },
    },
    Trigger {
        key: 'f',
        label: "Toggle file tree",
        run: |s, _| {
            s.panel_sidebar.opened = !s.panel_sidebar.opened;
        },
    },
    Trigger {
        key: 'e',
        label: "Toggle expand all",
        run: |s, _| {
            let next = match s.review.expand_mode {
                ExpandMode::Collapse => ExpandMode::Expand,
                ExpandMode::Expand => ExpandMode::Collapse,
            };
            s.set_review_expand_mode(next);
        },
    },
];

const INLINE_KEYS: &[Trigger] = &[replay(setup_inline)];

pub fn init_review(s: &mut App) {
    crate::open_scene(s);
    s.finish_stream();
    s.side = Default::default();
    s.review = Default::default();
    let Some(sid) = s.active_session_id().map(str::to_owned) else { return };
    s.show_side_tab(&sid, &panel_key("review", "changes"), None);
    match *s.variant {
        0 => s.set_review_diff_mode(DiffMode::Unified),
        1 => s.set_review_diff_mode(DiffMode::Split),
        _ => {
            s.set_review_diff_mode(DiffMode::Unified);
            s.set_review_change_mode(ChangeMode::Git);
            s.set_review_expand_mode(ExpandMode::Expand);
        }
    }
    s.ensure_session_review(&sid);
}

fn setup_review(s: &mut App, _play: &mut Play<'_, App>) {
    init_review(s);
}

pub fn init_inline(s: &mut App) {
    crate::open_scene(s);
    s.finish_stream();
    let mut inline = DiffDocState::inline(FileDiff::from_texts_with_context(
        "src/net/stream.ts",
        "export function watchdog(stream: EventStream, reconnect: () => void) {\n  stream.on(\"error\", reconnect)\n}\n",
        "const QUIET_MS = 25_000\n\nexport function watchdog(stream: EventStream, reconnect: () => void) {\n  let timer = setTimeout(fire, QUIET_MS)\n  function fire() {\n    stream.close()\n    reconnect()\n  }\n  stream.on(\"message\", () => {\n    clearTimeout(timer)\n    timer = setTimeout(fire, QUIET_MS)\n  })\n}\n",
        session_view::tools::TEXT_CONTEXT,
    ));
    inline.set_mode(if s.variant == 1 { DiffMode::Split } else { DiffMode::Unified });
    s.review.empty_diffs = inline;
}

fn setup_inline(s: &mut App, _play: &mut Play<'_, App>) {
    init_inline(s);
}

fn view_review_card(s: &App, p: &Pal) -> El {
    let w = if s.review.diff_mode == DiffMode::Split { 760.0 } else { 640.0 };
    crate::panels::view::story_card(s, p, w, 540.0)
}

fn view_inline_card(s: &App, p: &Pal) -> El {
    col()
        .key("inline-diff-story-card")
        .w(620.0)
        .h(260.0)
        .radius(8.0)
        .clip_rounded(8.0)
        .fill(p.bg_base)
        .box_shadow(Pal::ring_out(p.ring))
        .child(
            row()
                .w_full()
                .h(32.0)
                .px(10.0)
                .center_y()
                .fill(p.layer1)
                .child(text("Edit src/net/stream.ts").font(UI_MED).color(p.text).truncate_fade()),
        )
        .child(el().w_full().h(0.5).fill(p.border_muted))
        .child(
            DiffViewer::new(
                "story-inline-diff",
                |a: &App| &a.review.empty_diffs,
                |a: &mut App| &mut a.review.empty_diffs,
            )
            .sticky_headers(false)
            .build(s, p),
        )
}

// `--snap review-collapsed`: `src/net` closed, then the agent edits a file (a refetch).
rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [("review-collapsed", |s| {
    init_review(s);
    let Some(sid) = s.active_session_id().map(str::to_owned) else { return };
    if let Some(sr) = s.review.sessions.get_mut(&sid) {
        sr.tree.toggle("src/net");
    }
    s.note_review_event("filesystem.changed");
    s.ensure_session_review(&sid);
})]);

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story {
        path: "Review/Changes",
        scene: |s| {
            init_review(s);
            stage(s, view_review_card);
        },
        knobs: &[],
        keys: REVIEW_KEYS,
        variants: REVIEW_VARIANTS,
    },
    Story {
        path: "Review/Inline diff",
        scene: |s| {
            init_inline(s);
            stage(s, view_inline_card);
        },
        knobs: &[],
        keys: INLINE_KEYS,
        variants: INLINE_VARIANTS,
    },
]);
