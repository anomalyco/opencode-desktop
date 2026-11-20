//! The navigation prototype (Settings › Experimental › Organize vertical sidebar, Consolidated;
//! local and dev builds only; `titlebar.tsx`, `pinned-area.tsx`): Home becomes Sessions, Search
//! leaves the sidebar and the titlebar for Home's own field, and the sidebar lists the pinned
//! sessions above the side tabs (Tabs) or Recent (List).

use super::*;
use opencode_i18n::t;

/// `sessions-stack` (`packages/ui/src/icons/catalog.ts`): Home's glyph under the prototype.
pub(crate) static SESSIONS_STACK: Icon = Icon {
    name: "sessions-stack",
    view: 16.0,
    body: r#"<path d="M3.5 3.5H12.5M1.5 5.5V12.5H14.5V5.5H12.5H3.5H1.5Z" stroke-linecap="round" stroke-linejoin="round" stroke="currentColor"/>"#,
};

/// Home's label and glyph (`homeLabel`, `homeIcon`): Sessions under the prototype.
pub(crate) fn home_label(s: &App) -> (&'static str, &'static Icon) {
    if s.settings.navigation_prototype() { (t!("sidebar.sessions"), &SESSIONS_STACK) } else { (t!("home.title"), &ic::GRID_PLUS) }
}

/// The side tabs under the prototype's Tabs (`pinnedVerticalStrip`): the pinned sessions lead,
/// and their open tabs keep their place in the tab order without showing again below.
pub(crate) fn tabs(s: &App, p: &Pal) -> El {
    let (pins, rest) = pins::section(s, p);
    let Some(pins) = pins else { return crate::view::tab_strip::tab_strip(s, p, "tabs", TabOrientation::Vertical, None, None) };
    let mut content = col().key("consolidated-content").w_full().gap(12.0).child(pins);
    if !rest.is_empty() {
        let tabs_h = rune::memo::layout_with("tabs-list", |r| r[3]).unwrap_or(rest.len() as f32 * 30.0);
        content = content.child(col().key("consolidated-tabs").w_full().h(tabs_h).child(crate::view::tab_strip::section_strip(s, p, "tabs", &rest, false)));
    }
    let viewport = col().grow(1.0).shrink().min_h(0.0);
    let scroll = session_view::scroller::scroller("consolidated-scroll").build::<App>(p, viewport, content);
    col().key("consolidated").w_full().grow(1.0).shrink().min_h(0.0).child(scroll)
}
