//! Single source of truth for desktop overlay stacking layers, key-dispatch bands, and
//! page-Escape dismissal bands.
//!
//! Mirrors [`crate::tick::stage`]: `kernel::build()` registers [`layer::ALL`], [`key_band::ALL`],
//! and [`escape_band::ALL`] once at start-up, and `OverlayDispatcher::resolve()` validates every
//! overlay and Escape registration against these declarations (failing loudly on undeclared
//! bands, duplicate IDs, missing edges, backward edges, or cycles).

/// Overlay key-dispatch bands in execution order (first band sees keys first).
///
/// Interleaved with non-overlay handlers in [`crate::keys::on_key`]:
/// `POPUP` -> `MODAL` -> `glitch` / `demo` -> `MID` -> `dictation` / `knobs` -> `LOW`
/// -> `panels` / `question` / `add_menu` / `draft_location` / `shortcuts` / `pickers` / `global` / `escape`.
pub(crate) mod key_band {
    use opencode_kernel::KeyBand;

    /// Drawn popup/context menus and narrow drawers (runs first in `keys::on_key`).
    pub const POPUP: KeyBand = KeyBand::new("keys.popup");
    /// Plugin popovers/dialogs and top-tier modal dialogs (runs before `glitch` and `demo`).
    pub const MODAL: KeyBand = KeyBand::new("keys.modal");
    /// Mid-tier lightboxes and timeline/session dialogs (runs after `glitch`/`demo`, before `dictation`/`knobs`).
    pub const MID: KeyBand = KeyBand::new("keys.mid");
    /// Command/project switchers and the full-page Settings screen (runs after `dictation`/`knobs`, before `panels`).
    pub const LOW: KeyBand = KeyBand::new("keys.low");

    /// Every desktop overlay key-dispatch band in exact execution order.
    pub const ALL: &[KeyBand] = &[POPUP, MODAL, MID, LOW];
}

/// Visual overlay stacking layers in bottom-to-top drawing order (last layer draws on top).
///
/// Each layer names its paired `default_key_band` so standard overlays get matching visual and key
/// precedence automatically. An overlay whose key band differs from its visual tier must opt in
/// explicitly via `OverlayRegistration::with_key_in(...)` with a comment explaining why.
pub(crate) mod layer {
    use opencode_kernel::OverlayLayer;

    use super::key_band;

    /// Narrow bottom sheets, composer pickers/popovers/menus, activity/todo panels, and home/worktree popovers.
    pub const SURFACE_POPOVER: OverlayLayer = OverlayLayer::new("layer.surface_popover", key_band::LOW);
    /// Command/file/session and project/worktree switchers.
    pub const SWITCHER: OverlayLayer = OverlayLayer::new("layer.switcher", key_band::LOW);
    /// Fullscreen image lightbox, Mermaid diagram lightbox, and tool execution details dialog.
    pub const MEDIA_LIGHTBOX: OverlayLayer = OverlayLayer::new("layer.media_lightbox", key_band::MID);
    /// Session delete/rename, terminal paste guard, and worktree delete confirmation dialogs.
    pub const ACTION_DIALOG: OverlayLayer = OverlayLayer::new("layer.action_dialog", key_band::MID);
    /// Floating quote editor/toolbar, line-comment toolbar, and side-panel context menus & drag ghost.
    pub const FLOATING_CHROME: OverlayLayer = OverlayLayer::new("layer.floating_chrome", key_band::MID);
    /// Standard modal dialogs (default layer for feature dialogs: fork, jump, connect, mcp, and `layer = "standard"` plugin dialogs).
    pub const DIALOG: OverlayLayer = OverlayLayer::new("layer.dialog", key_band::MODAL);
    /// Elevated confirmation, system, and plugin dialogs drawn above standard dialogs.
    pub const ELEVATED_DIALOG: OverlayLayer = OverlayLayer::new("layer.elevated_dialog", key_band::MODAL);
    /// Tab hover preview card and drawn context/dropdown popup menus.
    pub const CHROME_POPUP: OverlayLayer = OverlayLayer::new("layer.chrome_popup", key_band::POPUP);
    /// Top-most plugin popover/flyout and third-party plugin trust prompt.
    pub const PLUGIN_TOP: OverlayLayer = OverlayLayer::new("layer.plugin_top", key_band::MODAL);
    /// Status and upload toast notification stack (top-most layer inside the zoomed shell).
    pub const TOAST: OverlayLayer = OverlayLayer::new("layer.toast", key_band::POPUP);

    /// Every visual overlay layer inside the zoomed shell in exact bottom-to-top drawing order.
    pub const ALL: &[OverlayLayer] = &[
        SURFACE_POPOVER,
        SWITCHER,
        MEDIA_LIGHTBOX,
        ACTION_DIALOG,
        FLOATING_CHROME,
        DIALOG,
        ELEVATED_DIALOG,
        CHROME_POPUP,
        PLUGIN_TOP,
        TOAST,
    ];
}

/// Dismissible page-level Escape bands in dismissal order (first band dismisses first, after
/// `OverlayStack`).
pub(crate) mod escape_band {
    use opencode_kernel::EscapeBand;

    /// Transient shell chrome and selection toolbars (sidebar peek, comment selection, quote offer).
    pub const CHROME: EscapeBand = EscapeBand::new("escape.chrome");
    /// In-flight session actions (cancelling a pending session open, stopping an active run).
    pub const SESSION_ACTION: EscapeBand = EscapeBand::new("escape.session_action");
    /// Page-scoped popovers and inline bars (home projects menu, activity panel, todos, session find).
    pub const PAGE_POPOVER: EscapeBand = EscapeBand::new("escape.page_popover");
    /// Fallback layers (persistent failure toast, expanded tool step, returning from a child subagent).
    pub const FALLBACK: EscapeBand = EscapeBand::new("escape.fallback");

    /// Every desktop page-Escape band in exact dismissal order.
    pub const ALL: &[EscapeBand] = &[CHROME, SESSION_ACTION, PAGE_POPOVER, FALLBACK];
}
