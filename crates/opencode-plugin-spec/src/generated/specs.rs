//! Contribution specs and the records nested in them.
//! Generated from crates/opencode-plugin-spec/contract.json by `cargo run -p opencode-plugin-spec -- --write`. Do not edit.

#![allow(unused_imports)]

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::*;
use crate::{ColorSpec, impl_record};

/// A command's slash form in the composer.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CommandSlash {
    /// The name after the slash, such as `open` for `/open`.
    pub name: String,
    /// The text after the name goes to `run`.
    pub arguments: Option<bool>,
    /// Lists the entry right after the slash entry with this name.
    pub after: Option<String>,
}
impl_record!(CommandSlash, ["name", "arguments", "after"], [], []);

/// A command in the palette, with an optional keybind and slash command. Published as `<plugin>.<id>`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CommandSpec {
    /// Its id within the plugin.
    pub id: String,
    /// The palette label.
    pub title: String,
    /// The palette's second line.
    pub description: Option<String>,
    /// More words the palette matches, separated by spaces.
    pub keywords: Option<String>,
    /// The palette category.
    pub group: Option<String>,
    /// The Settings › Shortcuts section. Defaults to `general`.
    pub section: Option<CommandSection>,
    /// The default keybind, such as `"mod+shift+t"`.
    pub bind: Option<String>,
    /// Also offers it as a slash command.
    pub slash: Option<CommandSlash>,
    /// Keeps it out of the palette. The keybind still works.
    pub hidden: Option<bool>,
    /// Also lists it under Suggested.
    pub suggested: Option<bool>,
    /// Lists it when the palette opens without a query.
    pub featured: Option<bool>,
    /// False takes it out of the palette and turns its keybind off. Defaults to true.
    pub enabled: Option<bool>,
    /// The keybind also fires while a text field has focus.
    pub editable: Option<bool>,
    /// Offered only while a session is open.
    pub requires_session: Option<bool>,
}
impl_record!(CommandSpec, ["id", "title", "description", "keywords", "group", "section", "bind", "slash", "hidden", "suggested", "featured", "enabled", "editable", "requiresSession"], ["run"], []);

/// A panel: tabs in a session's side region.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct PanelSpec {
    /// Its id within the plugin.
    pub id: String,
    /// Its tab and launcher title.
    pub title: String,
    /// The launcher card's second line.
    pub description: Option<String>,
    /// Its icon.
    pub icon: Option<String>,
    /// Launcher order, ascending.
    pub order: Option<i64>,
    /// Asks for the wider session minimum while open.
    pub wide: Option<bool>,
    /// A width of its own for the side region, in points.
    pub width: Option<f64>,
    /// Its tabs are not restored.
    pub transient: Option<bool>,
    /// Offered in the + menu and the New tab view. Defaults to true.
    pub launcher: Option<bool>,
    /// Its tab sits at the front of the strip.
    pub first: Option<bool>,
    /// Its tab is selected when the selected tab closes.
    pub fallback: Option<bool>,
}
impl_record!(PanelSpec, ["id", "title", "description", "icon", "order", "wide", "width", "transient", "launcher", "first", "fallback"], ["list", "render", "label", "onClose", "onFocus"], []);

/// A row in Settings › Plugins, under this plugin.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SettingSpec {
    /// Its search id.
    pub id: String,
    /// The row's title.
    pub title: String,
    /// A line under the title.
    pub description: Option<String>,
    /// More words settings search matches.
    pub keywords: Option<String>,
}
impl_record!(SettingSpec, ["id", "title", "description", "keywords"], ["render"], []);

/// A settings search entry, indexed without drawing the page.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SettingEntry {
    /// The `ui.settingsRow` id search reveals.
    pub id: String,
    /// The title in search results.
    pub title: String,
    /// Its second line.
    pub description: Option<String>,
    /// More words search matches.
    pub keywords: Option<String>,
}
impl_record!(SettingEntry, ["id", "title", "description", "keywords"], [], []);

/// A settings page, or a section on a host page.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SettingsPageSpec {
    /// The page's id, also for `Layout:OpenSettings`.
    pub id: String,
    /// Its navigation label.
    pub title: String,
    /// Its navigation icon.
    pub icon: Option<String>,
    /// Adds a section to this host page instead of a page of its own.
    pub page: Option<HostSettingsPage>,
    /// Lists the page in the About group, such as release notes.
    pub about: Option<bool>,
    /// Search entries.
    pub entries: Option<Vec<SettingEntry>>,
}
impl_record!(SettingsPageSpec, ["id", "title", "icon", "page", "about", "entries"], ["render"], []);

/// An item of a host menu.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct MenuItemSpec {
    /// The menu.
    pub menu: MenuName,
    /// Its id within the plugin.
    pub id: String,
    /// Its label.
    pub title: String,
    /// Ascending order.
    pub order: Option<i64>,
    /// `session.panel`: its icon.
    pub icon: Option<String>,
    /// `session.panel`: a published command id whose shortcut it shows.
    pub keybind: Option<String>,
    /// `session.panel`: muted detail in launcher lists.
    pub description: Option<String>,
    /// `session.panel`: + runs this item directly.
    pub primary: Option<bool>,
}
impl_record!(MenuItemSpec, ["menu", "id", "title", "order", "icon", "keybind", "description", "primary"], ["when", "enabled", "run"], []);

/// An item of the app menu bar.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct MenubarItemSpec {
    /// The submenu.
    pub menu: MenubarMenu,
    /// Its id within the plugin.
    pub id: String,
    /// Its label.
    pub label: String,
    /// Ascending order.
    pub order: Option<i64>,
    /// Its accelerator, such as `"mod+,"`.
    pub bind: Option<String>,
    /// False shows it disabled.
    pub enabled: Option<bool>,
}
impl_record!(MenubarItemSpec, ["menu", "id", "label", "order", "bind", "enabled"], ["run"], []);

/// Opens local links it matches. The matching handler with the highest priority wins.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct LinkHandlerSpec {
    /// Its id within the plugin.
    pub id: String,
    /// Higher wins. Defaults to 0.
    pub priority: Option<i64>,
}
impl_record!(LinkHandlerSpec, ["id", "priority"], ["match", "open", "exists"], []);

/// A pill at the titlebar's end.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct TitlebarItemSpec {
    /// Its id within the plugin.
    pub id: String,
}
impl_record!(TitlebarItemSpec, ["id"], ["show", "run"], []);

/// A pill in the row above a session's composer.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ComposerPillSpec {
    /// Its id within the plugin.
    pub id: String,
    /// Ascending order among plugin pills.
    pub order: Option<i64>,
}
impl_record!(ComposerPillSpec, ["id", "order"], ["show", "renderMenu", "run"], []);

/// Renders the fenced code blocks of some languages as your own UI.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CodeBlockSpec {
    /// Its id within the plugin.
    pub id: String,
    /// The languages it renders. The host resolves no aliases.
    pub languages: Vec<String>,
}
impl_record!(CodeBlockSpec, ["id", "languages"], ["render"], []);

/// A button beside a code block's Copy button.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CodeBlockActionSpec {
    /// Its id within the plugin.
    pub id: String,
    /// The languages whose blocks show it.
    pub languages: Vec<String>,
    /// Its icon.
    pub icon: String,
    /// Its accessible name and tooltip.
    pub title: String,
    /// Ascending order, before Copy.
    pub order: Option<i64>,
}
impl_record!(CodeBlockActionSpec, ["id", "languages", "icon", "title", "order"], ["run"], []);

/// Content for one of the shell's slots.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SlotSpec {
    /// Its id within the plugin.
    pub id: String,
    /// The slot.
    pub at: SlotName,
    /// Ascending order.
    pub order: Option<i64>,
}
impl_record!(SlotSpec, ["id", "at", "order"], ["render"], []);

/// The seed colors a theme variant generates its scales from, as `#RRGGBB` strings.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ThemeSeeds {
    /// The gray scale's base.
    pub neutral: String,
    /// The brand and accent color.
    pub primary: String,
    /// Success states.
    pub success: String,
    /// Warning states.
    pub warning: String,
    /// Error states.
    pub error: String,
    /// Informational states.
    pub info: String,
    /// Links and interactive elements.
    pub interactive: String,
    /// Added lines.
    pub diff_add: String,
    /// Deleted lines.
    pub diff_delete: String,
}
impl_record!(ThemeSeeds, ["neutral", "primary", "success", "warning", "error", "info", "interactive", "diffAdd", "diffDelete"], [], []);

/// One color scheme of a theme.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ThemeVariant {
    /// The seed colors.
    pub seeds: ThemeSeeds,
    /// Exact values for single tokens of the app's theme file format.
    pub overrides: Option<BTreeMap<String, String>>,
}
impl_record!(ThemeVariant, ["seeds", "overrides"], [], []);

/// A color theme, listed in Settings › Appearance as `<plugin>:<id>`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ThemeSpec {
    /// Its id within the plugin.
    pub id: String,
    /// The name the theme list shows.
    pub name: String,
    /// The light scheme.
    pub light: ThemeVariant,
    /// The dark scheme.
    pub dark: ThemeVariant,
}
impl_record!(ThemeSpec, ["id", "name", "light", "dark"], [], []);

/// Shows files with some extensions in the file panel.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct FileViewerSpec {
    /// Its id within the plugin.
    pub id: String,
    /// Its name, such as `"Word document"`.
    pub title: String,
    /// The extensions it shows, without the dot.
    pub extensions: Vec<String>,
}
impl_record!(FileViewerSpec, ["id", "title", "extensions"], ["render"], []);

/// A source of servers, such as SSH hosts or WSL distributions.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ServerSourceSpec {
    /// Its id within the plugin.
    pub id: String,
    /// Its name in Settings › Servers.
    pub title: String,
    /// Ascending order among sources.
    pub order: Option<i64>,
}
impl_record!(ServerSourceSpec, ["id", "title", "order"], ["row", "connect", "remove"], []);

/// A modal dialog, opened with `Dialog:Open`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct DialogSpec {
    /// Its id within the plugin.
    pub id: String,
    /// Its heading and accessible name.
    pub title: Option<String>,
    /// Its width.
    pub size: Option<DialogSize>,
}
impl_record!(DialogSpec, ["id", "title", "size"], ["render", "onDismiss"], []);

/// Popover content, opened with `Popover:Open` or `ui.flyoutRow`. The host owns placement, focus and dismissal.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct PopoverSpec {
    /// Its id within the plugin.
    pub id: String,
    /// Its heading and accessible name.
    pub title: Option<String>,
    /// Its width in points.
    pub width: Option<f64>,
}
impl_record!(PopoverSpec, ["id", "title", "width"], ["render"], []);

/// A tool on the app's client MCP server, named `<plugin>_<name>`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct McpToolSpec {
    /// Its name within the plugin.
    pub name: String,
    /// What it does, for the model.
    pub description: String,
    /// Its arguments' JSON Schema.
    pub input_schema: BTreeMap<String, Value>,
}
impl_record!(McpToolSpec, ["name", "description", "inputSchema"], ["call"], []);
