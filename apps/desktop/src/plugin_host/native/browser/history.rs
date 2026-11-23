//! Native Luau bindings for `browser_pane::history::HistoryStore` and read-only user-explicit
//! browser history import (`UP-106` `#65073`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use browser_pane::cookies::BrowserEngineKind;
use browser_pane::cookies::icons::AppIcon;
use browser_pane::cookies::sources::source_history_database;
use browser_pane::history::{
    HistoryStore, ImportEngine, PageVisit, read_profile_history_in, sweep_stale_import_dirs,
};
use mlua::{Function, IntoLua, Lua, MultiValue, Table, Value};
use rune_luau::{Completer, Data, Host};

use super::{App, Dep, Vmx, cx, map, method, scope, with_app};

fn require_builtin_browser(lua: &Lua, what: &str) -> mlua::Result<()> {
    let cap = scope::TabScope::capture(lua);
    if cap.is_builtin_browser && cap.tool_sid.is_none() && cap.in_flight.is_empty() {
        return Ok(());
    }
    Err(mlua::Error::runtime(format!("{what} is restricted to the built-in browser.")))
}

fn import_scratch_dir() -> Option<PathBuf> {
    crate::dirs::data().map(|d| d.join("browser").join("import-tmp"))
}

fn staged_imports() -> &'static Mutex<HashMap<u64, Vec<PageVisit>>> {
    static STAGED: OnceLock<Mutex<HashMap<u64, Vec<PageVisit>>>> = OnceLock::new();
    STAGED.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_import_token() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn with_store<R>(app: &App, f: impl FnOnce(&mut HistoryStore) -> R) -> Option<R> {
    let mut cell = app.workspace_panes.browser.history.borrow_mut();
    if cell.is_none() {
        let can_persist = crate::attended() && opencode_storage::holds_profile() == Some(true);
        let store = if can_persist {
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

fn is_recordable_tab(app: &App, tab_id: Option<&str>) -> bool {
    let Some(id) = tab_id.and_then(|s| s.parse::<u64>().ok()) else {
        return false;
    };
    app.workspace_panes.browser.host.tab(id).is_some_and(|tab| {
        !tab.context.is_isolated() && tab.preview.is_none() && !tab.prompts.agent_driven
    })
}

pub(super) fn service_methods(lua: &Lua, vmx: &Vmx, t: &Table) -> mlua::Result<()> {
    method(lua, t, "HistoryPrepare", |lua, ()| {
        require_builtin_browser(lua, "Browser:HistoryPrepare")?;
        if let Some(dir) = import_scratch_dir() {
            sweep_stale_import_dirs(&dir);
        }
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
            // Only record top-level HTTP(S) visits from a live default-context non-agent,
            // non-preview tab (never isolated/private/remote/preview/agent navigations).
            if browser_pane::policy::destination_origin(&url).is_none() || !is_recordable_tab(app, tab_id.as_deref()) {
                return Ok(());
            }
            with_store(app, |store| store.visit(&url, &title, typed.unwrap_or(false)));
            Ok(())
        })
    })?;
    method(lua, t, "HistoryRetitle", |lua, (url, title, tab_id): (String, String, Option<String>)| {
        require_builtin_browser(lua, "Browser:HistoryRetitle")?;
        with_app("Browser:HistoryRetitle", |app| {
            if browser_pane::policy::destination_origin(&url).is_none() || !is_recordable_tab(app, tab_id.as_deref()) {
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
        // Listed off the UI thread; the page draws again when a listing lands.
        cx::read(Dep::Browser);
        let sources = with_app("Browser:HistoryImportSources", |app| Ok(app.workspace_panes.browser.app_icons.sources.get((*app.waker).clone())))?;
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
                    ("engine", Data::String(engine_name(b.engine).into())),
                    ("profiles", Data::List(profiles)),
                ])
            })
            .collect();
        Data::List(list).into_lua(lua)
    })?;
    // The browser's own icon (`settings.tsx:108-111` `Identity image`): `("rgba", pixels, w, h)`,
    // `("png", bytes)`, `("none")` when the browser has no application this knows or the OS no icon,
    // or nothing while it is still being read off the UI thread (`surfaces::browser::app_icons`).
    method(lua, t, "HistoryImportIcon", |lua, source_id: String| {
        require_builtin_browser(lua, "Browser:HistoryImportIcon")?;
        cx::read(Dep::Browser);
        let known = with_app("Browser:HistoryImportIcon", |app| {
            Ok(app.workspace_panes.browser.app_icons.get(&source_id, (*app.waker).clone()))
        })?;
        let Some(known) = known else {
            return Ok(MultiValue::new());
        };
        let Some(icon) = known else {
            return Ok(MultiValue::from_vec(vec![Value::String(lua.create_string("none")?)]));
        };
        let values = match &*icon {
            AppIcon::Rgba { width, height, pixels } => vec![
                Value::String(lua.create_string("rgba")?),
                Value::Buffer(lua.create_buffer(pixels.as_slice())?),
                Value::Number(f64::from(*width)),
                Value::Number(f64::from(*height)),
            ],
            AppIcon::Png(bytes) => vec![Value::String(lua.create_string("png")?), Value::Buffer(lua.create_buffer(bytes.as_slice())?)],
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
        let scratch = import_scratch_dir();
        let pages = match read_profile_history_in(engine, Path::new(&db_path), scratch.as_deref()) {
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
    method(lua, t, "_HistoryApplyImport", |lua, token: u64| {
        require_builtin_browser(lua, "Browser:HistoryImportProfileAsync")?;
        let Some(pages) = staged_imports().lock().unwrap_or_else(|e| e.into_inner()).remove(&token) else {
            return map([("error", Data::String("Staged history import expired".into()))]).into_lua(lua);
        };
        let res = with_app("Browser:HistoryImportProfileAsync", |app| {
            Ok(with_store(app, |store| store.add(&pages)).unwrap_or_else(|| Err("History store unavailable".into())))
        })?;
        match res {
            Ok(count) => map([("imported", Data::Number(count as f64))]).into_lua(lua),
            Err(err) => map([("error", Data::String(err))]).into_lua(lua),
        }
    })?;
    let raw_import: Function = vmx.asyncs.raw_get("Browser.HistoryImportProfileAsync")?;
    let import_async: Function = opencode_plugin_runtime::chunk::load(lua, "local raw = ... return function(self, b, p) local r = raw(b, p) if type(r) == 'table' and type(r.token) == 'number' then return self:_HistoryApplyImport(r.token) end return r end")
        .set_name("=Browser:HistoryImportProfileAsync")
        .call(raw_import)?;
    t.raw_set("HistoryImportProfileAsync", import_async)?;
    Ok(())
}

pub(super) fn asyncs(host: &Host<'_>, t: &Table) -> mlua::Result<()> {
    t.raw_set(
        "Browser.HistoryImportProfileAsync",
        host.async_function("Browser:HistoryImportProfileAsync", import_profile_async)?,
    )
}

fn import_profile_async(lua: &Lua, a: MultiValue, done: Completer) -> mlua::Result<()> {
    require_builtin_browser(lua, "Browser:HistoryImportProfileAsync")?;
    let (browser_id, profile_id): (String, String) = mlua::FromLuaMulti::from_lua_multi(a, lua)?;
    let scratch = import_scratch_dir();
    let done_slot = std::sync::Arc::new(Mutex::new(Some(done)));
    let thread_done = done_slot.clone();
    let finish = |slot: &Mutex<Option<Completer>>, data: Data| {
        if let Some(c) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
            c.complete(data);
        }
    };
    let spawned = std::thread::Builder::new().name("opencode-hist-import".into()).spawn(move || {
        let Some((engine, db_path)) = source_history_database(&browser_id, &profile_id) else {
            return finish(&thread_done, map([("error", Data::String("Browser profile not found".into()))]));
        };
        let engine = match engine {
            BrowserEngineKind::Chromium => ImportEngine::Chromium,
            BrowserEngineKind::Firefox => ImportEngine::Firefox,
            BrowserEngineKind::Safari => ImportEngine::Safari,
        };
        match read_profile_history_in(engine, Path::new(&db_path), scratch.as_deref()) {
            Ok(pages) => {
                let token = next_import_token();
                let mut map_guard = staged_imports().lock().unwrap_or_else(|e| e.into_inner());
                if map_guard.len() >= 8 {
                    map_guard.clear();
                }
                map_guard.insert(token, pages);
                drop(map_guard);
                finish(&thread_done, map([("token", Data::Integer(token as i64))]));
            }
            Err(err) => finish(&thread_done, map([("error", Data::String(err))])),
        }
    });
    if spawned.is_err() {
        finish(&done_slot, map([("error", Data::String("Could not start history import thread".into()))]));
    }
    Ok(())
}

fn engine_name(engine: BrowserEngineKind) -> &'static str {
    match engine {
        BrowserEngineKind::Chromium => "chromium",
        BrowserEngineKind::Firefox => "firefox",
        BrowserEngineKind::Safari => "safari",
    }
}
