//! The heavy views a plugin places but the host draws (the contract's `native` nodes) and the
//! objects they draw: `ui.diffView` over a `DiffDocument` (`Diff:Open`), `ui.fileTree` over a
//! `FileTreeModel`, the panel frame `ui.panelSidebar` and the `ui.openInButton` split button.
//! The engines behind them stay the app's (`review`, `files`, `code_view`, `open_in`); a plugin
//! decides what they show and when.
//!
//! A render reads `&App`. A node whose props ask for another state than the engine holds (a
//! diff's layout, the file it previews) hands the change to the next frame (`App::later`).

pub mod diff;
pub mod files;
pub mod icons;
pub mod open_in;
pub mod segmented;
pub mod sidebar;
pub mod tree;

use std::rc::Rc;

use mlua::{Lua, Table, Value};
use rune::{KeyEvent, Tx};
use rune_luau::{Data, HostNode, NodeArgs};

use super::vm::Vmx;
use crate::App;
use crate::palette::{Pal, pal};

/// The native nodes, for the script's `ui`.
pub fn nodes() -> Vec<HostNode> {
    vec![
        HostNode::new("diffView", diff::view),
        HostNode::new("codeView", files::view),
        HostNode::new("fileTree", tree::view),
        HostNode::new("panelSidebar", sidebar::view),
        HostNode::new("openInButton", open_in::view),
        HostNode::new("segmented", segmented::view),
    ]
}

/// The app's icon named `name` (`ui.icon`'s names).
pub(crate) fn icon(name: &str) -> &'static rune::Icon {
    crate::plugin_host::icons::resolve("", name)
}

/// Lands what the engines read, and reads again what file events made stale.
pub fn poll(app: &mut App, tx: &mut Tx) -> bool {
    crate::review::poll(app, tx) | files::poll(app, tx)
}

/// Keys for the native views on screen: the panel sidebar's toggle, then the review's diff and
/// the open file.
pub fn on_key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    sidebar::key(app, ev, tx) || diff::key(app, ev, tx) || files::key(app, ev, tx)
}

/// The palette a node draws in.
pub(crate) fn p(a: &NodeArgs) -> Pal {
    *pal(a.dark())
}

/// An object handed to a plugin: `fields` as its own, `methods` under its metatable's `__index`,
/// and `__type` naming it.
pub(crate) fn object(lua: &Lua, ty: &str, fields: &[(&str, Value)], methods: Table) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    for (k, v) in fields {
        t.raw_set(*k, v.clone())?;
    }
    let mt = lua.create_table()?;
    mt.raw_set("__index", methods)?;
    mt.raw_set("__type", ty)?;
    t.set_metatable(Some(mt))?;
    Ok(t)
}

/// The string field `field` of object prop `prop` (a handle's `sessionId`).
pub(crate) fn handle_field(a: &NodeArgs, prop: &str, field: &str) -> Option<String> {
    a.data(prop)?.get(field).as_str().map(str::to_owned)
}

/// A map prop's string entries (`kinds`).
pub(crate) fn string_map(d: Option<Data>) -> std::collections::HashMap<String, String> {
    match d {
        Some(Data::Map(m)) => m.into_iter().filter_map(|(k, v)| Some((k, v.as_str()?.to_owned()))).collect(),
        _ => Default::default(),
    }
}

/// Service tables of the native objects (`Diff`, `Workspace`'s trees and documents).
pub fn build(lua: &Lua, vmx: &Rc<Vmx>, name: &str, t: &Table) -> mlua::Result<()> {
    match name {
        "Diff" => diff::build(lua, vmx, t),
        "Workspace" => files::build(lua, t),
        _ => Ok(()),
    }
}
