//! The FTS5 tokenizer is connection-local. Register it before touching the index,
//! not in SQLite initialization (which also runs on the first-frame path).
#![allow(unsafe_code)]

use std::ffi::{c_char, c_int, c_void};
use std::ptr;

use opencode_search::tokenizer::{Mode, tokenize};
use rusqlite::{Connection, ffi, types::ToSqlOutput};

pub(super) fn register(c: &Connection) -> rusqlite::Result<()> {
    let mut api: *mut ffi::fts5_api = ptr::null_mut();
    let ptr = ToSqlOutput::Pointer(((&raw mut api).cast(), c"fts5_api_ptr", None));
    c.query_row("SELECT fts5(?1)", [ptr], |_| Ok(()))?;
    // SAFETY: SQLite's pointer-passing interface filled `api` during the statement;
    // it lives as long as this connection. xCreateTokenizer copies the methods.
    let api = unsafe { api.as_mut() }.ok_or(rusqlite::Error::InvalidQuery)?;
    let register = api.xCreateTokenizer.ok_or(rusqlite::Error::InvalidQuery)?;
    let mut methods = ffi::fts5_tokenizer { xCreate: Some(create), xDelete: Some(delete), xTokenize: Some(tokens) };
    let rc = unsafe { register(api, c"opencode_tok".as_ptr(), ptr::null_mut(), &raw mut methods, None) };
    if rc == ffi::SQLITE_OK { Ok(()) } else { Err(rusqlite::Error::SqliteFailure(ffi::Error::new(rc), None)) }
}

unsafe extern "C" fn create(_: *mut c_void, _: *mut *const c_char, argc: c_int, out: *mut *mut ffi::Fts5Tokenizer) -> c_int {
    if argc != 0 || out.is_null() { return ffi::SQLITE_ERROR; }
    // No state per tokenizer; FTS requires an opaque non-null handle, never dereferenced.
    unsafe { out.write(ptr::NonNull::<u8>::dangling().as_ptr().cast()) };
    ffi::SQLITE_OK
}

unsafe extern "C" fn delete(_: *mut ffi::Fts5Tokenizer) {}

type Emit = unsafe extern "C" fn(*mut c_void, c_int, *const c_char, c_int, c_int, c_int) -> c_int;

unsafe extern "C" fn tokens(_: *mut ffi::Fts5Tokenizer, ctx: *mut c_void, flags: c_int, text: *const c_char, n: c_int, emit: Option<Emit>) -> c_int {
    if n == 0 { return ffi::SQLITE_OK; }
    let Some(emit) = emit else { return ffi::SQLITE_ERROR };
    if text.is_null() || n < 0 { return ffi::SQLITE_ERROR; }
    // SAFETY: FTS lends `n` input bytes and `ctx` for this synchronous call. The callback
    // consumes (doesn't own) each token, including its original UTF-8 byte offsets.
    let bytes = unsafe { std::slice::from_raw_parts(text.cast(), n as usize) };
    let Ok(text) = std::str::from_utf8(bytes) else { return ffi::SQLITE_ERROR };
    std::panic::catch_unwind(|| {
        let mode = if flags & ffi::FTS5_TOKENIZE_DOCUMENT != 0 { Mode::Document } else { Mode::Query };
        let mut rc = ffi::SQLITE_OK;
        tokenize(text, mode, |t| {
            if rc == ffi::SQLITE_OK {
                let Ok(len) = c_int::try_from(t.text.len()) else { rc = ffi::SQLITE_TOOBIG; return };
                let colocated = if t.colocated { ffi::FTS5_TOKEN_COLOCATED } else { 0 };
                rc = unsafe { emit(ctx, colocated, t.text.as_ptr().cast(), len, t.bytes.start as c_int, t.bytes.end as c_int) };
            }
        });
        rc
    }).unwrap_or(ffi::SQLITE_ERROR)
}
