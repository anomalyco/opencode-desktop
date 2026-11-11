//! The window's file, read once as the window opens: in Desktop's mixed-server shape, migrated
//! from the per-server one the first time (`opencode_kit::servers::migration::window`), with
//! the new-session page's old words turned into a draft tab.

use opencode_kit::client::{DEFAULT_SERVER, Server};
use opencode_kit::servers::key::draft_href;
use opencode_kit::servers::migration::Context;
use opencode_kit::servers::tabs::{DraftTab, Tab, WindowDoc};
use opencode_kit::servers::{ServerKey, TabKey};
use serde_json::Value;

use crate::App;

/// Window `app`'s saved tabs, page, drafts, panes and closed tabs. `server` is the app's own
/// server: its addresses are `"sidecar"` in a file from before.
pub fn read_window(app: &App, server: &Server) -> WindowDoc {
    let ctx = Context {
        sidecar: vec![server.base.clone(), DEFAULT_SERVER.to_owned()],
        aliases: Vec::new(),
        saved: crate::servers::list().into_iter().map(|s| s.url).collect(),
        active: Some(server.base.clone()),
    };
    let (v, migrated) = crate::windows::store::load(&app.window, &ctx);
    let mut doc = WindowDoc::from_json(&v);
    if migrated {
        legacy_new_drafts(&mut doc, &app.default_dir);
        // Once: the file is in the new shape from now on, what the old one held under `legacy`.
        if crate::attended() {
            crate::windows::store::save_all(&app.window, doc.to_json());
        }
    }
    doc
}

/// The words the new-session page kept under `new` before drafts were tabs, which the migration
/// keeps under `legacy`, come back in a draft tab on their server; the page it was shows it.
fn legacy_new_drafts(doc: &mut WindowDoc, dir: &str) {
    let words = |d: &Value| d.as_str().or(d["text"].as_str()).is_some_and(|t| !t.trim().is_empty());
    let found: Vec<(ServerKey, Option<Value>, bool)> = doc
        .legacy
        .values()
        .filter_map(|r| {
            let server = ServerKey::new(r["server"].as_str()?);
            let text = r["drafts"].get("new").filter(|d| words(d)).cloned();
            let page = r["active"] == true && r["route"] == "new";
            (text.is_some() || page).then_some((server, text, page))
        })
        .collect();
    for (server, text, page) in found {
        let id = crate::tabs::draft_id();
        doc.tabs.get_or_insert_with(Vec::new).push(Tab::Draft(DraftTab::new(&id, server, dir)));
        if let Some(text) = text {
            doc.drafts.push((TabKey::draft(&id), text));
        }
        if page {
            doc.last_active_url = Some(draft_href(&id));
            doc.recent = Some(TabKey::draft(&id));
        }
    }
}
