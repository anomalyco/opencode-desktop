//! Project icons (`project-icon`) in this app's palette and knobs, and the `icon-playground`
//! scene that lays every mark style out for comparing them.

use opencode_kit::model::Project;
use project_icon::{Family, Look, Palette, Style};
use rune::{Cx, El, col, el, row, text};

use crate::App;
use crate::knobs::{ICON_GRAIN, ICON_SATURATION, ICON_STYLE};
use crate::palette::{Pal, pal};
use crate::view::{UI_MED, UI_SMALL};

const NAMES: [&str; 12] =
    ["rune", "scrawl", "opencode", "seer", "hex", "abattoir", "motel", "knobs", "deadbolt", "wraith", "conclave", "voice-control"];

pub fn palette(p: &Pal) -> Palette {
    Palette { dark: p.dark, avatars: p.avatars, fg: p.avatar_fg, tile: p.layer3, ring: p.ring }
}

/// The look the knobs set (⌘⇧K, Icons), starting from `--icon-style <name>`.
pub fn look(s: &App) -> Look {
    let style = Style::ALL.get(s.knobs.get(&ICON_STYLE).round() as usize).copied().unwrap_or(Style::Gradient);
    Look { style, saturation: s.knobs.get(&ICON_SATURATION), grain: s.knobs.get(&ICON_GRAIN) }
}

/// The knob value for `--icon-style`, if it names a style.
pub fn style_arg() -> Option<f32> {
    let name = crate::arg("--icon-style")?;
    Style::ALL.iter().position(|s| s.name().eq_ignore_ascii_case(&name)).map(|i| i as f32)
}

/// Before a frame: marks and SVGs rasterize for the screen and zoom, in the knobs' look.
pub fn prepare(s: &App, cx: &Cx<App>) {
    s.icons.set_scale(cx.env().scale * s.zoom);
    s.icons.set_look(look(s));
}

/// A project's 16 pt icon, or a mark for `name` when the project isn't known.
pub fn badge(s: &App, p: &Pal, project: Option<&Project>, name: &str) -> El {
    let pal = palette(p);
    match project {
        Some(x) => s.icons.view(&pal, x, 16.0),
        None => s.icons.view(&pal, project_icon::Project::named(name), 16.0),
    }
}

const MORE_NAMES: [&str; 12] =
    ["atlas", "bramble", "cobalt", "drift", "ember", "fjord", "gizmo", "harbor", "ink", "juniper", "kiwi", "lumen"];

/// Every style, by family, for twelve project names at 16, 24 and 48 pt; light above dark.
/// `--icon-family <name>` shows one family; `--icon-seeds` shows the knob's style for 24 names.
pub fn playground(s: &App) -> El {
    if crate::flag("--icon-seeds") {
        return seeds(s);
    }
    let base = look(s);
    let only = crate::arg("--icon-family");
    let panel = |dark: bool| {
        let p = pal(dark);
        let icons = palette(p);
        let mut rows = col().gap(14.0).px(20.0).py(20.0).fill(p.bg_base).w_full();
        rows = rows.child(text(if dark { "Dark" } else { "Light" }).font(UI_MED).color(p.text));
        for family in Family::ALL {
            if only.as_deref().is_some_and(|f| !f.eq_ignore_ascii_case(family.name())) {
                continue;
            }
            rows = rows.child(text(family.name().to_uppercase()).font(UI_SMALL).color(p.faint).mt(8.0));
            for style in family.styles() {
                let look = Look { style, ..base };
                let mut line = row().gap(20.0).center_y().child(text(style.name()).font(UI_MED).color(p.muted).w(84.0));
                for (size, gap) in [(16.0, 6.0), (24.0, 6.0), (48.0, 8.0)] {
                    let mut group = row().gap(gap).center_y();
                    for name in NAMES {
                        group = group.child(s.icons.mark(&icons, project_icon::Project::named(name), size, look));
                    }
                    line = line.child(group);
                }
                rows = rows.child(line);
            }
        }
        let mut sizes = row().gap(20.0).child(el().w(84.0));
        for (size, gap) in [(16.0, 6.0), (24.0, 6.0), (48.0, 8.0)] {
            sizes = sizes.child(text(format!("{size:.0} pt")).font(UI_SMALL).color(p.faint).w(12.0 * size + 11.0 * gap));
        }
        rows.child(sizes)
    };
    let panels = s.playground.iter().map(|&dark| panel(dark));
    col().key("icon-playground").w_full().h_full().scroll(rune::ScrollSpec { stick_bottom: false }).children(panels)
}

/// The knob's style for 24 project names, at 16 and 24 pt in a line and 48 pt with names.
fn seeds(s: &App) -> El {
    let look = look(s);
    let names = || NAMES.iter().chain(MORE_NAMES.iter()).copied();
    let panel = |dark: bool| {
        let p = pal(dark);
        let icons = palette(p);
        let mark = |name: &str, size: f32| s.icons.mark(&icons, project_icon::Project::named(name), size, look);
        let title = format!("{} · {}", look.style.name(), if dark { "Dark" } else { "Light" });
        let mut rows = col().gap(14.0).px(20.0).py(20.0).fill(p.bg_base).w_full();
        rows = rows.child(text(title).font(UI_MED).color(p.text));
        for (size, gap) in [(16.0, 6.0), (24.0, 8.0)] {
            rows = rows.child(row().gap(gap).center_y().children(names().map(|n| mark(n, size))));
        }
        for half in [&NAMES, &MORE_NAMES] {
            let cells = half.iter().map(|&n| col().gap(4.0).items_center().w(64.0).child(mark(n, 48.0)).child(text(n).font(UI_SMALL).color(p.faint)));
            rows = rows.child(row().gap(8.0).children(cells));
        }
        rows
    };
    let panels = s.playground.iter().map(|&dark| panel(dark));
    col().key("icon-playground").w_full().h_full().scroll(rune::ScrollSpec { stick_bottom: false }).children(panels)
}
