//! Native Luau bindings for `browser_pane::history::HistoryStore` and read-only user-explicit
//! browser history import (`UP-106` `#65073`).

use std::path::Path;

use browser_pane::cookies::BrowserEngineKind;
use browser_pane::cookies::icons::{AppIcon, application, application_icon};
use browser_pane::cookies::sources::{list_sources, source_history_database};
use browser_pane::history::{HistoryStore, ImportEngine, read_profile_history};
use mlua::{IntoLua, Lua, MultiValue, Table, Value};
use rune_luau::Data;

use super::{App, map, method, scope, with_app};

fn require_builtin_browser(lua: &Lua, what: &str) -> mlua::Result<()> {
    let cap = scope::TabScope::capture(lua);
    if cap.is_builtin_browser && cap.tool_sid.is_none() {
        return Ok(());
    }
    Err(mlua::Error::runtime(format!("{what} is restricted to the built-in browser.")))
}

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
    method(lua, t, "HistoryPrepare", |lua, ()| {
        require_builtin_browser(lua, "Browser:HistoryPrepare")?;
        with_app("Browser:HistoryPrepare", |app| {
            with_store(app, |store| store.prepare());
            Ok(())
        })
    })?;
    method(lua, t, "HistorySearch", |lua, (query, limit): (String, Option<usize>)| {
        require_builtin_browser(lua, "Browser:HistorySearch")?;
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
    method(lua, t, "HistoryVisit", |lua, (url, title, typed, tab_id): (String, String, Option<bool>, Option<String>)| {
        require_builtin_browser(lua, "Browser:HistoryVisit")?;
        with_app("Browser:HistoryVisit", |app| {
            // Never record history from isolated/preview tabs or non-HTTP(S) URLs.
            if browser_pane::policy::destination_origin(&url).is_none() {
                return Ok(());
            }
            if let Some(id) = tab_id.as_deref().and_then(|s| s.parse::<u64>().ok())
                && let Some(tab) = app.workspace_panes.browser.host.tab(id)
                && (tab.context.is_isolated() || tab.preview.is_some())
            {
                return Ok(());
            }
            with_store(app, |store| store.visit(&url, &title, typed.unwrap_or(false)));
            Ok(())
        })
    })?;
    method(lua, t, "HistoryRetitle", |lua, (url, title): (String, String)| {
        require_builtin_browser(lua, "Browser:HistoryRetitle")?;
        with_app("Browser:HistoryRetitle", |app| {
            if browser_pane::policy::destination_origin(&url).is_none() {
                return Ok(());
            }
            with_store(app, |store| store.retitle(&url, &title));
            Ok(())
        })
    })?;
    method(lua, t, "HistoryAdopt", |lua, visits: Value| {
        require_builtin_browser(lua, "Browser:HistoryAdopt")?;
        let mut pairs: Vec<(String, String)> = Vec::new();
        if let Value::Table(tbl) = visits {
            for item in tbl.sequence_values::<Table>().flatten() {
                let url: String = item.raw_get("url").unwrap_or_default();
                let title: String = item.raw_get("title").unwrap_or_default();
                if browser_pane::policy::destination_origin(&url).is_some() {
                    pairs.push((url, title));
                }
            }
        }
        with_app("Browser:HistoryAdopt", |app| {
            with_store(app, |store| store.adopt(&pairs));
            Ok(())
        })
    })?;
    method(lua, t, "HistoryForget", |lua, url: String| {
        require_builtin_browser(lua, "Browser:HistoryForget")?;
        with_app("Browser:HistoryForget", |app| {
            Ok(with_store(app, |store| store.remove(&url)).unwrap_or(false))
        })
    })?;
    method(lua, t, "HistoryClear", |lua, ()| {
        require_builtin_browser(lua, "Browser:HistoryClear")?;
        with_app("Browser:HistoryClear", |app| {
            Ok(with_store(app, |store| store.clear().is_ok()).unwrap_or(false))
        })
    })?;
    method(lua, t, "HistoryCount", |lua, ()| {
        require_builtin_browser(lua, "Browser:HistoryCount")?;
        with_app("Browser:HistoryCount", |app| {
            Ok(with_store(app, |store| store.count() as i64).unwrap_or(0))
        })
    })?;
    // Official's list of browsers and their profiles (`profile/sources.ts:60-143,349-423`): a
    // profile counts when it has a cookie database, named from `Local State` or `profiles.ini`.
    method(lua, t, "HistoryImportSources", |lua, ()| {
        require_builtin_browser(lua, "Browser:HistoryImportSources")?;
        let list: Vec<Data> = list_sources()
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
                    ("engine", Data::String(engine_name(b.engine).into())),
                    ("profiles", Data::List(profiles)),
                ])
            })
            .collect();
        Data::List(list).into_lua(lua)
    })?;
    // The browser's own icon (`settings.tsx:108-111` `Identity image`): `("rgba", pixels, w, h)`,
    // `("png", bytes)`, or nothing, when the browser has no application this knows or the OS no icon.
    method(lua, t, "HistoryImportIcon", |lua, source_id: String| {
        require_builtin_browser(lua, "Browser:HistoryImportIcon")?;
        let Some(icon) = application(&source_id).and_then(|path| application_icon(&path)) else {
            return Ok(MultiValue::new());
        };
        let values = match icon {
            AppIcon::Rgba { width, height, pixels } => vec![
                Value::String(lua.create_string("rgba")?),
                Value::Buffer(lua.create_buffer(pixels)?),
                Value::Number(f64::from(width)),
                Value::Number(f64::from(height)),
            ],
            AppIcon::Png(bytes) => vec![Value::String(lua.create_string("png")?), Value::Buffer(lua.create_buffer(bytes)?)],
        };
        Ok(MultiValue::from_vec(values))
    })?;
    method(lua, t, "HistoryImportProfile", |lua, (browser_id, profile_id): (String, String)| {
        require_builtin_browser(lua, "Browser:HistoryImportProfile")?;
        // Resolved only through `list_sources()` ids, so a plugin never names a filesystem path.
        let Some((engine, db_path)) = source_history_database(&browser_id, &profile_id) else {
            return map([("error", Data::String("Browser profile not found".into()))]).into_lua(lua);
        };
        let engine = match engine {
            BrowserEngineKind::Chromium => ImportEngine::Chromium,
            BrowserEngineKind::Firefox => ImportEngine::Firefox,
            BrowserEngineKind::Safari => ImportEngine::Safari,
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

fn engine_name(engine: BrowserEngineKind) -> &'static str {
    match engine {
        BrowserEngineKind::Chromium => "chromium",
        BrowserEngineKind::Firefox => "firefox",
        BrowserEngineKind::Safari => "safari",
    }
}
