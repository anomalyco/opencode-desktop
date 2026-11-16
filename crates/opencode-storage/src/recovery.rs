//! A conflicting draft stays beside the primary, never in its place. The same transition
//! is used for a running memory-only editor and for a pending quit. Only the writer may
//! publish the resulting set, in the transaction that stores the attachment bytes.
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const LIMIT: usize = 8;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Draft {
    pub document: Value,
    pub history: Value,
}

impl Draft {
    fn normalized(&self) -> (Value, &Value) {
        let mut doc = self.document.clone();
        if let Some(fields) = doc.as_object_mut() {
            fields.remove("caret");
            if fields.len()==1 && let Some(Value::String(s))=fields.get("text") { doc=Value::String(s.clone()); }
        }
        (doc,&self.history)
    }

    pub fn words(&self) -> &str {
        self.document.as_str().or_else(|| self.document.get("text").and_then(Value::as_str)).unwrap_or("")
    }

    pub fn same_work(&self, other:&Self) -> bool { self.normalized()==other.normalized() }

    /// `newer` must let Undo reconstruct the old text, its history and rich parts. A
    /// substring is not ancestry. Ambiguous, evicted, coalesced or future formats keep both.
    pub fn covers(&self, older:&Self) -> bool {
        let (mut new,nh)=self.normalized(); let (mut old,oh)=older.normalized();
        if new==old && nh==oh { return true; }
        fn parts(v:&mut Value) { if let Some(m)=v.as_object_mut(){m.remove("text");}else if v.is_string(){*v=Value::Object(Default::default());} }
        parts(&mut new); parts(&mut old);
        if new!=old || nh["v"].as_u64()!=Some(2) || oh["v"].as_u64()!=Some(2)
            || nh["d"].as_str()!=Some(self.words()) || oh["d"].as_str()!=Some(older.words()) {return false;}
        let (Some(n),Some(o),Some(old_redo))=(nh["undo"].as_array(),oh["undo"].as_array(),oh["redo"].as_array()) else{return false};
        if !old_redo.is_empty() || n.len()<=o.len() || !n.starts_with(o) {return false;}
        let mut text=self.words().to_owned();
        for c in n[o.len()..].iter().rev() {
            let (Some(at),Some(i),Some(r))=(c["at"].as_u64().and_then(|x|usize::try_from(x).ok()),c["i"].as_str(),c["r"].as_str()) else{return false};
            let Some(end)=at.checked_add(i.len()) else{return false};
            if c["al"].as_u64()!=Some(text.len() as u64) || text.get(at..end)!=Some(i) {return false;}
            text.replace_range(at..end,r);
            if c["bl"].as_u64()!=Some(text.len() as u64) {return false;}
        }
        text==older.words()
    }
}

/// Each session has one notice, regardless of how many independently authored versions it holds.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Versions { pub drafts: Vec<Draft> }
pub enum Decision { AlreadyKept, Store(Versions), Full }

impl Versions {
    pub fn retain(&self, primary:&Draft, incoming:&Draft) -> Decision {
        if primary.covers(incoming) || self.drafts.iter().any(|v|v.covers(incoming)) {return Decision::AlreadyKept;}
        let mut next:Vec<_>=self.drafts.iter().filter(|v|!incoming.covers(v)).cloned().collect();
        if next.len()>=LIMIT { return Decision::Full; }
        next.push(incoming.clone());
        Decision::Store(Self{drafts:next})
    }
}

pub fn versions_key(tab:&str) -> String {format!("rune.recovered.v1:{tab}")}

#[cfg(windows)]
pub struct Pick {
    pub primary_key:String,
    pub selected:Draft,
    pub action:Action,
}
#[cfg(windows)]
pub enum Action { Discard, Use { expected: Draft } }
#[cfg(windows)]
pub type PickReply=Box<dyn FnOnce(Result<Versions,crate::StorageError>)+Send>;
