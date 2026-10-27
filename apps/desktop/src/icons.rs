//! Project icons (`project-icon`) in this app's palette: Desktop's image or letter tile.

use opencode_kit::model::Project;
use project_icon::{Look, Palette, Style};
use rune::{Cx, El};

use crate::App;
use crate::palette::Pal;

pub fn palette(p: &Pal) -> Palette {
    Palette { dark: p.dark, avatars: p.avatars, fg: p.avatar_fg, tile: p.layer3, ring: p.ring }
}

/// Before a frame: images and SVGs rasterize for the screen and zoom.
pub fn prepare(s: &App, cx: &Cx<App>) {
    s.icons.set_scale(cx.env().scale * s.zoom);
    s.icons.set_look(Look::style(Style::Desktop));
}

/// A project's 16 pt icon, or its letter tile for `name` when the project isn't known.
pub fn badge(s: &App, p: &Pal, project: Option<&Project>, name: &str) -> El {
    let pal = palette(p);
    match project {
        Some(x) => s.icons.view(&pal, x, 16.0),
        None => s.icons.view(&pal, project_icon::Project::named(name), 16.0),
    }
}
