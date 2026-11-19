//! `Sidebar/Project icons`: the sidebar's tabs in each icon finish (⌘⇧K, Project icons), with
//! a full-bleed image, a mark on its tile, a chosen colour and gray letter tiles, as Kit's own
//! sidebar mixes them.

use rune::{El, Play, col};

use super::{FINISH, GROUP};
use crate::palette::Pal;
use crate::stories::{Story, Trigger, replay, stage};
use crate::{App, view};

pub const VARIANTS: &[(&str, usize)] = &[("Candy", 0), ("Glass", 1), ("Flat (before)", 2)];

const KEYS: &[Trigger] = &[
    replay(setup),
    Trigger {
        key: 'f',
        label: "Next finish",
        run: |s, _| {
            let next = (s.diag.knobs.get(&FINISH).round() as usize + 1) % project_icon::Finish::ALL.len();
            s.diag.knobs.set(&FINISH, next as f32);
        },
    },
];

/// A drop on a dark tile, like a project's own app icon.
const DROP: &str = "data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 64 64'><rect width='64' height='64' fill='%23201a1c'/><path d='M32 11C32 11 17 29 17 39.5a15 15 0 0 0 30 0C47 29 32 11 32 11Z' fill='%23e5484d'/></svg>";
/// A white mark on a blue gradient.
const MARK: &str = "data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 64 64'><defs><linearGradient id='g' x1='0' y1='0' x2='0' y2='1'><stop offset='0' stop-color='%234f8cff'/><stop offset='1' stop-color='%232456d6'/></linearGradient></defs><rect width='64' height='64' fill='url(%23g)'/><path d='M20 16h16a12 12 0 0 1 3 23.6L46 48h-9l-6-8h-3v8h-8Zm8 7v10h7a5 5 0 0 0 0-10Z' fill='white'/></svg>";
/// A transparent glyph, drawn over the avatar colour.
const GLYPH: &str = "data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 64 64'><rect x='18' y='18' width='28' height='28' rx='5' fill='none' stroke='white' stroke-width='6'/></svg>";

fn setup(s: &mut App, _: &mut Play<App>) {
    s.finish_stream();
    s.diag.knobs.set(&FINISH, *s.fixture_run.variant as f32);
    for p in &mut s.projects {
        match opencode_kit::path::basename(&p.directory) {
            "lumen" => p.icon.override_url = DROP.into(),
            "harbor" => p.icon.override_url = MARK.into(),
            "tidepool" => p.icon.override_url = GLYPH.into(),
            "quill" => p.icon.color = "blue".into(),
            _ => {}
        }
    }
}

/// The sidebar's tabs, as the app draws them, at the sidebar's width.
fn story_view(s: &App, p: &Pal) -> El {
    let list = view::tab_strip(s, p, "story-tabs", rune_kit::shell::TabOrientation::Vertical, None, None);
    col().key("story-project-icons").w(*s.sidebar_w).h(300.0).pt(8.0).pl(11.0).pr(3.0).fill(p.bg_deep).radius(10.0).child(list)
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story { path: "Sidebar/Project icons", scene: |s| stage(s, story_view), knobs: &[GROUP], keys: KEYS, variants: VARIANTS },
]);
