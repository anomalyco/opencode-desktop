//! Shared muted-tab indicator helpers for sidebar tabs and pinned items.

use rune_kit::shell::TabItemStatus;

use super::*;
use opencode_i18n::t;

pub(crate) fn muted_mark(p: &Pal) -> El {
    // `MutedIcon`, tab-nav.tsx.
    static BELL: Icon = Icon {
        name: "sidebar-muted",
        view: 16.0,
        body: r#"<path d="M11.89 4.13C11.24 2.61 9.74 1.56 8 1.56C5.67 1.56 3.78 3.45 3.78 5.78V9.56C3.78 10.54 2.98 11.33 2 11.33H4.67M14.22 1.78L1.78 14.22M8.44 11.33H14C13.02 11.33 12.22 10.54 12.22 9.56V7.55M9.33 13.68C9.07 14.14 8.57 14.44 8 14.44C7.43 14.44 6.93 14.14 6.67 13.68" stroke="currentColor" fill="none"/>"#,
    };
    icon(&BELL, 14.0).key("muted").color(p.icon_faint).help(t!("sidebar.recent.muted"))
}

/// Accessible description for a muted `role("tab")`: preserves both `"Muted"` and any underlying
/// status (`Needs attention`, `Unread`, `Failed`, `Working`) that the muted bell replaced visually.
pub(crate) fn muted_description(mark: Option<TabItemStatus>) -> String {
    let words = super::super::tab_strip::tab_words();
    let status = match mark {
        Some(TabItemStatus::Busy) => Some(words.busy),
        Some(TabItemStatus::Attention) => Some(words.attention),
        Some(TabItemStatus::Unread) => Some(words.unread),
        Some(TabItemStatus::Failed) => Some(words.failed),
        None => None,
    };
    match status {
        Some(state) => format!("{state}, {}", t!("sidebar.recent.muted")),
        None => t!("sidebar.recent.muted").to_owned(),
    }
}

/// The kit's decorated element wraps the entire tab. Reserve a column beside the
/// pill so its title and close button keep their own space.
pub(crate) fn marked_slot(slot: El, mark: El, status: Option<TabItemStatus>) -> El {
    slot.description(muted_description(status))
        .pr(22.0)
        .child(el().pin(None, Some(0.0), Some(4.0), Some(0.0)).w(16.0).center().child(mark))
}
