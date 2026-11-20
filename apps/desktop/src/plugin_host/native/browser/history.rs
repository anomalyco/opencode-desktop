//! Native Luau bindings for `browser_pane::history::HistoryStore` and read-only user-explicit
//! browser history import (`UP-106` `#65073`).

use std::path::Path;

use browser_pane::history::{HistoryStore, discover_sources, read_profile_history};
use mlua::{IntoLua, Lua, Table, Value};
use rune_luau::Data;

use super::{App, map, method, with_app};

fn with_store<R>(app: &App, f: impl FnOnce(&mut HistoryStore) -> R) -> Option<R> {
    let mut cell = app.workspace_panes.browser.history.borrow_mut();
    if cell.is_none() {
        let store = if crate::attended() {
            crate::dirs::data()
                .map(|d| d.join("browser"))
                .and_then(|dir| HistoryStore::open(&dir).ok())
                .or_else(|| HistoryStore::open_in_memory().ok())
        } else {
            HistoryStore::open_in_memory().ok()
        };
        *cell = store;
    }
    cell.as_mut().map(f)
}

pub(super) fn service_methods(lua: &Lua, t: &Table) -> mlua::Result<()> {
    method(lua, t, "HistoryPrepare", |_, ()| {
        with_app("Browser:HistoryPrepare", |app| {
            with_store(app, |store| store.prepare());
            Ok(())
        })
    })?;
    method(lua, t, "HistorySearch", |lua, (query, limit): (String, Option<usize>)| {
        let cap = limit.unwrap_or(8).min(32);
        let found = with_app("Browser:HistorySearch", |app| {
            Ok(with_store(app, |store| store.search(&query, cap)).unwrap_or_default())
        })?;
        let list: Vec<Data> = found
            .into_iter()
            .map(|p| {
                map([
                    ("url", Data::String(p.url)),
                    ("title", Data::String(p.title)),
                    ("score", Data::Number(p.score)),
                ])
            })
            .collect();
        Data::List(list).into_lua(lua)
    })?;
    method(lua, t, "HistoryVisit", |_, (url, title, typed, tab_id): (String, String, Option<bool>, Option<String>)| {
        with_app("Browser:HistoryVisit", |app| {
            // Never record history from isolated/private preview tabs.
            if let Some(id) = tab_id.as_deref().and_then(|s| s.parse::<u64>().ok())
                && let Some(tab) = app.workspace_panes.browser.host.tab(id)
                && matches!(tab.context, browser_pane::protocol::ContextKind::IsolatedPreview { .. })
            {
                return Ok(());
            }
            with_store(app, |store| store.visit(&url, &title, typed.unwrap_or(false)));
            Ok(())
        })
    })?;
    method(lua, t, "HistoryRetitle", |_, (url, title): (String, String)| {
        with_app("Browser:HistoryRetitle", |app| {
            with_store(app, |store| store.retitle(&url, &title));
            Ok(())
        })
    })?;
    method(lua, t, "HistoryAdopt", |_, visits: Value| {
        let mut pairs: Vec<(String, String)> = Vec::new();
        if let Value::Table(tbl) = visits {
            for item in tbl.sequence_values::<Table>().flatten() {
                let url: String = item.raw_get("url").unwrap_or_default();
                let title: String = item.raw_get("title").unwrap_or_default();
                if !url.is_empty() {
                    pairs.push((url, title));
                }
            }
        }
        with_app("Browser:HistoryAdopt", |app| {
            with_store(app, |store| store.adopt(&pairs));
            Ok(())
        })
    })?;
    method(lua, t, "HistoryForget", |_, url: String| {
        with_app("Browser:HistoryForget", |app| {
            Ok(with_store(app, |store| store.remove(&url)).unwrap_or(false))
        })
    })?;
    method(lua, t, "HistoryClear", |_, ()| {
        with_app("Browser:HistoryClear", |app| {
            Ok(with_store(app, |store| store.clear().is_ok()).unwrap_or(false))
        })
    })?;
    method(lua, t, "HistoryCount", |_, ()| {
        with_app("Browser:HistoryCount", |app| {
            Ok(with_store(app, |store| store.count() as i64).unwrap_or(0))
        })
    })?;
    method(lua, t, "HistoryImportSources", |lua, ()| {
        let sources = discover_sources();
        let list: Vec<Data> = sources
            .into_iter()
            .map(|b| {
                let profiles: Vec<Data> = b
                    .profiles
                    .into_iter()
                    .map(|p| map([("id", Data::String(p.id)), ("name", Data::String(p.name))]))
                    .collect();
                map([
                    ("id", Data::String(b.id)),
                    ("name", Data::String(b.name)),
                    ("engine", Data::String(b.engine.as_str().into())),
                    ("profiles", Data::List(profiles)),
                ])
            })
            .collect();
        Data::List(list).into_lua(lua)
    })?;
    method(lua, t, "HistoryImportProfile", |lua, (browser_id, profile_id): (String, String)| {
        // Resolve browser/profile exclusively through `discover_sources()` so plugins cannot
        // supply arbitrary filesystem paths.
        let sources = discover_sources();
        let target = sources
            .into_iter()
            .find(|b| b.id == browser_id)
            .and_then(|b| {
                let engine = b.engine;
                b.profiles.into_iter().find(|p| p.id == profile_id).map(|p| (engine, p.db_path))
            });
        let Some((engine, db_path)) = target else {
            return map([("error", Data::String("Browser profile not found".into()))]).into_lua(lua);
        };
        let pages = match read_profile_history(engine, Path::new(&db_path)) {
            Ok(p) => p,
            Err(err) => return map([("error", Data::String(err))]).into_lua(lua),
        };
        let res = with_app("Browser:HistoryImportProfile", |app| {
            Ok(with_store(app, |store| store.add(&pages)).unwrap_or_else(|| Err("History store unavailable".into())))
        })?;
        match res {
            Ok(count) => map([("imported", Data::Number(count as f64))]).into_lua(lua),
            Err(err) => map([("error", Data::String(err))]).into_lua(lua),
        }
    })?;
    Ok(())
}
