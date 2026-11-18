//! Shared OpenCode UI design-system foundation: palettes and themes (`palette`, `themes`),
//! typography and layout style tokens (`style`), dialog sizing (`dialog`), scroll containers
//! (`scroller`), rolling odometers (`odometer`), bottom sheets (`sheet`), popovers (`popover`),
//! provider marks (`provider_mark`), shortcut keymap (`keymap`), reusable controls (`widgets`),
//! file/review panel chrome (`panel_chrome`), popup/context menus (`popup`), and toast
//! notifications (`toast`) for Rune apps (`apps/desktop`, `apps/web`, `features/*`, and embedders
//! such as Seer).

#![forbid(unsafe_code)]

pub mod dialog;
pub mod keymap;
pub mod odometer;
pub mod palette;
pub mod panel_chrome;
pub mod popover;
pub mod popup;
pub mod provider_mark;
pub mod scroller;
pub mod sheet;
pub mod style;
pub mod themes;
pub mod toast;
pub mod widgets;
