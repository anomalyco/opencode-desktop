//! Native Luau bindings for MV3 browser extension management in `Settings › Browser › Extensions`.
//!
//! Security boundary (`notes/review-rd274.md` & `notes/review-rd280.md`):
//! - EVERY sync and async method is strictly restricted to user actions in the bundled built-in
//!   `browser` plugin (`scope.is_builtin_browser && scope.tool_sid.is_none() && scope.in_flight.is_empty()`).
//!   Coding-agent MCP tools and third-party plugins are unconditionally refused.
//! - Folder installs open a native OS directory picker via `Tx::open_folder`; Luau never passes
//!   arbitrary filesystem paths.
//! - User-facing errors return stable catalog reason codes (`"manifest" | "download" | "load" |
//!   "collision" | "expired" | "downgrade"`), never raw Rust strings.

use std::sync::{Arc, Mutex, PoisonError};

use browser_extensions::manager::{ExtensionManager, StagedInstall};
use browser_extensions::review::{InstallEffect, InstallEvent};
use browser_extensions::sources::{list_browser_extensions, list_chromium_sources};
use browser_extensions::{ExtensionSource, InstallFailure, ProfileExtension, StoredExtension};
use mlua::{Function, Lua, MultiValue, Table, Value};
use rune::{FileDialogOptions, Tx};
use rune_luau::{Completer, Data, Host};

use super::{App, Dep, Vmx, cx, json_to_data, map, method, scope, with_app};
use crate::surfaces::browser::extensions::{
    BROWSER_EXTENSIONS, CHROME_STORE_VERSION, ExtInstallCore, failure_code_str, manager,
    platform_dirs, run_cleanup_effects, sync_startup_specs,
};

const RESTRICTED: &str =
    "Browser extension management is restricted to user actions in Settings > Browser.";

fn require_builtin_user(lua: &Lua) -> mlua::Result<()> {
    let sc = scope::TabScope::capture(lua);
    if sc.is_builtin_browser && sc.tool_sid.is_none() && sc.in_flight.is_empty() {
        return Ok(());
    }
    Err(mlua::Error::runtime(RESTRICTED))
}

fn decode_stored(lua: &Lua, val: Value) -> Vec<StoredExtension> {
    let Ok(data) = <Data as mlua::FromLua>::from_lua(val, lua) else {
        return Vec::new();
    };
    let json = data.to_json();
    let Some(arr) = json.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|item| serde_json::from_value::<StoredExtension>(item.clone()).ok())
        .collect()
}

fn to_lua_json(lua: &Lua, val: &impl serde::Serialize) -> mlua::Result<Value> {
    let json = serde_json::to_value(val).unwrap_or(serde_json::Value::Null);
    mlua::IntoLua::into_lua(json_to_data(&json), lua)
}

fn failed_data(reason: InstallFailure) -> Data {
    map([
        ("type", Data::String("failed".into())),
        ("reason", Data::String(failure_code_str(reason).into())),
    ])
}

fn finish_stage_result(
    core: &Arc<Mutex<ExtInstallCore>>,
    mgr: &ExtensionManager,
    generation: u64,
    result: Result<StagedInstall, (InstallFailure, Option<String>)>,
) -> Data {
    match result {
        Ok(staged) => {
            let review_json = serde_json::to_value(&staged.review).unwrap_or(serde_json::Value::Null);
            let effects = {
                let mut guard = core.lock().unwrap_or_else(PoisonError::into_inner);
                let effs = guard.step(InstallEvent::Staged {
                    generation,
                    token: staged.token.clone(),
                    staged_dir: staged.staged_dir.clone(),
                    record: Box::new(staged.record.clone()),
                    review: Box::new(staged.review.clone()),
                });
                if effs.is_empty() {
                    guard.staged = Some(staged);
                } else {
                    // Generation was superseded before staging completed: clean up the stale dir.
                    mgr.remove_staging_dir(&staged.staged_dir);
                }
                effs
            };
            run_cleanup_effects(mgr, &effects);
            if !effects.is_empty() {
                return map([("type", Data::String("cancelled".into()))]);
            }
            map([
                ("type", Data::String("review".into())),
                ("review", json_to_data(&review_json)),
            ])
        }
        Err((reason, message)) => {
            let effects = {
                let mut guard = core.lock().unwrap_or_else(PoisonError::into_inner);
                guard.step(InstallEvent::StageFailed { generation, reason, message })
            };
            run_cleanup_effects(mgr, &effects);
            failed_data(reason)
        }
    }
}

pub(super) fn service_methods(lua: &Lua, vmx: &std::rc::Rc<Vmx>, t: &Table) -> mlua::Result<()> {
    method(lua, t, "ExtAvailable", |lua, ()| {
        cx::read(Dep::Browser);
        if require_builtin_user(lua).is_err() {
            return Ok(false);
        }
        with_app("Browser:ExtAvailable", |app| {
            Ok(!app.browser_unsupported() && manager().is_ok())
        })
    })?;

    method(lua, t, "ExtList", |lua, stored_val: Value| {
        require_builtin_user(lua)?;
        cx::read(Dep::Browser);
        let stored = decode_stored(lua, stored_val);
        let list: Vec<ProfileExtension> = with_app("Browser:ExtList", |app| {
            let mut changed_ids = app
                .kernel
                .slots
                .get(&BROWSER_EXTENSIONS)
                .integrity_failed
                .clone();
            let Ok(mgr) = manager() else {
                return Ok(Vec::new());
            };
            let verified = mgr.verify_enabled_for_startup(&stored);
            for id in verified.changed_ids {
                changed_ids.insert(id);
            }
            let stored_for_sync = stored.clone();
            app.later(Box::new(move |s: &mut App, _: &mut Tx| {
                let _ = sync_startup_specs(s, &stored_for_sync);
            }));
            let mut out: Vec<ProfileExtension> = stored
                .iter()
                .filter_map(|rec| {
                    let is_changed = changed_ids.contains(&rec.id);
                    mgr.describe(rec, is_changed, None)
                })
                .collect();
            out.sort_by_key(|e| e.name.to_ascii_lowercase());
            Ok(out)
        })?;
        to_lua_json(lua, &list)
    })?;

    method(lua, t, "ExtBrowserSources", |lua, ()| {
        require_builtin_user(lua)?;
        let sources = list_chromium_sources(&platform_dirs());
        to_lua_json(lua, &sources)
    })?;

    method(
        lua,
        t,
        "ExtBrowserList",
        |lua, (browser_id, profile_id, stored_val): (String, String, Value)| {
            require_builtin_user(lua)?;
            let stored = decode_stored(lua, stored_val);
            let changed = with_app("Browser:ExtBrowserList", |app| {
                Ok(app
                    .kernel
                    .slots
                    .get(&BROWSER_EXTENSIONS)
                    .integrity_failed
                    .clone())
            })?;
            let list = list_browser_extensions(&platform_dirs(), &browser_id, &profile_id, &stored, &changed);
            to_lua_json(lua, &list)
        },
    )?;

    method(lua, t, "ExtCancelReview", |lua, ()| {
        require_builtin_user(lua)?;
        with_app("Browser:ExtCancelReview", |app| {
            let core = app.kernel.slots.get(&BROWSER_EXTENSIONS).core.clone();
            let effects = {
                let mut guard = core.lock().unwrap_or_else(PoisonError::into_inner);
                let effs = guard.step(InstallEvent::Cancel);
                if let Some(old) = guard.staged.take()
                    && let Ok(mgr) = manager()
                {
                    mgr.remove_staging_dir(&old.staged_dir);
                }
                effs
            };
            if let Ok(mgr) = manager() {
                run_cleanup_effects(&mgr, &effects);
            }
            Ok(())
        })
    })?;

    method(
        lua,
        t,
        "ExtSetEnabled",
        |lua, (ext_id, enabled, stored_val): (String, bool, Value)| {
            require_builtin_user(lua)?;
            let mut stored = decode_stored(lua, stored_val);
            let mut found = false;
            for rec in &mut stored {
                if rec.id == ext_id {
                    rec.enabled = enabled;
                    found = true;
                    break;
                }
            }
            if !found {
                return mlua::IntoLua::into_lua(map([("ok", Data::Bool(false))]), lua);
            }
            let records_json =
                serde_json::to_value(&stored).unwrap_or(serde_json::Value::Array(Vec::new()));
            with_app("Browser:ExtSetEnabled", |app| {
                app.later(Box::new(move |s: &mut App, _: &mut Tx| {
                    if !enabled {
                        s.workspace_panes.browser.host.disable_extension_live(&ext_id);
                    }
                    let _ = sync_startup_specs(s, &stored);
                    if enabled
                        && s.workspace_panes.browser.host.has_live_sidecar()
                        && s.workspace_panes.browser.host.engine_extensions()
                        && !s.workspace_panes.browser.host.reenable_extension_live(&ext_id)
                    {
                        s.workspace_panes.browser.host.restart_engine_for_extensions();
                    }
                }));
                Ok(())
            })?;
            mlua::IntoLua::into_lua(
                map([("ok", Data::Bool(true)), ("records", json_to_data(&records_json))]),
                lua,
            )
        },
    )?;

    method(lua, t, "ExtRemove", |lua, (ext_id, stored_val): (String, Value)| {
        require_builtin_user(lua)?;
        let mut stored = decode_stored(lua, stored_val);
        let removed = stored
            .iter()
            .position(|r| r.id == ext_id)
            .map(|pos| stored.remove(pos));
        let Some(rec) = removed else {
            return mlua::IntoLua::into_lua(map([("ok", Data::Bool(false))]), lua);
        };
        let records_json =
            serde_json::to_value(&stored).unwrap_or(serde_json::Value::Array(Vec::new()));
        with_app("Browser:ExtRemove", |app| {
            app.later(Box::new(move |s: &mut App, _: &mut Tx| {
                s.workspace_panes.browser.host.disable_extension_live(&ext_id);
                let core = s.kernel.slots.get(&BROWSER_EXTENSIONS).core.clone();
                let effects = core
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .step(InstallEvent::ExtensionRemoved { id: ext_id.clone() });
                s.kernel
                    .slots
                    .get_mut(&BROWSER_EXTENSIONS)
                    .integrity_failed
                    .remove(&ext_id);
                if let Ok(mgr) = manager() {
                    run_cleanup_effects(&mgr, &effects);
                    mgr.remove_managed_dir(&rec);
                }
                let _ = sync_startup_specs(s, &stored);
            }));
            Ok(())
        })?;
        mlua::IntoLua::into_lua(
            map([("ok", Data::Bool(true)), ("records", json_to_data(&records_json))]),
            lua,
        )
    })?;

    method(lua, t, "ExtSync", |lua, (stored_val, restart_if_live): (Value, Option<bool>)| {
        require_builtin_user(lua)?;
        let stored = decode_stored(lua, stored_val);
        with_app("Browser:ExtSync", |app| {
            app.later(Box::new(move |s: &mut App, _: &mut Tx| {
                let _ = sync_startup_specs(s, &stored);
                if restart_if_live == Some(true)
                    && s.workspace_panes.browser.host.has_live_sidecar()
                    && s.workspace_panes.browser.host.engine_extensions()
                {
                    s.workspace_panes.browser.host.restart_engine_for_extensions();
                }
            }));
            Ok(())
        })
    })?;

    for (method_name, async_key) in [
        ("ExtStageStoreAsync", "Browser.ExtStageStoreAsync"),
        ("ExtStageBrowserAsync", "Browser.ExtStageBrowserAsync"),
        ("ExtStageFolderAsync", "Browser.ExtStageFolderAsync"),
        ("ExtApproveAsync", "Browser.ExtApproveAsync"),
    ] {
        let raw: Function = vmx.asyncs.raw_get(async_key)?;
        let wrapped: Function = lua
            .load("local raw = ... return function(_self, ...) return raw(...) end")
            .set_name(format!("=Browser:{method_name}"))
            .call(raw)?;
        t.raw_set(method_name, wrapped)?;
    }

    Ok(())
}

pub(super) fn asyncs(host: &Host<'_>, t: &Table) -> mlua::Result<()> {
    t.raw_set(
        "Browser.ExtStageStoreAsync",
        host.async_function("Browser:ExtStageStoreAsync", stage_store)?,
    )?;
    t.raw_set(
        "Browser.ExtStageBrowserAsync",
        host.async_function("Browser:ExtStageBrowserAsync", stage_browser)?,
    )?;
    t.raw_set(
        "Browser.ExtStageFolderAsync",
        host.async_function("Browser:ExtStageFolderAsync", stage_folder)?,
    )?;
    t.raw_set(
        "Browser.ExtApproveAsync",
        host.async_function("Browser:ExtApproveAsync", approve_review)?,
    )
}

fn begin_stage(core: &Arc<Mutex<ExtInstallCore>>, mgr: &ExtensionManager, source: ExtensionSource) -> Option<u64> {
    let mut guard = core.lock().unwrap_or_else(PoisonError::into_inner);
    let generation = guard.next_generation();
    let effects = guard.step(InstallEvent::BeginStage { generation, source });
    if !matches!(guard.machine.state, browser_extensions::review::InstallState::Staging { generation: g, .. } if g == generation) {
        return None;
    }
    if let Some(old) = guard.staged.take() {
        mgr.remove_staging_dir(&old.staged_dir);
    }
    run_cleanup_effects(mgr, &effects);
    Some(generation)
}

fn stage_store(lua: &Lua, a: MultiValue, done: Completer) -> mlua::Result<()> {
    require_builtin_user(lua)?;
    let (url, stored_val): (String, Value) = mlua::FromLuaMulti::from_lua_multi(a, lua)?;
    let stored = decode_stored(lua, stored_val);
    with_app("Browser:ExtStageStoreAsync", |app| {
        app.later(Box::new(move |s: &mut App, _: &mut Tx| {
            let Ok(mgr) = manager() else {
                return done.complete(failed_data(InstallFailure::Load));
            };
            let core = s.kernel.slots.get(&BROWSER_EXTENSIONS).core.clone();
            let Some(generation) = begin_stage(&core, &mgr, ExtensionSource::Store) else {
                return done.complete(failed_data(InstallFailure::Load));
            };
            rune_luau::spawn_blocking(move || {
                let res = mgr.stage_store_install(&url, CHROME_STORE_VERSION, &stored);
                done.complete(finish_stage_result(&core, &mgr, generation, res));
            });
        }));
        Ok(())
    })
}

fn stage_browser(lua: &Lua, a: MultiValue, done: Completer) -> mlua::Result<()> {
    require_builtin_user(lua)?;
    let (browser_id, profile_id, ext_id, stored_val): (String, String, String, Value) =
        mlua::FromLuaMulti::from_lua_multi(a, lua)?;
    let stored = decode_stored(lua, stored_val);
    with_app("Browser:ExtStageBrowserAsync", |app| {
        app.later(Box::new(move |s: &mut App, _: &mut Tx| {
            let Ok(mgr) = manager() else {
                return done.complete(failed_data(InstallFailure::Load));
            };
            let core = s.kernel.slots.get(&BROWSER_EXTENSIONS).core.clone();
            let Some(generation) = begin_stage(&core, &mgr, ExtensionSource::Browser) else {
                return done.complete(failed_data(InstallFailure::Load));
            };
            let dirs = platform_dirs();
            rune_luau::spawn_blocking(move || {
                let res = mgr.stage_browser_install(&dirs, &browser_id, &profile_id, &ext_id, &stored);
                done.complete(finish_stage_result(&core, &mgr, generation, res));
            });
        }));
        Ok(())
    })
}

fn stage_folder(lua: &Lua, a: MultiValue, done: Completer) -> mlua::Result<()> {
    require_builtin_user(lua)?;
    let (title, stored_val): (String, Value) = mlua::FromLuaMulti::from_lua_multi(a, lua)?;
    let stored = decode_stored(lua, stored_val);
    with_app("Browser:ExtStageFolderAsync", |app| {
        app.later(Box::new(move |s: &mut App, tx: &mut Tx| {
            let opts = FileDialogOptions { title, ..Default::default() };
            let core = s.kernel.slots.get(&BROWSER_EXTENSIONS).core.clone();
            tx.open_folder(opts, move |_app: &mut App, picked, _tx: &mut Tx| {
                let Ok(Some(folder)) = picked else {
                    return done.complete(map([("type", Data::String("cancelled".into()))]));
                };
                let Ok(mgr) = manager() else {
                    return done.complete(failed_data(InstallFailure::Load));
                };
                let Some(generation) = begin_stage(&core, &mgr, ExtensionSource::Folder) else {
                    return done.complete(failed_data(InstallFailure::Load));
                };
                rune_luau::spawn_blocking(move || {
                    let res = mgr.stage_folder_install(&folder, &stored);
                    done.complete(finish_stage_result(&core, &mgr, generation, res));
                });
            });
        }));
        Ok(())
    })
}

fn approve_review(lua: &Lua, a: MultiValue, done: Completer) -> mlua::Result<()> {
    require_builtin_user(lua)?;
    let (token, stored_val): (String, Value) = mlua::FromLuaMulti::from_lua_multi(a, lua)?;
    let mut stored = decode_stored(lua, stored_val);
    with_app("Browser:ExtApproveAsync", |app| {
        app.later(Box::new(move |s: &mut App, _: &mut Tx| {
            let Ok(mgr) = manager() else {
                return done.complete(failed_data(InstallFailure::Load));
            };
            let core = s.kernel.slots.get(&BROWSER_EXTENSIONS).core.clone();
            let (effects, staged_opt) = {
                let mut guard = core.lock().unwrap_or_else(PoisonError::into_inner);
                let effs = guard.step(InstallEvent::Approve { token: token.clone() });
                let staged = if effs.iter().any(|e| matches!(e, InstallEffect::VerifyAndSwap { .. })) {
                    guard.staged.take()
                } else {
                    if let Some(old) = guard.staged.take() {
                        mgr.remove_staging_dir(&old.staged_dir);
                    }
                    None
                };
                (effs, staged)
            };
            run_cleanup_effects(&mgr, &effects);
            if let Some(reason) = effects.iter().find_map(|e| match e {
                InstallEffect::RejectApproval(r) => Some(*r),
                _ => None,
            }) {
                return done.complete(failed_data(reason));
            }
            let Some(staged) = staged_opt else {
                let mut guard = core.lock().unwrap_or_else(PoisonError::into_inner);
                let _ = guard.step(InstallEvent::SwapFailed { reason: InstallFailure::Expired, message: None });
                return done.complete(failed_data(InstallFailure::Expired));
            };
            // Unload any running copy first so Windows file handles inside `<root>/<id>` do not block rename.
            s.workspace_panes.browser.host.disable_extension_live(&staged.record.id);
            s.kernel
                .slots
                .get_mut(&BROWSER_EXTENSIONS)
                .integrity_failed
                .remove(&staged.record.id);
            rune_luau::spawn_blocking(move || {
                match mgr.approve_staged(staged, &token) {
                    Ok(swap) => {
                        if let Some(pos) = stored.iter().position(|r| r.id == swap.record.id) {
                            stored[pos] = swap.record.clone();
                        } else {
                            stored.push(swap.record.clone());
                        }
                        mgr.finalize_swap(&swap);
                        {
                            let mut guard = core.lock().unwrap_or_else(PoisonError::into_inner);
                            let _ = guard.step(InstallEvent::SwapSucceeded);
                        }
                        let ext_json = serde_json::to_value(&swap.profile).unwrap_or(serde_json::Value::Null);
                        let recs_json = serde_json::to_value(&stored).unwrap_or(serde_json::Value::Array(Vec::new()));
                        done.complete(map([
                            ("type", Data::String("installed".into())),
                            ("extension", json_to_data(&ext_json)),
                            ("records", json_to_data(&recs_json)),
                        ]));
                    }
                    Err(reason) => {
                        let effects = {
                            let mut guard = core.lock().unwrap_or_else(PoisonError::into_inner);
                            guard.step(InstallEvent::SwapFailed { reason, message: None })
                        };
                        run_cleanup_effects(&mgr, &effects);
                        done.complete(failed_data(reason));
                    }
                }
            });
        }));
        Ok(())
    })
}
