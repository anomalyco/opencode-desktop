//! The page a window shows (Desktop's `LayoutRoute`, `shell/state/layout.tsx:72-78`): Home, a
//! draft tab's new-session page, or a session on one of the servers the app keeps live at once
//! (`/server/<base64 server>/session/<id>`).

use opencode_kit::servers::ServerKey;

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Route {
    #[default]
    Home,
    /// Session `id` on `server`. The window works against `server` while it shows it
    /// (`servers::runtime`'s checkout).
    Session { server: ServerKey, id: String },
    /// "New session": an empty page with the composer, shown by the draft tab with this id.
    Draft(String),
}

impl Route {
    /// Session `id` on the server the window is working against now (the checkout), the server
    /// every session id this turn reads or hears comes from.
    pub fn here(id: String) -> Route {
        Route::Session { server: crate::servers::runtime::current(), id }
    }

    pub fn session(server: ServerKey, id: impl Into<String>) -> Route {
        Route::Session { server, id: id.into() }
    }

    /// Whether it shows session `id` (a session route is always on the checked-out server).
    pub fn is_session(&self, id: &str) -> bool {
        matches!(self, Route::Session { id: open, .. } if open == id)
    }

    /// The server a session route is on.
    pub fn server(&self) -> Option<&ServerKey> {
        match self {
            Route::Session { server, .. } => Some(server),
            Route::Home | Route::Draft(_) => None,
        }
    }
}
