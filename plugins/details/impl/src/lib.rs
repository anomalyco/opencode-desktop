//! Desktop's built-in Session details plugin (`packages/gui-plugins/src/details`) as a native
//! plugin: the `[grid]` button in the session header (its `session.header` slot, order 20) and
//! the popover it opens (mod+shift+y), with the server's Extensions: its MCP servers (each with
//! its status and switch), its plugins and its skills, each list in a flyout beside its row.
//!
//! A built-in: this rlib declares it, `plugins/details` exports it as a library, and
//! `opencode-desktop --features static-builtins` links it in.

mod services;

use opencode_plugin::abi::{ALIGN_END, SHADOW_FLOATING, SIDE_BOTTOM};
use opencode_plugin::prelude::*;

pub(crate) use services::Service;

/// The popover's id.
const POPOVER: &str = "main";
/// What the app's session header keeps at its end for its popovers to hang from (Desktop's
/// `Popover.Anchor`, 12 from the end, 10 down, 28 tall).
const ANCHOR: &str = "session-header-anchor";
/// The header button's key.
const TRIGGER: &str = "details-trigger";

pub struct Details {
    /// The list open beside the card.
    flyout: Option<Service>,
    /// Bumped on each open, so each open reads the lists afresh.
    opens: u64,
    /// MCP servers whose switch is on its way, by name.
    toggling: Vec<String>,
    /// A footer action is running.
    acting: bool,
}

impl WindowPlugin for Details {
    fn setup(cx: &mut WindowSetupCx<Self>) -> Result<Self, PluginError> {
        cx.add_slot(Contribution::slot(SlotName::SessionHeader, "details").order(20), header);
        cx.add_popover(Contribution::popover(POPOVER, "Session details", SIDE_BOTTOM, ALIGN_END, 8), popover);
        cx.add_command(
            CommandSpec::new("toggle", "Toggle session details").section(CommandSection::Session).bind("mod+shift+y").keywords("view details extensions mcp plugins skills"),
            |s, cx| s.toggle(cx),
        );
        Ok(Details { flyout: None, opens: 0, toggling: Vec::new(), acting: false })
    }
}

impl Details {
    fn toggle(&mut self, cx: &mut ActionCx) {
        if !cx.popover_open(POPOVER) {
            self.flyout = None;
            self.opens += 1;
        }
        cx.toggle_popover(POPOVER, ANCHOR);
    }
}

/// The header's button, for a session of a project.
fn header(_: &Details, cx: &mut ViewCx<Details>, props: &Props) -> El {
    let Some(session) = cx.session(&props.session_id).filter(|s| !s.project_id.is_empty()) else { return cx.nothing() };
    let open = cx.popover_open(POPOVER);
    cx.memo(&format!("trigger-{}", session.id), (open, cx.dark()), |cx| {
        let p = cx.pal();
        let button = el()
            .key(TRIGGER)
            .role("button")
            .label("Session details")
            .help("Session details  Ctrl+Shift+Y")
            .size(28.0, 28.0)
            .center()
            .radius(6.0)
            .hover(p.hover)
            .press(p.pressed)
            .instant_hover()
            .child(icon("window-analytics", 16.0).color(if open { p.icon } else { p.icon_muted }))
            .on_press(cx.handler(|s: &mut Details, cx| s.toggle(cx)));
        // The memo keys its root, so the button's own key sits a level down.
        row().child(if open { button.fill(p.pressed) } else { button })
    })
}

/// The popover: the Extensions card, and the list open beside it.
fn popover(s: &Details, cx: &mut ViewCx<Details>, props: &Props) -> El {
    if props.session_id.is_empty() {
        return cx.nothing();
    }
    let lists = services::Lists::read(cx, &props.directory, s.opens);
    let p = cx.pal();
    let heading = row()
        .w_full()
        .min_h(28.0)
        .pl(12.0)
        .pr(36.0)
        .py(4.0)
        .center_y()
        .child(text(if cx.servers() < 2 { "Extensions" } else { "Server" }).font(13.0, 440.0).line_height(16.0).color(p.faint).truncate().shrink());
    let rows = col().key("session-summary-rows").w_full().gap(2.0).children(Service::ALL.iter().map(|&service| services::trigger(s, cx, &lists, service)));
    let card = col()
        .key("session-summary-card")
        .role("dialog")
        .label("Session details")
        .w(280.0)
        .padding(2.0)
        .gap(2.0)
        .radius(6.0)
        .fill(p.layer1)
        .shadow(SHADOW_FLOATING)
        .child(heading)
        .child(rows);
    let flyout = s.flyout.map(|service| services::flyout(s, cx, &lists, service, &props.directory));
    col().key("session-summary-popover").child(card).children(flyout)
}

opencode_plugin::declare_plugin!(window = Details);
