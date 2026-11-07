//! The session header's location (`SessionLocation` in `session-identity-header.tsx`): the
//! project's avatar and name as a button that opens the location card: the project (back to it
//! on Home), its workspace, the branch, the folder, and Edit project.

use rune::{El, TextStyle, Tx, row, text};
use rune_theme_opencode::icons as ic;

use crate::App;
use crate::palette::Pal;

/// The trigger's key: the card opens beside it.
pub const TRIGGER: &str = "location-trigger";

/// 13 on 16 at the regular weight, as the trigger's `text-[13px] font-normal`.
const LABEL: TextStyle = TextStyle::new(13.0, 400.0).lh(16.0).tracking(-0.04);

pub use super::location_card::Location;

/// The trigger: the 16 pt project avatar, the project's name, and for a session in one of the
/// project's worktrees a slash, the worktree glyph and its name, muted until hovered or open.
/// `collapse` is the header's fit: 1 hides the worktree's name, 2 the project's too.
pub fn trigger(s: &App, p: &Pal, project: Option<&opencode_kit::model::Project>, name: &str, worktree: Option<&str>, collapse: u8) -> El {
    let open = s.popup.open_at(TRIGGER);
    let ink = if open { p.text } else { p.muted };
    let label = match worktree {
        Some(w) => format!("{name} / {w}"),
        None => name.to_owned(),
    };
    let mut t = row()
        .key(TRIGGER)
        .role("button")
        .label(label.clone())
        .help(label)
        .h(28.0)
        .px(6.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .shrink()
        .min_w(0.0)
        .hover_group()
        .hover(p.hover)
        .hover_motion(crate::view::INSTANT, crate::view::INSTANT)
        .on_press_tx(|s: &mut App, tx: &mut Tx| super::location_card::open_card(s, tx))
        .child(crate::icons::badge(s, p, project, name));
    if collapse < 2 {
        t = t.child(text(name.to_owned()).key("location-project").font(LABEL).color(ink).group_color(p.text).single_line());
    }
    if let Some(w) = worktree {
        // `SessionHeaderSeparator gap 0`: the trigger's own gap each side, less the glyph's bearing.
        t = t
            .child(text("/").font(TextStyle::new(13.0, 400.0).lh(16.0)).color(p.icon_faint).mx(-0.5))
            .child(rune::icon(&ic::OUTLINE_WORKTREE, 16.0).color(p.icon_muted));
        if collapse < 1 {
            t = t.child(text(w.to_owned()).key("location-worktree").font(LABEL).color(ink).group_color(p.text).single_line());
        }
    }
    if open { t.fill(p.hover) } else { t }
}

/// The worktree a session in `dir` runs in, by its folder's name: one of the project's sandboxes
/// (its root left out) holding `dir` (`worktree()` in `SessionLocation`).
pub fn worktree_name(project: Option<&opencode_kit::model::Project>, dir: &str) -> Option<String> {
    let project = project?;
    // Windows' paths compare without case, as Desktop's `pathKey` does there.
    let norm = |d: &str| {
        let d = d.trim_end_matches(['/', '\\']).replace('\\', "/");
        if cfg!(windows) { d.to_lowercase() } else { d }
    };
    let root = norm(&project.directory);
    let dir = norm(dir);
    opencode_kit::model::project_sandboxes(&project.id)
        .into_iter()
        .filter(|w| norm(w) != root)
        .find(|w| {
            let w = norm(w);
            dir == w || dir.starts_with(&format!("{w}/"))
        })
        .map(|w| opencode_kit::path::basename(w.trim_end_matches(['/', '\\'])).to_owned())
}
