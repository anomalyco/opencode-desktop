//! Auto-accept permissions, as in Desktop (`session/requests/auto-approve.ts`, the `permissions.autoaccept`
//! command on ⇧⌘A, Settings › General). One client-local switch covers every session and project: while
//! it's on, each permission request is answered "once" the moment it's known, pending ones included,
//! and the permission card stays hidden. Forms and questions still wait for a person.

use opencode_kit::model::Pending;
use rune::{El, Tx, row};

use crate::palette::Pal;
use rune::icon;
use rune_theme_opencode::icons as ic;

use crate::App;

impl App {
    pub fn set_auto_accept(&mut self, on: bool) {
        self.change_settings(|s| s.auto_accept = on);
        if on {
            let open: Vec<String> =
                self.pending.iter().filter(|p| matches!(p, Pending::Permission { .. })).map(|p| p.id().to_owned()).collect();
            for id in open {
                self.reply_permission(&id, "once");
            }
        }
    }

    pub fn toggle_auto_accept(&mut self, tx: &mut Tx) {
        tx.instant();
        self.set_auto_accept(!self.settings.auto_accept);
    }

    /// A request that just arrived: answered at once when auto-accept is on.
    pub(crate) fn auto_answer(&mut self, id: &str) {
        if self.settings.auto_accept && self.pending.iter().any(|p| p.id() == id && matches!(p, Pending::Permission { .. })) {
            self.reply_permission(id, "once");
        }
    }
}

/// The composer toolbar's mark that auto-accept is on; pressing it turns it off.
pub fn indicator(s: &App, p: &Pal) -> Option<El> {
    s.settings.auto_accept.then(|| {
        row()
            .key("auto-accept")
            .role("button")
            .label("Stop auto-accepting permissions")
            .size(28.0, 28.0)
            .center()
            .radius(6.0)
            .hover(p.hover)
            .press(p.pressed)
            .on_press_tx(|s: &mut App, tx: &mut Tx| s.toggle_auto_accept(tx))
            .child(icon(&ic::CHECK_SMALL, 16.0).color(p.accent))
    })
}

#[cfg(test)]
mod tests;
