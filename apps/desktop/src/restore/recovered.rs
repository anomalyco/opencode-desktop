//! A session's recovered versions have one notice. Preview is view state only; it never
//! calls storage or touches the primary editor. Explicit Use/Discard identify a version
//! by its full payload, not by a list index that another window might have reused.
use std::sync::{Arc,Mutex};
use opencode_i18n::t;
use opencode_storage::{Lookup, recovery::{Action,Draft,Pick,Versions,versions_key}};
use rune::{Tx,col,el,row,text};
use crate::{App,Route};
use crate::palette::Pal;
use serde_json::Value;

#[derive(Default)]
pub(super) struct Inbox { state:State, events:Arc<Mutex<Vec<Event>>> }
#[derive(Default)]
enum State { #[default] Idle, Loading(String), Empty(String), Notice(String,Arc<Versions>,Page), Applying(String) }
#[derive(Clone,Copy)]
enum Page { Summary, Preview(usize) }
enum Event { Loaded(String,Versions), Picked(String,Versions,Option<(Draft,Draft)>), Failed(String,String) }

impl App {
    pub(crate) fn poll_recovered(&mut self)->bool {
        if !self.persists()||!crate::storage::is_ready(){return false;}
        let tab=match &*self.route {Route::Session{server,id}=>opencode_kit::servers::TabKey::session(server.clone(),id).encode(),_=>{self.restore.recovery.state=State::Idle;return false}};
        let events=std::mem::take(&mut *self.restore.recovery.events.lock().unwrap_or_else(|e|e.into_inner()));
        let mut changed=false;
        for ev in events {
            let (key,set,install)=match ev {
                Event::Loaded(k,set) if matches!(&self.restore.recovery.state,State::Loading(active) if *active==k)=>(k,set,None),
                Event::Picked(k,set,pick)=>(k,set,pick),
                Event::Failed(k,why) if k==tab=>{crate::status_toast::fail(self,t!("rune.storage.notSaving.title"),why);self.restore.recovery.state=State::Idle;changed=true;continue},_=>continue,
            };
            if key!=tab {continue;}
            if let Some((draft,expected))=install && self.live_recovery_draft(&tab).is_some_and(|now| now.same_work(&expected)) {
                self.install_recovered(draft);
            }
            self.restore.recovery.state=if set.drafts.is_empty(){State::Empty(key)}else{State::Notice(key,Arc::new(set),Page::Summary)};
            changed=true;
        }
        if matches!(&self.restore.recovery.state,State::Loading(k)|State::Empty(k)|State::Notice(k,..)|State::Applying(k) if *k==tab){return changed;}
        let key=versions_key(&tab);
        if matches!(crate::storage::doc(&key),Lookup::Ready(None)){self.restore.recovery.state=State::Empty(tab);return changed;}
        self.restore.recovery.state=State::Loading(tab.clone());
        let events=self.restore.recovery.events.clone();let wake=(*self.waker).clone();
        crate::storage::with_storage(move |db|{let s=db.clone();let load=key.clone();db.documents().load(&[&load],Some(Box::new(move ||{
            let ev=match s.documents().get(&key){Lookup::Ready(Some(v))=>match serde_json::from_str(&v){Ok(set)=>Event::Loaded(tab,set),Err(e)=>Event::Failed(tab,e.to_string())},_=>Event::Loaded(tab,Versions::default())};
            events.lock().unwrap_or_else(|e|e.into_inner()).push(ev);if let Some(w)=wake{w();}
        })));});
        true
    }

    fn install_recovered(&mut self,draft:Draft){
        let Some(k)=self.restore.shown.clone() else{return};
        let decoded=super::draft_doc::decode(&draft.document);
        let _=self.take_composer_draft();self.restore.edits.remove(&k);
        self.restore.drafts.insert(k.clone(),decoded.text); self.restore.parked.insert(k.clone(),decoded.parked);
        self.put_composer_draft(Some(&k));self.replace_draft_files(&k,decoded.stored);
        self.add.mode=if decoded.shell{session_view::composer::mode::Mode::Shell}else{session_view::composer::mode::Mode::Normal};
        if !draft.history.is_null(){self.composer.restore_history_json(&draft.history);}
        self.adopt_recovered_history(&k);
        self.restore.stamp=None;
    }

    /// A focus, redraw or caret movement is not an edit. But a new image without
    /// a storage ID is work too; never compare the incomplete document as equal.
    fn live_recovery_draft(&self,tab:&str)->Option<Draft>{
        if self.attach.files.iter().any(|f|f.media.is_none() && !self.restore.blobs.knows(&f.bytes)){return None;}
        let document=self.draft_documents().get(tab).map(|v|serde_json::from_str(v)).transpose().ok()?.unwrap_or(Value::Null);
        Some(Draft{document,history:self.composer.history_json(32,64*1024).unwrap_or(Value::Null)})
    }

    fn pick_version(&mut self,index:usize,use_it:bool){
        let State::Notice(tab,set,_) =&self.restore.recovery.state else{return};
        let Some(selected)=set.drafts.get(index).cloned() else{return};let tab=tab.clone();
        let key=opencode_storage::draft_document(&self.window.id,&tab);
        let (action,pick)=if use_it{
            // Compare the whole current document, never just its text. If it is not yet
            // saved or a different version landed, the writer leaves both versions alone.
            let Some(expected)=self.live_recovery_draft(&tab) else{return};
            (Action::Use{expected:expected.clone()},Some((selected.clone(),expected)))
        }else{(Action::Discard,None)};
        self.restore.recovery.state=State::Applying(tab.clone());let events=self.restore.recovery.events.clone();let wake=(*self.waker).clone();
        crate::storage::with_storage(move |s|s.pick_recovery(Pick{primary_key:key,selected,action},Box::new(move |r|{
            let ev=match r{Ok(set)=>Event::Picked(tab,set,pick),Err(e)=>Event::Failed(tab,e.to_string())};
            events.lock().unwrap_or_else(|e|e.into_inner()).push(ev);if let Some(w)=wake{w();}
        })));
    }

    pub(crate) fn recovered_notice(&self,p:&Pal)->Option<rune::El>{
        let State::Notice(_,set,page)=&self.restore.recovery.state else{return None};
        let count=set.drafts.len();let title=if count==1{t!("rune.storage.recovered.one").to_owned()}else{t!("rune.storage.recovered.many",count=count.to_string())};
        let mut notice=col().key("recovered-drafts").w_full().px(12.).py(8.).gap(4.).fill(p.layer1).child(text(title).color(p.text).wrap().w_full());
        for (i,draft) in set.drafts.iter().enumerate(){
            let mut actions=row().gap(8.);
            for (key,label,act) in [("preview",t!("rune.storage.recovered.preview"),0),("use",t!("rune.storage.recovered.use"),1),("discard",t!("rune.storage.recovered.discard"),2)]{
                let shown=if count==1{label.to_owned()}else{format!("{label} {}",i+1)};
                actions=actions.child(el().key(format!("recovered-{key}-{i}")).role("button").label(shown.clone()).focusable(true).px(6.).py(3.).hover(p.layer2).child(text(shown).color(p.text)).on_press_tx(move |s:&mut App,tx:&mut Tx|{
                    if act==0{if let State::Notice(_,_,page)=&mut s.restore.recovery.state {*page=match page{Page::Preview(n) if *n==i=>Page::Summary,_=>Page::Preview(i)};}}else{s.pick_version(i,act==1)}tx.instant();
                }));
            }
            notice=notice.child(actions);
            if matches!(page,Page::Preview(n) if *n==i){
                notice=notice.child(text(draft.words().to_owned()).color(p.text).wrap().w_full());
                for f in draft.document["files"].as_array().into_iter().flatten(){if let Some(name)=f["name"].as_str(){notice=notice.child(text(name.to_owned()).color(p.text));}}
            }
        }
        Some(notice)
    }
}
