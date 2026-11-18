//! Shared OpenCode UI primitives (`widgets`), file/review panel chrome (`panel_chrome`),
//! popup/context menus (`popup`), toast notifications (`toast`), and shortcut keymap (`keymap`)
//! for Rune apps (`apps/desktop`, `apps/web`, `features/*`, and embedders such as Seer).

#![forbid(unsafe_code)]

pub mod keymap;
pub mod panel_chrome;
pub mod popup;
pub mod toast;
pub mod widgets;
