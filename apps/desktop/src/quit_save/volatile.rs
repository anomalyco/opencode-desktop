//! A memory session has no failing disk job to keep polling. Check on an explicit exit
//! choice, or on activity in a running workspace; keep its original identity through exit.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use opencode_i18n::t;
use opencode_storage::{Storage,native::recovery::{MemoryDrafts,VerifiedWorkspace,Reconciled}};
use crate::storage::VolatileOrigin;
use super::{Event,Save,send};

#[derive(Clone,PartialEq,Eq)]
pub(super) struct Risk { pub revision:u64, pub reason:String, pub drafts:String }
#[derive(Clone)]
pub(super) enum Choice { Quit, Discard(Arc<Risk>) }
pub(super) enum Outcome { Empty(u64), Saved(u64), AtRisk(Risk) }

static LABELS:Mutex<Option<HashMap<String,String>>>=Mutex::new(None);
static OWNER:Mutex<Option<VerifiedWorkspace>>=Mutex::new(None);

/// No timer polls a permanently held workspace. A new editor change or native focus gain
/// may make progress; a failed attempt only waits for another such event or an exit choice.
enum Live { Idle, Working, Again }
static LIVE:Mutex<Live>=Mutex::new(Live::Idle);
pub(crate) fn activity() {
    if crate::storage::volatile_origin().is_none(){return;}
    let Some(db)=crate::storage::live_volatile_store() else{return};
    let mut lane=LIVE.lock().unwrap_or_else(|e|e.into_inner());
    if !matches!(*lane,Live::Idle){*lane=Live::Again;return;}
    *lane=Live::Working;drop(lane);
    if std::thread::Builder::new().name("reconcile-memory-workspace".into()).spawn(move || loop {
        if let Some(batch)=db.memory_drafts(0) && batch.has_user_changes() { let _=reconcile(&batch); }
        let mut lane=LIVE.lock().unwrap_or_else(|e|e.into_inner());
        if matches!(*lane,Live::Again){*lane=Live::Working;}else{*lane=Live::Idle;return;}
    }).is_err(){*LIVE.lock().unwrap_or_else(|e|e.into_inner())=Live::Idle;}
}

pub(super) fn name_drafts(a:&crate::App) {
    let mut labels=LABELS.lock().unwrap_or_else(|e|e.into_inner());let labels=labels.get_or_insert_with(HashMap::new);
    for (i,tab) in a.tabs.iter().enumerate(){
        let title=a.session(&tab.id).map(|s|s.title.as_str()).filter(|s|!s.is_empty()).unwrap_or(&tab.title);
        let label=if !title.is_empty(){crate::tabs::tab_title(title)}else{format!("{} {}",t!("session.tab.session"),i+1)};
        labels.insert(opencode_storage::draft_document(&a.window.id,&tab.key().encode()),label);
    }
}

fn loss(b:&MemoryDrafts,reason:String)->Risk {
    let labels=LABELS.lock().unwrap_or_else(|e|e.into_inner());
    let mut names:Vec<_>=b.draft_keys().iter().map(|k|labels.as_ref().and_then(|l|l.get(k)).cloned().unwrap_or_else(||t!("rune.session.untitled").into())).collect();
    names.sort();let drafts=if names.is_empty(){t!("rune.storage.quit.workspaceEdits").to_owned()}else{t!("rune.storage.quit.drafts",drafts=names.join(", "))};
    Risk{revision:b.revision,reason,drafts}
}

fn reopen_reason()->String {
    match crate::storage::volatile_origin(){
        Some(VolatileOrigin::Held{shown,..}) if shown.source==opencode_storage::BootSource::Checkpoint => t!("rune.storage.checkpoint.description").to_owned(),
        Some(VolatileOrigin::Held{..})=>t!("rune.storage.firstRun.description").to_owned(),
        Some(VolatileOrigin::Unavailable{why,kept:Some(p),..})=>format!("{} {}",t!("rune.storage.quit.quarantined",path=p.display().to_string()),t!("rune.storage.memoryOnly.description",why=why.clone())),
        Some(VolatileOrigin::Unavailable{why,..})=>t!("rune.storage.memoryOnly.description",why=why.clone()),
        _=>t!("rune.storage.memoryOnly.title").to_owned(),
    }
}

fn reconcile(batch:&MemoryDrafts)->Outcome {
    let mut owner=OWNER.lock().unwrap_or_else(|e|e.into_inner());
    if owner.is_none(){
        let original=match crate::storage::volatile_origin(){Some(VolatileOrigin::Held{original,..})|Some(VolatileOrigin::Unavailable{original,kept:None,..})=>original.as_ref(),_=>None};
        let Some((file,dir))=original.zip(crate::dirs::data()) else{return Outcome::AtRisk(loss(batch,reopen_reason()))};
        match file.reacquire(&dir.join("opencode.sqlite")){
            Ok(store)=>*owner=Some(store),
            Err(e)=>return Outcome::AtRisk(loss(batch,if e.0=="the saved workspace was replaced"{t!("rune.storage.quit.replaced").into()}else{reopen_reason()})),
        }
    }
    match owner.as_mut().unwrap().save_drafts(batch,crate::wall_ms()){
        Ok(Reconciled::Durable(r))=>Outcome::Saved(r),
        Ok(Reconciled::AtCapacity(_))=>Outcome::AtRisk(loss(batch,t!("rune.storage.quit.limit").into())),
        Ok(Reconciled::WorkspaceChanged)=>Outcome::AtRisk(loss(batch,t!("rune.storage.quit.replaced").into())),
        Ok(Reconciled::UnsupportedSchema)=>Outcome::AtRisk(loss(batch,t!("rune.storage.quit.schema").into())),
        Err(e)=>Outcome::AtRisk(loss(batch,t!("rune.storage.writeFailed.description",why=e.0))),
    }
}

pub(super) fn check(s:Save,db:Storage){
    status_refresh(s);
    if std::thread::Builder::new().name("check-memory-workspace".into()).spawn(move || {
        let Some(batch)=db.memory_drafts(s.epoch) else{return};
        let result=if batch.has_user_changes(){reconcile(&batch)}else{Outcome::Empty(batch.revision)};
        send(Event::MemoryResult(s,result));
    }).is_err(){ s.ticket.failed(); }
}
fn status_refresh(s:Save){super::status::refresh(s.ticket);}
