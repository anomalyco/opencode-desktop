//! Experimental › Sidebar sections (desktop host): side tabs a person picks several of with
//! ⌘-click and ⇧-click, acts on together from a right-click, and gathers under quiet named
//! headers (⌘G). Off, nothing here runs: no state is read or saved and no tab moves.
//!
//! The window keeps its sections in `sidebar.sections` beside its tabs; a closed tab leaves its
//! section, and a section left empty goes.

pub mod drag;
pub mod model;
pub mod naming;
mod ops;
pub mod select;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use opencode_kernel::{EscapeLayer, Feature, KernelBuilder, OverlayRegistration, Slot};
use rune::{KeyEvent, TextEdit, Tx};

pub use model::{Doc, Drop, Laid, Section};
pub use ops::*;

use crate::App;
use crate::tabs::Tab;

/// A window's sections, its selection, the name being edited and the drag.
#[derive(Default)]
pub struct SectionsState {
    pub doc: Doc,
    pub selection: select::Selection,
    pub naming: naming::Naming,
    /// The field a late edit lands in once naming ended.
    pub scratch: TextEdit,
    pub drag: drag::DragState,
    /// The tabs open when last looked (`Tab::uid`): one gone leaves its section.
    seen: Vec<String>,
}

pub static SECTIONS: Slot<SectionsState> = Slot::new();

/// What the rows read without redrawing: where each was laid out (window points) and the
/// modifiers held now (`home_modifiers`), for a press to read as it lands.
#[derive(Default)]
pub struct Live {
    pub rects: RefCell<HashMap<String, [f32; 4]>>,
    pub cmd: Cell<bool>,
    pub shift: Cell<bool>,
}

pub static LIVE: Slot<Live> = Slot::new();

const ROW: &str = "sidebar.sections";
pub const OVERLAY_ID: &str = "sidebar.sections";
/// The name field's key.
pub const FIELD: &str = "sidebar-section-name";

pub struct SidebarSectionsFeature;

impl Feature<App> for SidebarSectionsFeature {
    fn id(&self) -> &'static str {
        "feature.sidebar-sections"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder.register_slot(&SECTIONS);
        builder.register_slot(&LIVE);
        builder.schedule(crate::tick::stage::REACT_SHELL, "sidebar-sections.poll", |s, _, tx| poll(s, tx));
        // ⌘G gathers the selection before Find reads it as Find next (Find closed); no layer
        // draws here, so it takes the last band (`with_key_in`).
        builder.register_overlay(OverlayRegistration::<App>::new(OVERLAY_ID).with_key_in(crate::overlays::key_band::LOW, key));
        builder.register_escape_layer(EscapeLayer::<App>::new(
            "escape.sidebar_sections",
            crate::overlays::escape_band::FALLBACK,
            |s| !state(s).selection.is_empty(),
            |s, _, tx| {
                tx.instant();
                clear_selection(s);
            },
        ));
    }
}

pub fn state(s: &App) -> &SectionsState {
    s.kernel.slots.get(&SECTIONS)
}

pub(crate) fn state_mut(s: &mut App) -> &mut SectionsState {
    s.kernel.slots.get_mut(&SECTIONS)
}

/// Whether the sidebar lists its tabs in sections now: the setting is on and side tabs show the
/// plain list (Organize › List or Consolidated, with no tab groups of either kind).
pub fn on(s: &App) -> bool {
    use crate::settings::Organize;
    if !s.settings.sidebar_sections() || s.tab_layout() != crate::layout_state::TabLayout::Vertical || s.narrow() {
        return false;
    }
    match s.settings.sidebar_organize.effective() {
        Organize::List => !crate::tabs::groups::on(s),
        Organize::Projects => true,
        Organize::Consolidated => s.settings.navigation() != crate::settings::Navigation::List && crate::tabs::auto_groups::groups(s).is_none(),
        _ => false,
    }
}

/// The tabs the list shows (a pinned one is in the pinned area): index and uid.
pub fn shown(s: &App) -> Vec<(usize, String)> {
    s.tabs.iter().enumerate().filter(|(_, t)| !crate::tabs::pins::holds(s, t)).map(|(i, t)| (i, t.uid())).collect()
}

pub fn laid(s: &App) -> Laid {
    model::lay(&state(s).doc, &shown(s))
}

/// The tabs as listed, a folded section's hidden (⌘1–9 and ⇧-click run through these).
pub fn visible_uids(s: &App) -> Option<Vec<String>> {
    if !on(s) {
        return None;
    }
    Some(laid(s).visible().filter_map(|i| s.tabs.get(i).map(Tab::uid)).collect())
}

/// Every listed tab, sections first, folded ones too: the order a group of tabs keeps.
pub(crate) fn listed_uids(s: &App) -> Vec<String> {
    let l = laid(s);
    l.sections.iter().flat_map(|(_, t)| t.iter().copied()).chain(l.ungrouped.iter().copied()).filter_map(|i| s.tabs.get(i).map(Tab::uid)).collect()
}

/// Reads the window's row (from the boot packet at launch).
pub fn restore(s: &mut App) {
    let ns = opencode_storage::window_namespace(&s.window.id);
    let mut doc = crate::storage::json(&ns, ROW).map(|v| Doc::from_json(&v)).unwrap_or_default();
    let uids: Vec<String> = s.tabs.iter().map(Tab::uid).collect();
    if !uids.is_empty() {
        doc.retain_open(&uids);
    }
    let st = state_mut(s);
    st.doc = doc;
    st.seen = uids;
}

fn save(s: &App) {
    if s.persists() {
        crate::windows::store::save(&s.window, ROW, &state(s).doc.to_json());
    }
}

pub(crate) fn update(s: &mut App, f: impl FnOnce(&mut Doc)) {
    f(&mut state_mut(s).doc);
    save(s);
}

/// The live modifiers, from the window's modifier changes: written where no view reads them.
pub fn modifiers(s: &App, mods: rune::Mods) {
    let live = s.kernel.slots.untracked(&LIVE);
    live.cmd.set(mods.cmd);
    live.shift.set(mods.shift);
}

fn poll(s: &mut App, tx: &mut Tx) -> bool {
    if !s.settings.sidebar_sections() {
        return false;
    }
    let mut changed = false;
    let uids: Vec<String> = s.tabs.iter().map(Tab::uid).collect();
    if uids != s.kernel.slots.untracked(&SECTIONS).seen {
        let gone: Vec<String> = s.kernel.slots.untracked(&SECTIONS).seen.iter().filter(|u| !uids.contains(u)).cloned().collect();
        if !gone.is_empty() && state(s).doc.sections.iter().any(|sec| sec.members.iter().any(|m| gone.contains(m))) {
            update(s, |d| d.pull(&gone));
        }
        let st = state_mut(s);
        st.selection.step(select::Event::Retain(&uids));
        st.seen = uids;
        changed = true;
    }
    if !on(s) && !s.kernel.slots.untracked(&SECTIONS).selection.is_empty() {
        clear_selection(s);
        changed = true;
    }
    if let Some(focused) = s.kernel.slots.untracked(&SECTIONS).naming.focused() {
        let arming = matches!(s.kernel.slots.untracked(&SECTIONS).naming, naming::Naming::Arming { .. });
        if arming || !focused {
            naming_step(s, naming::Event::Focused(focused), tx);
            changed = true;
        }
    }
    changed
}

/// ⌘G (Ctrl+G off macOS) with tabs selected: a new section of them. Find's own ⌘G (Find next)
/// keeps the key while its bar is open.
fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if !ev.is_command('g') || ev.mods.shift || ev.mods.alt || state(s).selection.is_empty() || !on(s) || s.settings_open() {
        return false;
    }
    if s.kernel.slots.get(&crate::find::FIND).is_open() {
        return false;
    }
    let uids = state(s).selection.uids().to_vec();
    create(s, &uids, tx);
    true
}
