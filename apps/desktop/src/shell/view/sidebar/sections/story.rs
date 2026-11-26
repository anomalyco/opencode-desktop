//! Shell › Whisper sections: the sidebar's tabs in Experimental › Sidebar sections, made up
//! (the fixture's Lumen, Harbor and Tidepool projects). Variants: a section over ungrouped tabs; two tabs selected with the
//! bar showing; a new section being named, its other names under it; a folded section whose tab
//! works; and a walkthrough that plays it all through: two tabs apart picked, grouped (they glide
//! together under a new header), named, folded, opened and ungrouped again. R replays; S selects
//! two tabs, G groups them, F folds the first section.

use super::*;
use crate::stories::{Story, Trigger, replay, stage};
use crate::tabs::sections::{self as sec, Section, naming, select};

const VARIANTS: &[(&str, usize)] = &[("Default", 0), ("Multi-selected", 1), ("Naming new section", 2), ("Collapsed section", 3), ("Walkthrough", 4)];

const KEYS: &[Trigger] = &[
    replay(open),
    Trigger { key: 's', label: "Select two tabs", run: |s, _| pick(s) },
    Trigger { key: 'g', label: "Group the selection", run: |s, tx| {
        let uids = sec::state(s).selection.uids().to_vec();
        sec::create(s, &uids, tx);
    } },
    Trigger { key: 'f', label: "Fold the first section", run: |s, tx| {
        if let Some(id) = sec::state(s).doc.sections.first().map(|x| x.id.clone()) {
            sec::toggle(s, &id, tx);
        }
    } },
];

/// The made-up tabs, top to bottom: the fixture's session (in the project the title fits) and
/// its title.
const TABS: [(&str, &str); 7] = [
    ("ses_fx0001aaaaaaaaaaaaaaaaaa", "LUMEN TOKENIZER STATS"),
    ("ses_fx0003aaaaaaaaaaaaaaaaaa", "LUMEN ROUTING RULES"),
    ("ses_fx0006aaaaaaaaaaaaaaaaaa", "Deploy preview builds"),
    ("ses_fx0007aaaaaaaaaaaaaaaaaa", "Deploy cold starts"),
    ("ses_fx0008aaaaaaaaaaaaaaaaaa", "Sync engine rewrite"),
    ("ses_fx0009aaaaaaaaaaaaaaaaaa", "Offline cache"),
    ("ses_fx0010aaaaaaaaaaaaaaaaaa", "Release notes draft"),
];

fn uid(s: &App, i: usize) -> String {
    s.tabs.get(i).map(crate::tabs::Tab::uid).unwrap_or_default()
}

/// The story's world: seven made-up session tabs in side tabs, sections on, the first open.
fn setup(s: &mut App) {
    crate::open_scene(s);
    s.finish_stream();
    crate::scenes::side_tabs(s);
    s.settings.sidebar_organize = crate::settings::Organize::List;
    s.settings.tab_groups = Some(false);
    s.settings.sidebar_sections = Some(true);
    let mut tabs = Vec::new();
    for (id, title) in TABS {
        if let Some(info) = s.sessions.iter_mut().find(|x| x.id == id) {
            info.title = title.to_owned();
            tabs.push(crate::tabs::Tab::session(id, title, info.directory.clone()));
        }
    }
    *s.tabs = tabs;
    if let Some(t) = s.tabs.first() {
        *s.route = Route::here(t.id.clone());
    }
    let lumen = vec![uid(s, 0), uid(s, 1)];
    let st = sec::state_mut(s);
    st.doc.sections = vec![Section { id: "story-lumen".into(), name: "Lumen".into(), collapsed: false, members: lumen }];
    st.selection = select::Selection::Empty;
    st.naming = Default::default();
    st.drag = Default::default();
}

/// Harbor's two tabs, selected.
fn pick(s: &mut App) {
    let (a, b) = (uid(s, 2), uid(s, 3));
    let st = sec::state_mut(s);
    st.selection.step(select::Event::Toggle(&a));
    st.selection.step(select::Event::Toggle(&b));
}

fn open(s: &mut App, play: &mut rune::Play<'_, App>) {
    setup(s);
    match *s.fixture_run.variant {
        1 => pick(s),
        2 => {
            pick(s);
            let uids = sec::state(s).selection.uids().to_vec();
            sec::create(s, &uids, play);
        }
        3 => {
            if let Some(first) = sec::state_mut(s).doc.sections.first_mut() {
                first.collapsed = true;
            }
            let working = s.tabs.get(1).map(|t| t.id.clone());
            if let Some(id) = working {
                s.running.insert(id);
            }
            // The open tab is out of the folded section, as it would be.
            let open = s.tabs.get(4).map(|t| t.id.clone());
            if let Some(id) = open {
                *s.route = Route::here(id);
            }
        }
        _ => {}
    }
    play.instant();
    if *s.fixture_run.variant == 4 {
        // Harbor's two Deploy tabs two apart, so grouping them glides one past the others and
        // offers names (Deploy, Harbor) to pick.
        let cold = s.tabs.remove(3);
        s.tabs.insert(5, cold);
        walkthrough(play);
    }
}

/// Variant 4's steps, seconds from the start: Deploy preview builds and Deploy cold starts, two
/// tabs apart, picked one by one, grouped, named Harbor from the names offered, folded, opened,
/// and ungrouped.
fn walkthrough(play: &mut rune::Play<'_, App>) {
    play.after(0.55, |s, play| pick_one(s, play, 2));
    play.after(1.15, |s, play| pick_one(s, play, 5));
    play.after(1.85, |s, play| {
        play.animated();
        let uids = sec::state(s).selection.uids().to_vec();
        sec::create(s, &uids, play);
    });
    play.after(3.15, |s, play| {
        let alt = sec::state(s).naming.editing().and_then(|(_, _, names)| names.first().cloned());
        match alt {
            Some(name) => sec::naming_step(s, naming::Event::Pick(&name), play),
            None => sec::naming_step(s, naming::Event::Commit, play),
        }
        play.blur();
        play.animated();
    });
    play.after(4.05, fold_new);
    play.after(5.10, fold_new);
    play.after(6.05, |s, play| {
        if let Some(id) = newest(s) {
            sec::remove_section(s, &id, play);
        }
        play.animated();
    });
}

/// Tab `i` joins the selection, as a ⌘-click does.
fn pick_one(s: &mut App, play: &mut rune::Play<'_, App>, i: usize) {
    play.animated();
    let u = uid(s, i);
    sec::state_mut(s).selection.step(select::Event::Toggle(&u));
}

/// The section the walkthrough made (the last one).
fn newest(s: &App) -> Option<String> {
    sec::state(s).doc.sections.last().filter(|x| x.id != "story-lumen").map(|x| x.id.clone())
}

fn fold_new(s: &mut App, play: &mut rune::Play<'_, App>) {
    if let Some(id) = newest(s) {
        sec::toggle(s, &id, play);
    }
}

/// The sidebar alone, as wide as the app draws it.
fn view(s: &App, p: &Pal) -> El {
    col()
        .key("story-sections-sidebar")
        .w(*s.sidebar_w)
        .h(440.0)
        .pt(12.0)
        .pl(INSET_START)
        .pr(INSET_END)
        .pb(10.0)
        .radius(10.0)
        .fill(p.bg_deep)
        .box_shadow(Pal::ring_out(p.ring))
        .clip_soft(0.0)
        .child(super::list(s, p, None))
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story { path: "Shell/Whisper sections", scene: |s| { setup(s); stage(s, view); }, knobs: &[], keys: KEYS, variants: VARIANTS },
]);
