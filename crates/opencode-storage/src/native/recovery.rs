//! Conflict-preserving writes for a memory-only workspace. No schema changes and no
//! recovery-list read before the first frame. Images are ordinary document blob references,
//! so the existing blob sweep can reach them without a new index or a periodic scan.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::path::Path;
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde_json::Value;
use crate::{Cmd, StorageError};
use crate::recovery::{Decision, Draft, Versions, versions_key};
use super::{OriginalFile, lease, sql};

#[derive(Clone, Default)]
pub struct MemoryDrafts {
    pub revision:u64,
    base:Arc<BTreeMap<String,Arc<str>>>,
    docs:BTreeMap<String,Option<Arc<str>>>,
    blobs:BTreeMap<String,Arc<[u8]>>,
    initial_state:Arc<BTreeMap<(String,String),Arc<str>>>,
    state:BTreeMap<(String,String),Option<Arc<str>>>,
    initial_frames:Arc<BTreeMap<String,crate::WindowFrame>>,
    frames:BTreeMap<String,Option<crate::WindowFrame>>,
    plugins:BTreeMap<String,crate::PluginRecord>,
    initial_plugins:Arc<BTreeMap<String,crate::PluginRecord>>,
}

impl MemoryDrafts {
    pub(super) fn baseline(&mut self,c:&crate::cache::Cache) {
        self.base=Arc::new(c.docs.iter().filter_map(|(k,v)|match v {
            crate::cache::Doc::Present(s)=>Some((k.clone(),s.clone())), _=>None,
        }).collect());
        self.initial_state=Arc::new(c.state.keys().flat_map(|n| c.items(n).unwrap_or_default().into_iter().map(move |(k,v)|((n.clone(),k),v))).collect());
        self.initial_frames=Arc::new(c.windows.clone());
        self.initial_plugins=Arc::new(c.plugins.keys().chain(c.trust.keys()).map(|k|(k.clone(),crate::PluginRecord{enabled:c.plugins.get(k).copied(),trusted:c.trust.get(k).cloned()})).collect());
    }
    pub(super) fn observe(&mut self,cmd:&Cmd) -> bool {
        match cmd {
            Cmd::Doc{key,value,..}=>{
                if self.base.get(key)==value.as_ref() { self.docs.remove(key); }
                else { self.docs.insert(key.clone(),value.clone()); }
            }
            Cmd::Blob{id,data}=>{self.blobs.insert(id.clone(),data.clone());}
            Cmd::State{name,key,value}=>{let k=(name.clone(),key.clone());if self.initial_state.get(&k)==value.as_ref(){self.state.remove(&k);}else{self.state.insert(k,value.clone());}}
            Cmd::ClearState{name}=>{self.state.retain(|(n,_),_|n!=name);for k in self.initial_state.keys().filter(|(n,_)|n==name){self.state.insert(k.clone(),None);}}
            Cmd::Window{id,frame}=>{if self.initial_frames.get(id)==frame.as_ref(){self.frames.remove(id);}else{self.frames.insert(id.clone(),*frame);}}
            Cmd::PluginRecord{id,record,..}=>{if self.initial_plugins.get(id)==Some(record){self.plugins.remove(id);}else{self.plugins.insert(id.clone(),record.clone());}}
            _=>return false,
        }
        true
    }
    pub fn draft_keys(&self)->BTreeSet<String> {
        self.docs.keys().filter_map(|k|{
            if crate::journal::is_draft_key(k){Some(k.clone())}
            else{k.split_once(":undo:").map(|(ns,tab)|format!("{ns}:draft:{tab}"))}
        }).collect()
    }
    pub fn has_document_changes(&self)->bool { !self.docs.is_empty() }
    pub fn has_user_changes(&self)->bool {
        self.has_document_changes() || !self.plugins.is_empty() || !self.frames.is_empty() || self.state.keys().any(|(ns,k)| {
            // Speculative transcript copies and cache/LRU bookkeeping are reproducible;
            // authoritative preferences, terminal rows, tabs, and draft edits are not.
            ns!="rune.copies" && !k.starts_with("rune.recent:")
        })
    }
    pub fn changed_state(&self)->impl Iterator<Item=(&str,&str)> { self.state.keys().map(|(n,k)|(n.as_str(),k.as_str())) }
    fn payload(&self,key:&str,base:bool)->Result<Draft,StorageError>{
        let get=|k:&str|{
            let s=if base{self.base.get(k)}else{self.docs.get(k).map_or_else(||self.base.get(k),Option::as_ref)};
            s.map(|s|serde_json::from_str(s)).transpose().map(|v|v.unwrap_or(Value::Null)).map_err(err)
        };
        Ok(Draft{document:get(key)?,history:get(&undo_key(key)?)?})
    }
}

fn err(e:impl std::fmt::Display)->StorageError{StorageError(e.to_string())}
fn undo_key(key:&str)->Result<String,StorageError>{key.split_once(":draft:").map(|(ns,tab)|format!("{ns}:undo:{tab}")).ok_or_else(||err("invalid draft key"))}

/// The owning process must retain this object through the UI disposition. The connection
/// cannot create a new database or quarantine one. Field drop order: connection, lease,
/// no-delete-sharing identity guard. No file is opened writable before identity matches.
pub struct VerifiedWorkspace { conn:Connection, applied:Applied, db:std::path::PathBuf, _owner:lease::Owner, _identity:std::fs::File }
#[derive(Default)]
enum Applied { #[default] Fresh, Through { batch:MemoryDrafts, primary:BTreeMap<String,Draft> } }
impl OriginalFile {
    pub fn reacquire(&self,db:&Path)->Result<VerifiedWorkspace,StorageError>{
        let identity=self.same_existing_file(db).map_err(err)?;
        let owner=lease::acquire(db).map_err(err)?;
        let conn=Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_WRITE|OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(err)?;
        conn.busy_timeout(std::time::Duration::ZERO).map_err(err)?;
        let mode:String=conn.query_row("PRAGMA journal_mode",[],|r|r.get(0)).map_err(err)?;
        if !mode.eq_ignore_ascii_case("wal"){return Err(err("workspace not in WAL mode"));}
        conn.execute_batch("PRAGMA synchronous=FULL; PRAGMA wal_autocheckpoint=0").map_err(err)?;
        Ok(VerifiedWorkspace{conn,applied:Applied::Fresh,db:db.to_path_buf(),_owner:owner,_identity:identity})
    }
}

fn json(c:&Connection,key:&str)->Result<Value,StorageError>{
    let raw:Option<String>=c.query_row("SELECT value FROM document WHERE key=?1",[key],|r|r.get(0)).optional().map_err(err)?;
    let Some(s)=raw else{return Ok(Value::Null)};
    let v=serde_json::from_str(&s).map_err(err)?;
    let mut chunks=Vec::new();crate::chunk::text_refs(&v,&mut chunks);
    let mut verified=BTreeMap::new();
    for id in chunks {if !verified.contains_key(&id){let bytes=sql::read_blob(c,&id).map_err(err)?.ok_or_else(||err("missing document chunk"))?;if crate::chunk::blob_id(&bytes)!=id{return Err(err("invalid document chunk"))}verified.insert(id,String::from_utf8(bytes).map_err(err)?);}}
    Ok(crate::chunk::decode(&v,&mut |id|verified.get(id).cloned()))
}
fn write_doc(c:&Connection,key:&str,v:&Value,now:i64)->Result<(),StorageError>{
    if v.is_null(){return sql::put_document(c,key,None).map_err(err)}
    let (stored,chunks)=sql::encode_text(&v.to_string());
    for (id,data) in chunks { blob(c,&id,&data,now)?; }
    sql::put_document(c,key,Some(&stored)).map_err(err)
}
fn blob(c:&Connection,id:&str,data:&[u8],now:i64)->Result<(),StorageError>{
    if crate::chunk::blob_id(data)!=id {return Err(err("attachment bytes do not match their identifier"))}
    if !sql::insert_blob(c,id,data,now).map_err(err)? && sql::read_blob(c,id).map_err(err)?.as_deref()!=Some(data){return Err(err("stored blob has different bytes"))}
    Ok(())
}
fn require_files(c:&Connection,v:&Value)->Result<(),StorageError>{
    for f in v.get("files").and_then(Value::as_array).into_iter().flatten(){
        if let Some(id)=f.pointer("/blob/id").and_then(Value::as_str){
            let data=sql::read_blob(c,id).map_err(err)?.ok_or_else(||err("missing draft attachment"))?;
            if crate::chunk::blob_id(&data)!=id{return Err(err("invalid draft attachment"))}
        }
    }
    Ok(())
}

/// Runs inside the normal writer's FULL transaction. Selecting never discards the old
/// primary: put it into the slot freed by the chosen version unless already represented.
pub(super) fn pick(c:&Connection,p:&crate::recovery::Pick,now:i64)->Result<Versions,StorageError>{
    let key=&p.primary_key;let undo=undo_key(key)?;
    let (_,tab)=key.split_once(":draft:").ok_or_else(||err("invalid draft"))?;let vk=versions_key(tab);
    let mut set:Versions=serde_json::from_value(json(c,&vk)?).map_err(err)?;
    let n=set.drafts.iter().position(|v|v==&p.selected).ok_or_else(||err("recovered version has changed"))?;
    set.drafts.remove(n);
    if let crate::recovery::Action::Use{expected}=&p.action {
        let previous=Draft{document:json(c,key)?,history:json(c,&undo)?};
        if !previous.same_work(expected){return Err(err("current draft changed"))}
        require_files(c,&p.selected.document)?;
        if !previous.document.is_null() || !previous.history.is_null(){
            match set.retain(&p.selected,&previous){Decision::Store(next)=>set=next,Decision::AlreadyKept=>{},Decision::Full=>return Err(err("no room for existing draft"))}
        }
        write_doc(c,key,&p.selected.document,now)?;write_doc(c,&undo,&p.selected.history,now)?;
        super::search::index::draft(c,key,now).map_err(err)?;
    }
    write_doc(c,&vk,&if set.drafts.is_empty(){Value::Null}else{serde_json::to_value(&set).map_err(err)?},now)?;
    Ok(set)
}

pub enum Reconciled { Durable(u64), AtCapacity(String), WorkspaceChanged, UnsupportedSchema }
impl VerifiedWorkspace {
    /// One transaction for every affected draft, its exact undo/redo and blobs. A ninth
    /// independent version rolls back the *whole* batch. Retrying after a kill is a no-op
    /// for versions already present. Never substitute equality of text or file names.
    pub fn save_drafts(&mut self,batch:&MemoryDrafts,now:i64)->Result<Reconciled,StorageError>{
        // Reacquiring the original file is not enough if an update migrated that
        // very file while this editor was in memory. Check under the transaction
        // that would write the drafts, before any blobs, documents or receipts.
        let tx=self.conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(err)?;
        use super::schema::{BASE_SCHEMA, FLAG_BITS, SCHEMA_VERSION, user_version};
        if !(BASE_SCHEMA..=SCHEMA_VERSION).contains(&(user_version(&tx).map_err(err)? >> FLAG_BITS)){
            return Ok(Reconciled::UnsupportedSchema);
        }
        let (previous,mut applied)=match &self.applied {
            Applied::Fresh=>(None,BTreeMap::new()),
            Applied::Through{batch:last,..} if last.revision>=batch.revision=>return Ok(Reconciled::Durable(last.revision)),
            Applied::Through{batch:last,primary}=>(Some(last),primary.clone()),
        };
        let mut boot_changed=false;
        // A killed or forced owner's journal can hold keystrokes its document rows never got,
        // and the boot record this transaction rebuilds would stop the next owner trusting them.
        // Write them first, as that owner's next open would: this edit then compares against
        // that text and is kept as a recovered version instead of replacing it.
        for r in super::journal::owed(&tx,&self.db).map_err(err)? {
            let (stored,chunks)=sql::encode_text(&r.text);
            for (id,data) in &chunks {if !sql::insert_blob(&tx,id,data,now).map_err(err)? {sql::touch_blob(&tx,id,now).map_err(err)?;}}
            sql::put_document(&tx,&r.key,Some(&stored)).map_err(err)?;
            super::journal::put_rev(&tx,&r.key,r.seq).map_err(err)?;
            super::search::index::draft(&tx,&r.key,now).map_err(err)?;boot_changed=true;
        }
        for (id,data) in &batch.blobs {blob(&tx,id,data,now)?;}
        let mut keys=batch.draft_keys();keys.extend(applied.keys().cloned());
        for key in keys{
            let undo=undo_key(&key)?;let incoming=batch.payload(&key,false)?;
            require_files(&tx,&incoming.document)?;
            let primary=Draft{document:json(&tx,&key)?,history:json(&tx,&undo)?};
            if primary==incoming{applied.insert(key,primary);continue;}
            if primary.covers(&incoming){continue;}
            if applied.get(&key)==Some(&primary) || primary==batch.payload(&key,true)? {
                write_doc(&tx,&key,&incoming.document,now)?;write_doc(&tx,&undo,&incoming.history,now)?;
                super::search::index::draft(&tx,&key,now).map_err(err)?;boot_changed=true;
                applied.insert(key,incoming);
            }else{
                let (_,tab)=key.split_once(":draft:").ok_or_else(||err("invalid session"))?;let k=versions_key(tab);
                let v=json(&tx,&k)?;let versions:Versions=if v.is_null(){Versions::default()}else{serde_json::from_value(v).map_err(err)?};
                match versions.retain(&primary,&incoming){
                    Decision::AlreadyKept=>{},Decision::Full=>return Ok(Reconciled::AtCapacity(tab.into())),
                    Decision::Store(set)=>write_doc(&tx,&k,&serde_json::to_value(set).map_err(err)?,now)?,
                }
            }
        }
        // Never replace somebody else's newer settings with the entire held checkpoint.
        // An unrelated key added by the owner is not in this write set and is not touched.
        let state_keys:BTreeSet<_>=batch.state.keys().chain(previous.into_iter().flat_map(|b|b.state.keys())).collect();
        for pair in state_keys {
            let (ns,key)=pair;let initial=batch.initial_state.get(pair).map(AsRef::as_ref);
            let after=batch.state.get(pair).map_or(initial,Option::as_deref);
            let before=previous.and_then(|b|b.state.get(pair)).map_or(initial,Option::as_deref);
            let disk:Option<String>=tx.query_row("SELECT value FROM state WHERE name=?1 AND key=?2",[ns,key],|r|r.get(0)).optional().map_err(err)?;
            if disk.as_deref()==after{continue;}
            if disk.as_deref()!=before{return Ok(Reconciled::WorkspaceChanged)}
            sql::put_state(&tx,ns,key,after,now).map_err(err)?;boot_changed|=crate::in_boot_record(ns,key);
        }
        for (key,after) in &batch.docs {
            if crate::journal::is_draft_key(key)||key.contains(":undo:"){continue}
            let current=json(&tx,key)?;
            let decode=|s:Option<&Arc<str>>|s.map(|v|serde_json::from_str(v)).transpose().map(|v|v.unwrap_or(Value::Null)).map_err(err);
            let target=decode(after.as_ref())?;if current==target{continue}if current!=decode(batch.base.get(key))?{return Ok(Reconciled::WorkspaceChanged)}
            write_doc(&tx,key,&target,now)?;
        }
        for (id,after) in &batch.frames {
            let current=tx.query_row("SELECT x,y,width,height,maximized,full_screen,display_x,display_y,display_w,display_h FROM window_state WHERE id=?1",[id],|r|{
                let display=match (r.get(6)?,r.get(7)?,r.get(8)?,r.get(9)?){(Some(x),Some(y),Some(w),Some(h))=>Some([x,y,w,h]),_=>None};
                Ok(crate::WindowFrame{x:r.get(0)?,y:r.get(1)?,width:r.get(2)?,height:r.get(3)?,maximized:r.get(4)?,full_screen:r.get(5)?,display})
            }).optional().map_err(err)?;
            if current==*after{continue}if current.as_ref()!=batch.initial_frames.get(id){return Ok(Reconciled::WorkspaceChanged)}
            sql::put_window(&tx,id,after.as_ref(),now).map_err(err)?;boot_changed=true;
        }
        for (id,record) in &batch.plugins {
            let enabled=tx.query_row("SELECT enabled FROM plugin WHERE id=?1",[id],|r|r.get(0)).optional().map_err(err)?;
            let trusted=tx.query_row("SELECT sha256 FROM plugin_trust WHERE id=?1",[id],|r|r.get(0)).optional().map_err(err)?;
            let current=crate::PluginRecord{enabled,trusted};if current==*record{continue;}
            if Some(&current)!=batch.initial_plugins.get(id) && !(current==Default::default()&&!batch.initial_plugins.contains_key(id)){return Ok(Reconciled::WorkspaceChanged)}
            crate::plugins::native::write(&tx,id,record,now).map_err(err)?;boot_changed=true;
        }
        if boot_changed{super::boot_record::rebuild(&tx).map_err(err)?;}
        tx.commit().map_err(err)?;
        self.applied=Applied::Through{batch:batch.clone(),primary:applied};
        Ok(Reconciled::Durable(batch.revision))
    }
}
