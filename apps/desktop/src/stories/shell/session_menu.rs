//! The storybook's Sidebar › Rename and menus story: each right-click menu beside the title it's
//! on, a title being renamed in each place, and Delete's confirmation. Native menus never draw
//! headless, so the menus are drawn copies; M opens the real one, N lights its next item, E
//! starts the rename, D asks to delete, R replays.

use rune::{El, Play, PopupAnchor, Tx, col, el, row};

use crate::palette::Pal;
use crate::session_menu::{self, Line, Spot, Target};
use crate::session_menu_view::preview;
use crate::stories::{Trigger, replay};
use crate::view;
use crate::{App, Route};

pub const VARIANTS: &[(&str, usize)] =
    &[("Tab menu", 0), ("Home row menu", 1), ("Header menu", 2), ("Renaming a tab", 3), ("Renaming a row", 4), ("Renaming the title", 5), ("Delete", 6)];
pub const KEYS: &[Trigger] = &[
    replay(open),
    Trigger { key: 'm', label: "Open the real menu", run: |s, tx| native(s, tx) },
    Trigger { key: 'n', label: "Light the next item", run: |s, tx| next(s, tx) },
    Trigger { key: 'e', label: "Rename", run: |s, _| start(s, true) },
    Trigger { key: 'd', label: "Ask to delete", run: |s, _| { let id = first(s); s.ask_delete(&id) } },
];

fn spot(v: usize) -> Spot {
    match v {
        0 | 3 => Spot::Tab,
        1 | 4 => Spot::Row,
        _ => Spot::Header,
    }
}

/// The session the story is on: the open one, which is the first tab's.
fn first(s: &App) -> String {
    match &*s.route {
        Route::Session { id, .. } => s.root_of(id).to_owned(),
        _ => s.tabs.first().map_or_else(String::new, |t| t.id.clone()),
    }
}

fn target(s: &App) -> Target {
    Target::active(s, &first(s), spot(*s.variant))
}

fn open(s: &mut App, _: &mut Play<App>) {
    s.finish_stream();
    s.menus = Default::default();
    match *s.variant {
        0..=2 => {
            s.menus.on = Some(target(s));
            s.menus.hot = Some("rename".into());
        }
        3..=5 => start(s, false),
        _ => { let id = first(s); s.ask_delete(&id) }
    }
}

/// Starts the rename; `retitle` types a new title first, as a person would.
fn start(s: &mut App, retitle: bool) {
    let t = Target::active(s, &first(s), spot(*s.variant));
    s.begin_rename(&t);
    if retitle {
        s.menus.edit.replace_text("Reconnect jitter, with a fake clock");
    }
}

fn native(s: &mut App, tx: &mut Tx) {
    let t = target(s);
    s.menus.on = None;
    let anchor = match t.spot {
        Spot::Tab => "story-tab-0",
        Spot::Row => "story-row-0",
        Spot::Header => "story-title",
    };
    session_menu::open(s, t, PopupAnchor::Key(anchor.into()), tx);
}

fn next(s: &mut App, _: &mut Tx) {
    let ids: Vec<&'static str> = session_menu::lines(s, &target(s))
        .into_iter()
        .filter_map(|l| match l {
            Line::Item(i) if i.enabled => Some(i.id),
            _ => None,
        })
        .collect();
    let at = ids.iter().position(|id| Some(*id) == s.menus.hot.as_deref()).map_or(0, |i| i + 1);
    s.menus.hot = ids.get(at % ids.len().max(1)).map(|id| (*id).to_owned());
}

pub fn view(s: &App, p: &Pal) -> El {
    let t = target(s);
    let menu = (*s.variant <= 2).then(|| preview(s, p, "menu", &session_menu::lines(s, &t), s.menus.hot.as_deref()));
    let stage = match t.spot {
        Spot::Tab => tabs(s, p, menu),
        Spot::Row => rows(s, p, menu),
        Spot::Header => header(s, p, menu),
    };
    col().key("story-menus").w_full().max_w(820.0).child(stage)
}

/// The sidebar's tabs, as the app draws them.
fn tabs(s: &App, p: &Pal, menu: Option<El>) -> El {
    let list = view::tab_strip(s, p, "story-tabs", rune_kit::shell::TabOrientation::Vertical, None, None);
    let side = col().key("story-sidebar").w(*s.sidebar_w).h(280.0).pt(8.0).pl(11.0).pr(3.0).fill(p.bg_deep).radius(10.0).child(list);
    let stage = el().key("story-stage").w_full().h(300.0).child(side).child(row().key("story-tab-0").absolute(11.0, 8.0).w(*s.sidebar_w - 14.0).h(28.0));
    place(stage, menu, 36.0 + 8.0, 40.0)
}

fn rows(s: &App, p: &Pal, menu: Option<El>) -> El {
    let ids: std::rc::Rc<Vec<String>> = std::rc::Rc::new(s.sessions.iter().filter(|x| x.parent.is_none()).take(4).map(|x| x.id.clone()).collect());
    let mut list = col().key("story-rows").w_full().gap(1.0).padding(12.0).fill(p.bg_base).radius(10.0);
    for (i, id) in ids.iter().enumerate() {
        if let Some(info) = s.session(id) {
            list = list.child(view::session_row(s, p, info, &ids, i));
        }
    }
    let stage = el().key("story-stage").w_full().h(300.0).child(list).child(row().key("story-row-0").absolute(12.0, 12.0).w(400.0).h(40.0));
    place(stage, menu, 52.0, 60.0)
}

fn header(s: &App, p: &Pal, menu: Option<El>) -> El {
    let stage = el()
        .key("story-stage")
        .w_full()
        .h(220.0)
        .child(col().key("story-header").w_full().fill(p.bg_base).radius(10.0).child(view::story_header(s, p)))
        .child(row().key("story-title").absolute(150.0, 10.0).w(240.0).h(28.0));
    place(stage, menu, 44.0, 150.0)
}

/// `menu` hanging from (`x`, `y`) of the stage.
fn place(stage: El, menu: Option<El>, y: f32, x: f32) -> El {
    match menu {
        Some(m) => stage.child(m.absolute(x, y)),
        None => stage,
    }
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story { path: "Sidebar/Rename and menus", scene: |s| crate::stories::stage(s, view), knobs: &[], keys: KEYS, variants: VARIANTS },
]);
