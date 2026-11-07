//! Safe builders for the contribution registries: commands, panels and settings rows.

use std::borrow::Cow;

use crate::abi::{
    AbiStatus, AbiStr, COMMAND_EDITABLE, COMMAND_HIDDEN, Callback, CommandSpecAbi, PANEL_FALLBACK, PANEL_FIRST, PANEL_FIXED, PANEL_NO_LAUNCHER,
    PANEL_TRANSIENT, PanelSpecAbi, RenderCallback, SettingSpecAbi,
};
use crate::boundary::drop_packed;

type Text = Cow<'static, str>;

/// Settings › Shortcuts' groups (Desktop's `CommandSection`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CommandSection {
    #[default]
    General = 0,
    Session = 1,
    Navigation = 2,
    Model = 3,
    Terminal = 4,
    Prompt = 5,
}

/// A command for the palette, with an optional keybind (`Command` in Desktop's plugin API).
/// Published as `<plugin>.<id>`.
#[derive(Clone, Debug)]
pub struct CommandSpec {
    id: Text,
    title: Text,
    description: Text,
    keywords: Text,
    slash: Text,
    binds: [Text; 3],
    section: CommandSection,
    editable: bool,
    hidden: bool,
}

impl CommandSpec {
    pub fn new(id: impl Into<Text>, title: impl Into<Text>) -> Self {
        CommandSpec {
            id: id.into(),
            title: title.into(),
            description: Text::Borrowed(""),
            keywords: Text::Borrowed(""),
            slash: Text::Borrowed(""),
            binds: [Text::Borrowed(""), Text::Borrowed(""), Text::Borrowed("")],
            section: CommandSection::General,
            editable: false,
            hidden: false,
        }
    }

    pub fn description(mut self, description: impl Into<Text>) -> Self {
        self.description = description.into();
        self
    }

    /// Extra words the palette matches.
    pub fn keywords(mut self, keywords: impl Into<Text>) -> Self {
        self.keywords = keywords.into();
        self
    }

    /// The `/name` that runs it from the composer.
    pub fn slash(mut self, name: impl Into<Text>) -> Self {
        self.slash = name.into();
        self
    }

    /// One keybind on every OS, in `rune::Keybind` syntax: `mod+shift+h` (`mod` is ⌘ on macOS,
    /// Ctrl elsewhere).
    pub fn bind(mut self, keybind: impl Into<Text>) -> Self {
        let k = keybind.into();
        self.binds = [k.clone(), k.clone(), k];
        self
    }

    /// Keybinds per OS: Windows, macOS, Linux. Empty for none on that OS.
    pub fn bind_per_os(mut self, windows: impl Into<Text>, macos: impl Into<Text>, linux: impl Into<Text>) -> Self {
        self.binds = [windows.into(), macos.into(), linux.into()];
        self
    }

    pub fn section(mut self, section: CommandSection) -> Self {
        self.section = section;
        self
    }

    /// The keybind fires inside text fields too.
    pub fn editable(mut self, editable: bool) -> Self {
        self.editable = editable;
        self
    }

    /// Runs by keybind or `commands.run` but isn't listed in the palette.
    pub fn hidden(mut self, hidden: bool) -> Self {
        self.hidden = hidden;
        self
    }

    pub(crate) fn with_abi(&self, run: Callback, f: impl FnOnce(&CommandSpecAbi) -> AbiStatus) {
        let abi = CommandSpecAbi {
            id: AbiStr::new(&self.id),
            title: AbiStr::new(&self.title),
            description: AbiStr::new(&self.description),
            keywords: AbiStr::new(&self.keywords),
            slash: AbiStr::new(&self.slash),
            bind_windows: AbiStr::new(&self.binds[0]),
            bind_macos: AbiStr::new(&self.binds[1]),
            bind_linux: AbiStr::new(&self.binds[2]),
            section: self.section as u32,
            flags: if self.editable { COMMAND_EDITABLE } else { 0 } | if self.hidden { COMMAND_HIDDEN } else { 0 },
            run,
        };
        if f(&abi) != AbiStatus::Ok {
            drop_packed(run.data, run.drop_data);
        }
    }
}

/// A tab in the session side region (`Panel` in Desktop's plugin API).
#[derive(Clone, Debug)]
pub struct PanelSpec {
    id: Text,
    title: Text,
    description: Text,
    icon: Text,
    order: i32,
    flags: u32,
}

impl PanelSpec {
    pub fn new(id: impl Into<Text>, title: impl Into<Text>) -> Self {
        PanelSpec { id: id.into(), title: title.into(), description: Text::Borrowed(""), icon: Text::Borrowed(""), order: 0, flags: 0 }
    }

    fn flag(mut self, flag: u32, on: bool) -> Self {
        self.flags = if on { self.flags | flag } else { self.flags & !flag };
        self
    }

    /// No New tab card or `+` row: something else opens it, as Desktop's panels without a
    /// `MenuItem` (Context opens from its header ring).
    pub fn launcher(self, on: bool) -> Self {
        self.flag(PANEL_NO_LAUNCHER, !on)
    }

    /// Stored at the front of the strip (`first`).
    pub fn first(self, on: bool) -> Self {
        self.flag(PANEL_FIRST, on)
    }

    /// Selected when the active tab closes (`fallback`).
    pub fn fallback(self, on: bool) -> Self {
        self.flag(PANEL_FALLBACK, on)
    }

    /// Can't be dragged to another pane (`draggable: false`).
    pub fn fixed(self, on: bool) -> Self {
        self.flag(PANEL_FIXED, on)
    }

    /// The launcher card's line under the title (the manifest's description when empty).
    pub fn description(mut self, description: impl Into<Text>) -> Self {
        self.description = description.into();
        self
    }

    /// An icon this plugin interned (`cx.icon`), or a host icon name for the tab and launcher
    /// (`"code"`, `"globe"`, `"console"`, `"file-tree"`, `"folder"`).
    pub fn icon(mut self, icon: impl Into<Text>) -> Self {
        self.icon = icon.into();
        self
    }

    /// Ascending display order in the `+` menu.
    pub fn order(mut self, order: i32) -> Self {
        self.order = order;
        self
    }

    /// Closing the tab forgets it rather than keeping it in Recently closed.
    pub fn transient(self, transient: bool) -> Self {
        self.flag(PANEL_TRANSIENT, transient)
    }

    pub(crate) fn with_abi(&self, render: RenderCallback, label: RenderCallback, f: impl FnOnce(&PanelSpecAbi) -> AbiStatus) {
        let abi = PanelSpecAbi {
            id: AbiStr::new(&self.id),
            title: AbiStr::new(&self.title),
            description: AbiStr::new(&self.description),
            icon: AbiStr::new(&self.icon),
            order: self.order,
            flags: self.flags,
            render,
            label,
        };
        if f(&abi) != AbiStatus::Ok {
            drop_packed(render.data, render.drop_data);
            drop_packed(label.data, label.drop_data);
        }
    }
}

/// A row in Settings (`SettingsPage` entries in Desktop's plugin API): title, description and
/// a control the plugin draws. Indexed for settings search without rendering.
#[derive(Clone, Debug)]
pub struct SettingSpec {
    id: Text,
    title: Text,
    description: Text,
    keywords: Text,
}

impl SettingSpec {
    pub fn new(id: impl Into<Text>, title: impl Into<Text>) -> Self {
        SettingSpec { id: id.into(), title: title.into(), description: Text::Borrowed(""), keywords: Text::Borrowed("") }
    }

    pub fn description(mut self, description: impl Into<Text>) -> Self {
        self.description = description.into();
        self
    }

    pub fn keywords(mut self, keywords: impl Into<Text>) -> Self {
        self.keywords = keywords.into();
        self
    }

    pub(crate) fn with_abi(&self, render: RenderCallback, f: impl FnOnce(&SettingSpecAbi) -> AbiStatus) {
        let abi = SettingSpecAbi {
            id: AbiStr::new(&self.id),
            title: AbiStr::new(&self.title),
            description: AbiStr::new(&self.description),
            keywords: AbiStr::new(&self.keywords),
            render,
        };
        if f(&abi) != AbiStatus::Ok {
            drop_packed(render.data, render.drop_data);
        }
    }
}
