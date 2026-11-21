//! OpenCode Console extension sources and link components for Settings › Extensions and
//! Project › Extensions (`packages/app/src/settings/providers/console-link.tsx`,
//! `anomalyco/opencode#63125`).

use opencode_i18n::t;
use opencode_kit::model::{ConsoleMcp, ConsoleMcpLinks, ConsoleWorkspace, SkillInfo, console_mcp_links, console_skill_link, console_workspace};
use opencode_ui::focus::FocusExt;
use opencode_ui::palette::{Pal, kit};
use opencode_ui::widgets;
use rune::{Cursor, El, Icon, Span, Tx, icon, row, text};
use rune_kit::{ButtonVariant, ControlSize};

use super::{BODY, NAME, NAME_REGULAR};
use crate::host::SettingsHost;

/// `packages/ui/src/icons/catalog.ts` `"arrow-up-right"`.
pub(crate) static ARROW_UP_RIGHT: Icon = Icon {
    name: "arrow-up-right",
    view: 16.0,
    body: r#"<path d="M11 9.56V5H6.44M11 5L5 11" stroke="currentColor"/>"#,
};

/// Loaded Console-managed MCP servers for one Extensions page.
/// `None` = loading, `Some(Ok(_))` = ready (or unsupported on older servers), `Some(Err(()))` = failed.
#[derive(Clone, Debug, Default)]
pub struct ConsoleSources {
    pub read: Option<Result<Vec<ConsoleMcp>, ()>>,
}

impl ConsoleSources {
    pub fn apply(&mut self, result: Result<Vec<ConsoleMcp>, ()>) {
        self.read = Some(result);
    }

    pub fn mcps(&self) -> &[ConsoleMcp] {
        match &self.read {
            Some(Ok(items)) => items,
            _ => &[],
        }
    }

    pub fn failed(&self) -> bool {
        matches!(self.read, Some(Err(())))
    }

    pub fn mcp(&self, name: &str) -> Option<&ConsoleMcp> {
        self.mcps().iter().find(|item| item.name == name)
    }

    /// Console's managed entry for `name` when NOT replaced by the user's config.
    pub fn managed(&self, name: &str) -> Option<&ConsoleMcp> {
        self.mcp(name).filter(|item| !item.overridden)
    }

    pub fn links(&self, name: &str) -> Option<ConsoleMcpLinks> {
        self.mcp(name).map(console_mcp_links)
    }

    /// Connect URL for a Console MCP server whose account isn't connected yet (`protocol: 2026-07-28`).
    pub fn connect(&self, name: &str) -> Option<String> {
        let managed = self.managed(name)?;
        managed.needs_connection.then(|| console_mcp_links(managed).connect)
    }

    pub fn workspace(&self) -> Option<ConsoleWorkspace> {
        console_workspace(self.mcps())
    }

    pub fn skill(&self, skill: &SkillInfo, signed_in: bool) -> Option<String> {
        console_skill_link(&skill.id, &skill.path, self.workspace().as_ref(), signed_in)
    }
}

/// Whether the user has an active OpenCode Console credential on the server (`console-link.tsx:47`).
pub fn console_signed_in(s: &dyn SettingsHost, dir: Option<&str>) -> bool {
    let list = s.integrations(dir).or_else(|| dir.and_then(|_| s.integrations(None)));
    list.is_some_and(|items| {
        items
            .iter()
            .find(|i| i.id == "opencode")
            .is_some_and(|i| i.connections.iter().any(|c| c.credential))
    })
}

/// `.settings-console-name`: an extension's name that opens where OpenCode Console manages it.
pub fn console_name<H: SettingsHost + 'static>(p: &Pal, key: String, name: &str, href: String, muted: bool) -> El {
    let ink = if muted { p.muted } else { p.text };
    let label = t!("settings.extensions.console.manage", name = name);
    let text_el = text(name.to_owned())
        .font(NAME)
        .color(ink)
        .group_color(p.text)
        .spans(vec![Span::new(0, name.len()).underline(p.q.border_strong)])
        .truncate()
        .help_when_truncated(name.to_owned())
        .help_above()
        .shrink()
        .min_w(0.0);
    row()
        .key(key)
        .role("button")
        .label(label)
        .shrink()
        .min_w(0.0)
        .gap(2.0)
        .center_y()
        .radius(4.0)
        .cursor(Cursor::Pointer)
        .focus_default(p)
        .hover_group_focus()
        .hover(p.hover)
        .on_press_tx(move |s: &mut H, tx: &mut Tx| crate::open_external(s, &href, tx))
        .child(text_el)
        .child(icon(&ARROW_UP_RIGHT, 16.0).color(p.icon_muted).group_color(p.icon))
}

/// `.settings-mcp-source`: shown after a locally overridden MCP server, linking to Console's entry.
pub fn console_replaced<H: SettingsHost + 'static>(p: &Pal, key: String, name: &str, href: String) -> El {
    let words = t!("settings.extensions.console.replaced");
    let label = t!("settings.extensions.console.manage", name = name);
    row()
        .key(key)
        .role("button")
        .label(label)
        .shrink()
        .min_w(0.0)
        .gap(2.0)
        .center_y()
        .radius(4.0)
        .cursor(Cursor::Pointer)
        .focus_default(p)
        .hover_group_focus()
        .hover(p.hover)
        .on_press_tx(move |s: &mut H, tx: &mut Tx| crate::open_external(s, &href, tx))
        .child(text(words).font(NAME_REGULAR).color(p.muted).group_color(p.text).truncate().shrink().min_w(0.0))
        .child(icon(&ARROW_UP_RIGHT, 16.0).color(p.icon_muted).group_color(p.icon))
}

/// Small `ghost-muted` Connect button shown in a switch's place for Console MCPs needing connection.
pub fn console_connect_button<H: SettingsHost + 'static>(p: &Pal, key: String, name: &str, href: String) -> El {
    let a11y = t!("settings.extensions.console.connect", name = name);
    widgets::button(t!("common.connect"), ButtonVariant::GhostMuted)
        .control_size(ControlSize::Regular)
        .on_press_tx(move |s: &mut H, tx: &mut Tx| crate::open_external(s, &href, tx))
        .build(kit(p))
        .key(key)
        .label(a11y)
}

/// Inline `ExtensionLoadFailed` row (`settings/extension-load.tsx`) with a Retry action.
pub fn load_failed<H: SettingsHost + 'static>(
    p: &Pal,
    key: &str,
    message: &str,
    retry: &str,
    on_retry: impl Fn(&mut dyn SettingsHost, &mut Tx) + 'static,
) -> El {
    row()
        .key(key.to_owned())
        .role("alert")
        .gap(8.0)
        .center_y()
        .child(text(message.to_owned()).font(BODY).color(p.danger))
        .child(
            text(retry.to_owned())
                .key(format!("{key}-retry"))
                .role("button")
                .label(retry.to_owned())
                .font(BODY)
                .color(p.text)
                .spans(vec![Span::new(0, retry.len()).underline(p.text)])
                .on_press_tx(move |s: &mut H, tx: &mut Tx| {
                    tx.instant();
                    on_retry(s, tx);
                }),
        )
}
