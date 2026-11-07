//! Project icons (`project-icon`) in this app's palette: Desktop's image or letter tile.

use opencode_kit::model::Project;
use project_icon::{Look, Palette, Style};
use rune::{Cx, El, el};

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

/// A session row's avatar (`SessionTabAvatarView`): the project's badge, the open-tab bar 6 before
/// it when `open`, and the unread dot on its corner when `unread`, cut clear of the badge.
pub fn session_avatar(s: &App, p: &Pal, project: Option<&Project>, name: &str, open: bool, unread: bool) -> El {
    let mut face = badge(s, p, project, name);
    if unread {
        // `project-avatar.css:118-120`: `radial-gradient(circle 4.5px at 100% 0, transparent 4px,
        // black 5px)` cuts the badge's corner clear around the dot, over any row fill.
        face = el().key("face").size(16.0, 16.0).cutout_circle([16.0, 0.0], 4.0, 5.0).child(face);
    }
    let mut avatar = el().key("badge").size(16.0, 16.0).child(face);
    if open {
        // `h-3 w-0.5 rounded-[2px] bg-v2-background-bg-layer-04`, 6 before the avatar, centred.
        avatar = avatar.child(el().key("open").absolute(-6.0, 2.0).size(2.0, 12.0).radius(2.0).fill(p.q.layer4));
    }
    if unread {
        // `project-avatar.css:122-133`: a 6 pt `bg-accent` dot centred on the corner.
        avatar = avatar.child(el().key("unread").absolute(13.0, -3.0).size(6.0, 6.0).radius(3.0).fill(p.bg_accent));
    }
    avatar
}
