//! Contract contribution types, spec decoder, withdrawal helpers, and `@opencode/plugin` VM
//! module bootstrap shared across Rune plugin hosts.

use std::collections::BTreeMap;

use mlua::{FromLua, Function, Lua, Table, Value};
use opencode_plugin_spec::{self as spec, ContributionKind, Record};
use rune_luau::{Data, Host};
use serde::de::DeserializeOwned;

/// Which half of a plugin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Window,
    App,
}

/// What a half holds of a process-wide service, let go when the half goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Held {
    /// A `Power:KeepAwake` lock.
    Power(u64),
    /// An `Askpass:Listen` endpoint.
    Askpass(u64),
}

/// One contribution: its decoded spec, the functions it handed over, and its interned key.
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

/// Which half may add contribution `kind` (the contract's `halves`).
pub fn allowed(kind: ContributionKind, half: Kind) -> bool {
    use ContributionKind as K;
    match kind {
        K::MenubarItem => half == Kind::App,
        K::ServerSource | K::McpTool => true,
        _ => half == Kind::Window,
    }
}

impl Contribs {
    /// Decodes `t` as contribution `kind` (calling `warn` on unknown keys), keeps its callbacks,
    /// and replaces any earlier contribution with the same `key`.
    pub fn add(&mut self, lua: &Lua, kind: ContributionKind, t: &Table, key: &'static str, warn: &dyn Fn(&str)) -> mlua::Result<()> {
        use ContributionKind as K;
        match kind {
            K::Command => push(lua, &mut self.commands, t, key, warn),
            K::Panel => push(lua, &mut self.panels, t, key, warn),
            K::Setting => push(lua, &mut self.settings, t, key, warn),
            K::SettingsPage => push(lua, &mut self.settings_pages, t, key, warn),
            K::MenuItem => push(lua, &mut self.menu_items, t, key, warn),
            K::MenubarItem => push(lua, &mut self.menubar_items, t, key, warn),
            K::LinkHandler => push(lua, &mut self.link_handlers, t, key, warn),
            K::TitlebarItem => push(lua, &mut self.titlebar_items, t, key, warn),
            K::ComposerPill => push(lua, &mut self.composer_pills, t, key, warn),
            K::CodeBlock => push(lua, &mut self.code_blocks, t, key, warn),
            K::CodeBlockAction => push(lua, &mut self.code_block_actions, t, key, warn),
            K::Slot => push(lua, &mut self.slots, t, key, warn),
            K::Theme => push(lua, &mut self.themes, t, key, warn),
            K::FileViewer => push(lua, &mut self.file_viewers, t, key, warn),
            K::ServerSource => push(lua, &mut self.server_sources, t, key, warn),
            K::Dialog => push(lua, &mut self.dialogs, t, key, warn),
            K::Popover => push(lua, &mut self.popovers, t, key, warn),
            K::McpTool => push(lua, &mut self.mcp_tools, t, key, warn),
        }
    }

    /// Takes contribution `kind`/`id` out (`Contribution:Withdraw`). True when it was there.
    pub fn withdraw(&mut self, kind: ContributionKind, id: &str) -> bool {
        use ContributionKind as K;
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

/// Decodes `t` as `S` (calling `warn` per unknown key), keeps its functions, and replaces any
/// earlier contribution with the same key.
fn push<S: DeserializeOwned + Record>(lua: &Lua, list: &mut Vec<Contrib<S>>, t: &Table, key: &'static str, warn: &dyn Fn(&str)) -> mlua::Result<()> {
    let data = Data::from_lua(Value::Table(t.clone()), lua)?;
    if let Data::Map(fields) = &data {
        for name in fields.keys().filter(|k| !S::knows(k)) {
            warn(&format!("{}: unknown key {name:?}", S::NAME));
        }
    }
    let spec: S = data.decode().map_err(|e| mlua::Error::runtime(format!("{}: {e}", S::NAME)))?;
    let mut fns = BTreeMap::new();
    // A key that takes a value or a function (`enabled`) keeps only a function here; its value
    // decoded into the spec.
    for name in S::CALLBACKS {
        if let Value::Function(f) = t.raw_get::<Value>(*name)? {
            fns.insert(*name, f);
        }
    }
    list.retain(|c| c.key != key);
    list.push(Contrib { spec, fns, key });
    Ok(())
}

const PLUGIN_MODULE: &str = r#"
local hot = ...
local function define(half)
    return function(setup)
        if type(setup) ~= "function" then
            error(`plugin.define{if half == "window" then "Window" else "App"} needs a setup function`, 2)
        end
        return {
            __half = half,
            __setup = setup,
            saveHotState = function()
                if hot.save then
                    return hot.save()
                end
                return nil
            end,
            loadHotState = function(state)
                if hot.load and state ~= nil then
                    hot.load(state)
                end
            end,
        }
    end
end
return { contract = CONTRACT, defineWindow = define("window"), defineApp = define("app") }
"#;

/// Registers `@opencode/plugin` and `@opencode/ui` in `host`'s VM, returning the hot-state table
/// shared with `ctx:BindHotState`.
pub fn register_modules(host: &Host<'_>) -> mlua::Result<Table> {
    let lua = host.lua();
    let hot = lua.create_table()?;
    // The module's text with the contract in it, made once: every VM loads the same bytecode.
    static CODE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let code = CODE.get_or_init(|| PLUGIN_MODULE.replace("CONTRACT", &format!("{:?}", spec::CONTRACT)));
    let plugin: Table = crate::chunk::load(lua, code).set_name("=@opencode/plugin").call(hot.clone())?;
    host.register_module("@opencode/plugin", plugin)?;
    let ui: Table = lua.named_registry_value::<Table>("_REGISTEREDMODULES")?.raw_get("@rune/ui")?;
    host.register_module("@opencode/ui", ui)?;
    Ok(hot)
}

/// Checks that `module` returned from a plugin entrypoint matches `kind` (`defineWindow` / `defineApp`).
pub fn verify_module_half(module: &Table, kind: Kind) -> mlua::Result<()> {
    let want = if kind == Kind::Window { "window" } else { "app" };
    let half: Option<String> = module.raw_get("__half")?;
    if half.as_deref() != Some(want) {
        let define = if kind == Kind::Window { "defineWindow" } else { "defineApp" };
        return Err(mlua::Error::runtime(format!("the module must return plugin.{define}(function(ctx) ... end)")));
    }
    Ok(())
}
