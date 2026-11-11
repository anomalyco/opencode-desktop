//! The outbox in IndexedDB (`outbox`, `blobs`): a put's blobs and row go in one readwrite
//! transaction, and its callback runs when that transaction completes, as durable as the
//! browser makes it. Rows are keyed `<server>\n<id>`.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::{IdbTransaction, IdbTransactionMode};

use super::{OutboxEntry, OutboxOp, Written};
use crate::web::{Idb, Web, bytes, now_ms, on_success, outbox_from, outbox_json};
use crate::{Done, OutboxRow, StorageError};

fn key(server: &str, id: &str) -> JsValue {
    JsValue::from_str(&format!("{server}\n{id}"))
}

fn transaction(web: &Rc<RefCell<Web>>, stores: &[&str], mode: IdbTransactionMode) -> Option<IdbTransaction> {
    let w = web.borrow();
    let Idb::Open(db) = &w.idb else { return None };
    let names: js_sys::Array = stores.iter().map(|s| JsValue::from_str(s)).collect();
    db.transaction_with_str_sequence_and_mode(&names, mode).ok()
}

/// `done` once `tx` completes (`Ok`) or fails or aborts (`Err`).
fn on_complete(tx: &IdbTransaction, done: impl FnOnce(Result<(), StorageError>) + 'static) {
    let done = Rc::new(RefCell::new(Some(done)));
    let ok = done.clone();
    let complete = Closure::once_into_js(move || {
        if let Some(d) = ok.borrow_mut().take() {
            d(Ok(()));
        }
    });
    tx.set_oncomplete(Some(complete.unchecked_ref()));
    let failed = done.clone();
    let error = Closure::once_into_js(move || {
        if let Some(d) = failed.borrow_mut().take() {
            d(Err(StorageError("IndexedDB outbox write failed".into())));
        }
    });
    tx.set_onerror(Some(error.unchecked_ref()));
    let aborted = done;
    let abort = Closure::once_into_js(move || {
        if let Some(d) = aborted.borrow_mut().take() {
            d(Err(StorageError("IndexedDB outbox write aborted".into())));
        }
    });
    tx.set_onabort(Some(abort.unchecked_ref()));
}

pub(crate) fn handle(web: &Rc<RefCell<Web>>, op: OutboxOp, done: Option<Done>) {
    let unavailable = |done: Option<Done>| {
        if let Some(done) = done {
            done(Err(StorageError("IndexedDB is unavailable".into())));
        }
    };
    match op {
        OutboxOp::Put(build) => {
            let put = build();
            let Some(tx) = transaction(web, &["blobs", "outbox", "documents"], IdbTransactionMode::Readwrite) else { return unavailable(done) };
            let (Ok(blobs), Ok(rows), Ok(docs)) = (tx.object_store("blobs"), tx.object_store("outbox"), tx.object_store("documents")) else { return unavailable(done) };
            for key in &put.clear_documents {
                docs.delete(&JsValue::from_str(key)).ok();
            }
            let now = now_ms();
            for (id, data) in &put.blobs {
                blobs.put_with_key(&js_sys::Uint8Array::from(&**data).into(), &JsValue::from_str(id)).ok();
                web.borrow_mut().touched.insert(id.clone(), now);
            }
            rows.put_with_key(&JsValue::from_str(&outbox_json(&put.row)), &key(&put.row.server, &put.row.id)).ok();
            settle(web, &tx, Written::Put(put.row), done);
        }
        OutboxOp::Update { server, id, update } => {
            let Some(tx) = transaction(web, &["outbox"], IdbTransactionMode::Readwrite) else { return unavailable(done) };
            let Ok(rows) = tx.object_store("outbox") else { return unavailable(done) };
            let Ok(req) = rows.get(&key(&server, &id)) else { return unavailable(done) };
            let (u, k, t) = (update.clone(), key(&server, &id), tx.clone());
            on_success(&req, move |v| match v.as_string().as_deref().and_then(outbox_from) {
                Some(mut row) => {
                    row.state = u.state;
                    row.why = u.why;
                    row.after_check = u.after_check;
                    row.restored = u.restored;
                    row.operation_json = u.operation_json;
                    rows.put_with_key(&JsValue::from_str(&outbox_json(&row)), &k).ok();
                }
                // No such row: the update fails, as a native one does.
                None => {
                    t.abort().ok();
                }
            });
            settle(web, &tx, Written::Update { server, id, update }, done);
        }
        OutboxOp::Delete { server, id } => {
            let Some(tx) = transaction(web, &["outbox"], IdbTransactionMode::Readwrite) else { return unavailable(done) };
            let Ok(rows) = tx.object_store("outbox") else { return unavailable(done) };
            rows.delete(&key(&server, &id)).ok();
            settle(web, &tx, Written::Delete { server, id }, done);
        }
        OutboxOp::Read { server, adopt, done } => read(web, server, adopt, done),
    }
}

/// Once `tx` completes: the rows in memory follow, then `done`.
fn settle(web: &Rc<RefCell<Web>>, tx: &IdbTransaction, written: Written, done: Option<Done>) {
    let cache = web.borrow().cache.clone();
    on_complete(tx, move |result| {
        if result.is_ok() {
            super::cached(&mut cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner).outbox, &written);
        }
        if let Some(done) = done {
            done(result);
        }
    });
}

/// Every row of `server` and of the `adopt` names (moved to `server` in the same transaction),
/// then the bytes of the blobs they name.
fn read(web: &Rc<RefCell<Web>>, server: String, adopt: Vec<String>, done: super::OutboxRead) {
    let Some(tx) = transaction(web, &["outbox"], IdbTransactionMode::Readwrite) else {
        return done(Err(StorageError("IndexedDB is unavailable".into())));
    };
    let Ok(store) = tx.object_store("outbox") else { return done(Err(StorageError("IndexedDB is unavailable".into()))) };
    let Ok(req) = store.get_all() else { return done(Err(StorageError("IndexedDB is unavailable".into()))) };
    let found: Rc<RefCell<Vec<OutboxRow>>> = Rc::default();
    let into = found.clone();
    let target = server.clone();
    on_success(&req, move |all| {
        let rows: Vec<OutboxRow> = js_sys::Array::from(&all).iter().filter_map(|v| v.as_string()).filter_map(|t| outbox_from(&t)).collect();
        let mine: Vec<String> = rows.iter().filter(|r| r.server == target).map(|r| r.id.clone()).collect();
        for mut row in rows {
            if adopt.contains(&row.server) {
                store.delete(&key(&row.server, &row.id)).ok();
                if mine.contains(&row.id) {
                    continue;
                }
                row.server.clone_from(&target);
                store.put_with_key(&JsValue::from_str(&outbox_json(&row)), &key(&target, &row.id)).ok();
            }
            if row.server == target {
                into.borrow_mut().push(row);
            }
        }
    });
    let web = web.clone();
    on_complete(&tx, move |result| {
        if let Err(e) = result {
            return done(Err(e));
        }
        let mut rows = std::mem::take(&mut *found.borrow_mut());
        rows.sort_by_key(|r| r.seq);
        with_blobs(&web, rows, done);
    });
}

/// Reads each blob the rows name, then answers with the rows and their bytes.
fn with_blobs(web: &Rc<RefCell<Web>>, rows: Vec<OutboxRow>, done: super::OutboxRead) {
    let mut ids: Vec<String> = Vec::new();
    for row in &rows {
        ids.extend(row.attachments.iter().filter_map(|a| a.blob.clone()));
        if let Some(p) = row.prompt_json.as_deref().and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok()) {
            ids.extend(p["files"].as_array().into_iter().flatten().filter_map(|f| f["uri"].as_str()?.strip_prefix("blob:").map(str::to_owned)));
        }
    }
    ids.sort();
    ids.dedup();
    let finish = move |loaded: Vec<(String, Arc<[u8]>)>| {
        let entries = rows
            .into_iter()
            .map(|row| {
                let blobs = loaded.iter().filter(|(id, _)| row.attachments.iter().any(|a| a.blob.as_deref() == Some(id)) || row.prompt_json.as_deref().is_some_and(|p| p.contains(&format!("blob:{id}")))).cloned().collect();
                OutboxEntry { row, blobs }
            })
            .collect();
        done(Ok(entries));
    };
    if ids.is_empty() {
        return finish(Vec::new());
    }
    let Some(tx) = transaction(web, &["blobs"], IdbTransactionMode::Readonly) else { return finish(Vec::new()) };
    let Ok(store) = tx.object_store("blobs") else { return finish(Vec::new()) };
    let loaded: Rc<RefCell<Vec<(String, Arc<[u8]>)>>> = Rc::default();
    for id in ids {
        let Ok(req) = store.get(&JsValue::from_str(&id)) else { continue };
        let into = loaded.clone();
        on_success(&req, move |v| {
            if let Some(b) = bytes(&v) {
                into.borrow_mut().push((id, Arc::from(b)));
            }
        });
    }
    on_complete(&tx, move |_| finish(std::mem::take(&mut *loaded.borrow_mut())));
}
