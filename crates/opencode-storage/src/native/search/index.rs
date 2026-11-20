//! Only the dirty entity key is written on a durability commit. Derive the text from the
//! committed entity on a later writer turn. Reading, replacing and acknowledging that exact
//! revision share a transaction, so deleting or resending cannot resurrect stale postings.
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

use crate::native::{boot_record::session_of, sql};

const DIRTY: &str = "rune.search.dirty";

pub(crate) fn mark(tx: &Connection, entity: &str, now: i64) -> rusqlite::Result<()> {
    tx.prepare_cached("INSERT INTO state(name,key,value,updated_at) VALUES ('rune.search.dirty',?1,'1',?2)
        ON CONFLICT(name,key) DO UPDATE SET value=CAST(value AS INTEGER)+1,updated_at=excluded.updated_at")?.execute(params![entity,now])?;
    Ok(())
}

pub(crate) fn draft(tx:&Connection, key:&str, now:i64) -> rusqlite::Result<()> {
    if key.starts_with("opencode.window.") && key.contains(".dat:draft:") { mark(tx,&format!("d\n{key}"),now)?; }
    Ok(())
}

pub(crate) fn outbox(tx:&Connection, server:&str, id:&str, now:i64) -> rusqlite::Result<()> {
    mark(tx,&format!("o\n{server}\n{id}"),now)
}

fn words(doc:&str) -> Option<String> {
    let mut v=serde_json::from_str::<Value>(doc).ok()?;
    match v { Value::String(s)=>Some(s), _ => v.get_mut("text").and_then(|t| t.take().as_str().map(str::to_owned)) }
}

fn remove(c:&Connection, id:i64) -> rusqlite::Result<()> {
    c.prepare_cached("DELETE FROM search_fts WHERE rowid=?1")?.execute([id])?;
    c.prepare_cached("DELETE FROM search_meta WHERE id=?1")?.execute([id])?;
    Ok(())
}

fn insert(c:&Connection, server:&str, session:&str, entity:&str, kind:&str, body:&str, now:i64) -> rusqlite::Result<()> {
    if body.is_empty(){return Ok(())}
    let id:i64=c.prepare_cached("INSERT INTO state(name,key,value,updated_at) VALUES('rune.search','next_seq','-1',?1)
        ON CONFLICT(name,key) DO UPDATE SET value=CAST(value AS INTEGER)-1,updated_at=excluded.updated_at RETURNING CAST(value AS INTEGER)")?.query_row([now],|r|r.get(0))?;
    let snippet=&body[..body.floor_char_boundary(400.min(body.len()))];
    let compressed=zstd::bulk::compress(snippet.as_bytes(),1).map_err(|e|rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    c.prepare_cached("INSERT INTO search_meta(id,server,session_id,project_dir,message_id,kind,archived,updated_ms,title,snippet_zstd) VALUES (?1,?2,?3,'',?4,?5,0,?6,'',?7)")?
        .execute(params![id,server,session,entity,kind,now,compressed])?;
    c.prepare_cached("INSERT INTO search_fts(rowid,title,body) VALUES(?1,'',?2)")?.execute(params![id,body])?;
    Ok(())
}

/// One entity per admission. False means empty: block until another commit, no idle polling.
/// What one indexing step did.
pub(crate) enum Step {
    /// The queue is empty.
    Empty,
    /// A write or a flush came first: the step was rolled back, to be done again later.
    Yielded,
    /// `key` was indexed: its read and index, and its commit, in µs.
    Done { key: String, work_us: u128, commit_us: u128 },
}

/// One entity, as a step the writer's own work comes before: a command queued meanwhile
/// (`urgent` moves) interrupts it between SQLite's instructions (it's derivable; it's done again).
pub(crate) fn step(c: &mut Connection, urgent: &std::sync::Arc<std::sync::atomic::AtomicU64>) -> rusqlite::Result<Step> {
    let admitted = urgent.load(std::sync::atomic::Ordering::Acquire);
    let watch = urgent.clone();
    c.progress_handler(1000, Some(move || watch.load(std::sync::atomic::Ordering::Acquire) != admitted))?;
    let t = std::time::Instant::now();
    let r = consume(c);
    c.progress_handler(0, None::<fn() -> bool>)?;
    match r {
        Ok(Some((key, work))) => Ok(Step::Done { key, work_us: work.as_micros(), commit_us: (t.elapsed() - work).as_micros() }),
        Ok(None) => Ok(Step::Empty),
        Err(rusqlite::Error::SqliteFailure(f, _)) if f.code == rusqlite::ErrorCode::OperationInterrupted => Ok(Step::Yielded),
        Err(e) => Err(e),
    }
}

fn consume(c:&mut Connection) -> rusqlite::Result<Option<(String, std::time::Duration)>> {
    let started = std::time::Instant::now();
    let tx=c.transaction()?;
    let item:Option<(String,String,i64)>=tx.prepare_cached("SELECT key,value,updated_at FROM state WHERE name=?1 ORDER BY key LIMIT 1")?
        .query_row([DIRTY],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    let Some((key,revision,now))=item else{return Ok(None)};
    if let Some(doc)=key.strip_prefix("d\n") && let Some((_,tab))=doc.split_once(".dat:draft:") {
        let (server,session)=session_of(tab).unwrap_or(("",tab));
        let old=tx.prepare_cached("SELECT id FROM search_meta WHERE server=?1 AND session_id=?2 AND message_id=?3 AND kind='draft'")?.query_row(params![server,session,doc],|r|r.get(0)).optional()?;
        if let Some(id)=old{remove(&tx,id)?}
        if let Some(text)=sql::read_document(&tx,doc)?.as_deref().and_then(words) {insert(&tx,server,session,doc,"draft",&text,now)?}
    } else if let Some((server,id))=key.strip_prefix("o\n").and_then(|s|s.split_once('\n')) {
        let old=tx.prepare_cached("SELECT id FROM search_meta WHERE server=?1 AND message_id=?2 AND kind='outbox'")?.query_row(params![server,id],|r|r.get(0)).optional()?;
        if let Some(id)=old{remove(&tx,id)?}
        // A queued prompt is the user's words.
        if let Some(row)=sql::read_outbox_row(&tx,server,id)? && let Some(text)=words(&row.draft_json){insert(&tx,server,&row.session,id,"outbox",&text,now)?}
    } else if let Some((server,session))=key.strip_prefix("rune.copy\n").and_then(|s|s.split_once('\n')) {
        use opencode_kit::model::{Message, Part, preview};
        let old=tx.prepare_cached("SELECT id FROM search_meta WHERE server=?1 AND session_id=?2 AND kind IN ('user','assistant')")?.query_map([server,session],|r|r.get(0))?.collect::<rusqlite::Result<Vec<i64>>>()?;
        for id in old {remove(&tx,id)?}
        let bytes:Option<Vec<u8>>=tx.prepare_cached("SELECT data FROM cache WHERE key=?1 AND version=?2")?.query_row(params![key,preview::VERSION],|r|r.get(0)).optional()?;
        if let Some(messages)=bytes.and_then(|b|preview::decode_for_search(&b)) {
            for msg in messages {
                // SEARCH's kinds (`opencode_search::fulltext::Kind`): what the user wrote, what came back.
                let (kind,body)=match &msg { Message::User{text,..}|Message::Note{text,..}=>("user",text.clone()), Message::Assistant(a)=>("assistant",a.parts.iter().filter_map(|p|match p {Part::Text{text}=>Some(text.as_str()),_=>None}).collect::<Vec<_>>().join("\n")), Message::Compaction(c)=>("assistant",c.summary.clone()), Message::Notice(_)=>continue };
                insert(&tx,server,session,msg.id(),kind,&body,now)?;
            }
        }
    } else {return Err(rusqlite::Error::InvalidQuery)}
    tx.prepare_cached("DELETE FROM state WHERE name=?1 AND key=?2 AND value=?3")?.execute(params![DIRTY,key,revision])?;
    let work = started.elapsed();
    tx.commit()?;
    Ok(Some((key, work)))
}
