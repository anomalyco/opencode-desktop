//! Shared object-safe host capability traits implemented once by the host shell.
//!
//! Feature crates inherit from these traits (`pub trait MyHost: AppEnv + AppFeedback + AppOverlays`)
//! instead of re-declaring `clock`, `zoom`, `toast_*`, and `on_overlay_*` on every `*Host` trait.

/// Shared read-only environment & window context provided once by the host shell.
pub trait AppEnv {
    fn clock(&self) -> f64 {
        0.0
    }

    fn now_ms(&self) -> i64 {
        0
    }

    fn zoom(&self) -> f32 {
        1.0
    }

    fn spellcheck(&self) -> bool {
        false
    }

    fn reduced_motion(&self) -> bool {
        false
    }

    fn is_live(&self) -> bool {
        false
    }

    fn persists(&self) -> bool {
        false
    }

    fn is_headless(&self) -> bool {
        rune::is_headless()
    }

    fn waker(&self) -> Option<rune::Waker> {
        None
    }

    fn active_session_id(&self) -> Option<&str> {
        None
    }

    /// The session whose side panel strip the routed session shows. A child session (a
    /// subagent) a tab routes to shares its root session's strip: its tabs, its selection, its
    /// panes and whether it is open (official `packages/app/src/session/session-layout.ts`
    /// `panelKey`, opencode#65957). What a panel shows of a session (a review's diff, the
    /// context) still follows [`Self::active_session_id`].
    fn panel_session_id(&self) -> Option<&str> {
        self.active_session_id().map(|id| self.panel_root(id))
    }

    /// The session `sid`'s side panel strip is kept under: its root session while a tab holds
    /// that root, else `sid` itself (official `host-apis.tsx` `panelSessionID`).
    fn panel_root<'a>(&'a self, sid: &'a str) -> &'a str {
        sid
    }

    /// Whether session `sid`'s side panel is the one on the page: `sid` is the routed session or
    /// shares its strip (its root's page, or a sibling subagent's).
    fn panel_shows(&self, sid: &str) -> bool {
        self.panel_session_id().is_some_and(|shown| shown == self.panel_root(sid))
    }
}

/// Shared toast feedback capability provided once by the host shell.
pub trait AppFeedback {
    fn toast_say(&mut self, _title: String, _description: String) {}
    fn toast_succeed(&mut self, _title: String, _description: String) {}
    fn toast_fail(&mut self, _title: String, _description: String) {}
}

/// Shared modal/popover overlay stack notifications provided once by the host shell.
pub trait AppOverlays {
    fn on_overlay_opened(&mut self, _id: &'static str, _return_focus: Option<String>) {}
    fn on_overlay_closed(&mut self, _id: &'static str) {}
}
