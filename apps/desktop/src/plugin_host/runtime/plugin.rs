//! One plugin half's state shared with its bindings: what its setup contributed, the signals
//! the host fires into it, its editors, its cached server reads and what its surfaces read.
//! The bindings hold it as `Rc<Half>`; the host holds the same `Rc` beside the script.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use mlua::{Function, RegistryKey, Value};
use opencode_plugin_spec as spec;

use crate::plugin_host::cx::Dep;

/// Which half of a plugin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Window,
    App,
}

/// One contribution: its decoded spec, the functions it handed over, and the VM they live in.
pub struct Contrib<S> {
    pub spec: S,
    pub fns: BTreeMap<&'static str, Function>,
    /// `<plugin>.<id>`, interned: how the app's lists name it.
    pub key: &'static str,
}

impl<S> Contrib<S> {
    pub fn f(&self, name: &str) -> Option<&Function> {
        self.fns.get(name)
    }
}

/// What a setup contributed, by kind.
#[derive(Default)]
pub struct Contribs {
    pub commands: Vec<Contrib<spec::CommandSpec>>,
    pub panels: Vec<Contrib<spec::PanelSpec>>,
    pub settings: Vec<Contrib<spec::SettingSpec>>,
    pub settings_pages: Vec<Contrib<spec::SettingsPageSpec>>,
    pub menu_items: Vec<Contrib<spec::MenuItemSpec>>,
    pub menubar_items: Vec<Contrib<spec::MenubarItemSpec>>,
    pub link_handlers: Vec<Contrib<spec::LinkHandlerSpec>>,
    pub titlebar_items: Vec<Contrib<spec::TitlebarItemSpec>>,
    pub composer_pills: Vec<Contrib<spec::ComposerPillSpec>>,
    pub code_blocks: Vec<Contrib<spec::CodeBlockSpec>>,
    pub code_block_actions: Vec<Contrib<spec::CodeBlockActionSpec>>,
    pub slots: Vec<Contrib<spec::SlotSpec>>,
    pub themes: Vec<Contrib<spec::ThemeSpec>>,
    pub file_viewers: Vec<Contrib<spec::FileViewerSpec>>,
    pub server_sources: Vec<Contrib<spec::ServerSourceSpec>>,
    pub dialogs: Vec<Contrib<spec::DialogSpec>>,
    pub popovers: Vec<Contrib<spec::PopoverSpec>>,
    pub mcp_tools: Vec<Contrib<spec::McpToolSpec>>,
}

impl Contribs {
    /// Takes contribution `kind`/`id` out (`Contribution:Withdraw`). True when it was there.
    pub fn withdraw(&mut self, kind: spec::ContributionKind, id: &str) -> bool {
        use spec::ContributionKind as K;
        fn drop_id<S>(list: &mut Vec<Contrib<S>>, id: &str, of: impl Fn(&S) -> &str) -> bool {
            let before = list.len();
            list.retain(|c| of(&c.spec) != id);
            list.len() != before
        }
        match kind {
            K::Command => drop_id(&mut self.commands, id, |s| &s.id),
            K::Panel => drop_id(&mut self.panels, id, |s| &s.id),
            K::Setting => drop_id(&mut self.settings, id, |s| &s.id),
            K::SettingsPage => drop_id(&mut self.settings_pages, id, |s| &s.id),
            K::MenuItem => drop_id(&mut self.menu_items, id, |s| &s.id),
            K::MenubarItem => drop_id(&mut self.menubar_items, id, |s| &s.id),
            K::LinkHandler => drop_id(&mut self.link_handlers, id, |s| &s.id),
            K::TitlebarItem => drop_id(&mut self.titlebar_items, id, |s| &s.id),
            K::ComposerPill => drop_id(&mut self.composer_pills, id, |s| &s.id),
            K::CodeBlock => drop_id(&mut self.code_blocks, id, |s| &s.id),
            K::CodeBlockAction => drop_id(&mut self.code_block_actions, id, |s| &s.id),
            K::Slot => drop_id(&mut self.slots, id, |s| &s.id),
            K::Theme => drop_id(&mut self.themes, id, |s| &s.id),
            K::FileViewer => drop_id(&mut self.file_viewers, id, |s| &s.id),
            K::ServerSource => drop_id(&mut self.server_sources, id, |s| &s.id),
            K::Dialog => drop_id(&mut self.dialogs, id, |s| &s.id),
            K::Popover => drop_id(&mut self.popovers, id, |s| &s.id),
            K::McpTool => drop_id(&mut self.mcp_tools, id, |s| &s.name),
        }
    }
}

/// A cached server read (`Server:Resource`): its ticket, and the answer's text and JSON as Luau
/// values, made once per answer.
#[derive(Default)]
pub struct Resource {
    pub ticket: crate::plugin_host::machines::Ticket,
    pub value: Option<(i64, Rc<RegistryKey>, Rc<RegistryKey>)>,
}

pub struct Half {
    pub id: Rc<str>,
    pub kind: Kind,
    /// A migrated bundled half explicitly invalidates its surfaces; other 1.0 plugins keep the implicit redraw.
    pub owns_redraw: bool,
    /// The VM whose setup filled `made`, waiting for the host to adopt it.
    pub made: RefCell<Option<(u64, Contribs)>>,
    /// What the running VM contributed.
    pub contribs: RefCell<Contribs>,
    /// Host signals by VM and name (`Sessions.Changed`), made as a field is first read.
    pub signals: RefCell<HashMap<(u64, String), Value>>,
    /// `Server:OnEvent(prefix)` signals, by VM.
    pub events: RefCell<Vec<(u64, String, Value)>>,
    /// Fires a script signal, by VM: `function(s, ...) s:Fire(...) end`.
    pub fire: RefCell<HashMap<u64, Function>>,
    /// The editors the running setup made, by creation order: the n-th is the window's n-th
    /// field of this plugin, kept across reloads.
    pub editors_made: Cell<usize>,
    pub resources: RefCell<HashMap<String, Resource>>,
    /// What each surface read when it last ran, by surface id.
    pub reads: RefCell<HashMap<String, Vec<Dep>>>,
    pub catalogs: RefCell<crate::plugin_host::Catalogs>,
    /// Its processes, and the inbox their threads fill.
    pub procs: RefCell<HashMap<u64, crate::plugin_host::services::process::Proc>>,
    pub inbox: crate::plugin_host::services::process::Inbox,
    /// `Process:RunAsync`'s processes, by VM, killed with the half.
    pub runs: RefCell<Vec<(u64, crate::plugin_host::process::Control)>>,
    /// What the half holds of the host's process-wide services (display locks, prompt
    /// listeners), by VM, let go with the half.
    pub held: RefCell<Vec<(u64, crate::plugin_host::services::Held)>>,
    /// Bumped when anything a surface may read from this half changes outside a render.
    pub epoch: Cell<u64>,
    /// What each watched signal saw last (`services::watch`).
    pub seen: RefCell<HashMap<String, u64>>,
    /// What `Sessions.Changed` read when it last compared the sessions' versions.
    pub(crate) sessions_read: Cell<Option<crate::plugin_host::services::SessionInputs>>,
    /// Shell slots whose render failed in the running VM: they draw nothing until a reload.
    pub failed: RefCell<std::collections::HashSet<String>>,
}

impl Half {
    pub fn new(id: &str, kind: Kind, tier: crate::plugin_host::Tier) -> Rc<Half> {
        Rc::new(Half {
            id: id.into(),
            kind,
            owns_redraw: super::redraw::explicit(tier, id),
            made: RefCell::default(),
            contribs: RefCell::default(),
            signals: RefCell::default(),
            events: RefCell::default(),
            fire: RefCell::default(),
            editors_made: Cell::new(0),
            resources: RefCell::default(),
            reads: RefCell::default(),
            catalogs: RefCell::default(),
            procs: RefCell::default(),
            inbox: Default::default(),
            runs: RefCell::default(),
            held: RefCell::default(),
            epoch: Cell::new(0),
            seen: RefCell::default(),
            sessions_read: Cell::default(),
            failed: RefCell::default(),
        })
    }

    /// Takes what VM `vm` contributed, if its setup is the one that ran last. Forgets the
    /// signals, events and cached values of the VM it replaces.
    pub fn adopt(&self, vm: Option<u64>) -> bool {
        let made = self.made.borrow_mut().take();
        match (made, vm) {
            (Some((made_by, contribs)), Some(vm)) if made_by == vm => {
                *self.contribs.borrow_mut() = contribs;
                self.signals.borrow_mut().retain(|(v, _), _| *v == vm);
                self.events.borrow_mut().retain(|(v, _, _)| *v == vm);
                self.fire.borrow_mut().retain(|v, _| *v == vm);
                self.resources.borrow_mut().values_mut().for_each(|r| r.value = None);
                self.reads.borrow_mut().clear();
                self.failed.borrow_mut().clear();
                self.epoch.set(self.epoch.get() + 1);
                true
            }
            _ => false,
        }
    }
}
