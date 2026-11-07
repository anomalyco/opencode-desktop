//! A session's undelivered prompts: queued or steering, as the server delivers them.

use super::*;

/// How the server delivers a prompt sent while the session is busy: `Steer` joins the running
/// turn at its next step boundary; `Queue` waits for the turn (and every steer) to finish. The
/// server steers when a prompt names neither.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Delivery {
    #[default]
    Steer,
    Queue,
}

impl Delivery {
    pub fn as_str(self) -> &'static str {
        match self {
            Delivery::Steer => "steer",
            Delivery::Queue => "queue",
        }
    }

    pub fn parse(v: &Value) -> Option<Delivery> {
        match v.as_str()? {
            "steer" => Some(Delivery::Steer),
            "queue" => Some(Delivery::Queue),
            _ => None,
        }
    }
}

/// A prompt the server admitted but hasn't delivered yet (`GET /api/session/:id/inbox`). Its id
/// becomes the user message's id once delivered.
#[derive(Clone, Debug, PartialEq)]
pub struct InboxItem {
    pub id: String,
    pub text: String,
    pub delivery: Delivery,
    pub files: Vec<Attachment>,
}

/// One user item, from the inbox list (`{id, type, payload, delivery}`) or a
/// `session.inbox.enqueued` event (`{inboxID, item: {type, payload, delivery}}`). Synthetic,
/// compaction and move items are None.
pub fn parse_inbox_item(v: &Value) -> Option<InboxItem> {
    let (id, item) = if v["item"].is_object() { (&v["inboxID"], &v["item"]) } else { (&v["id"], v) };
    if item["type"] != "user" {
        return None;
    }
    let payload = &item["payload"];
    let text = payload["metadata"]["displayText"].as_str().filter(|t| !t.is_empty()).map_or_else(|| s(&payload["text"]), str::to_owned);
    if let Some((_, quotes)) = crate::quote::parse_metadata(&payload["metadata"]) {
        set_user_quotes(&s(id), quotes);
    }
    Some(InboxItem { id: s(id), text, delivery: Delivery::parse(&item["delivery"])?, files: parse_attachments(&payload["files"]) })
}

/// What [`Inbox::apply`] did.
#[derive(Debug, PartialEq)]
pub enum InboxChange {
    None,
    Changed,
    /// The item left the inbox for the transcript.
    Delivered(InboxItem),
}

/// One session's undelivered prompts, in admission order, kept current from
/// `session.inbox.*` events.
#[derive(Clone, Debug, Default)]
pub struct Inbox {
    pub items: Vec<InboxItem>,
    /// The id of a compaction the server admitted but hasn't started.
    pub compaction: Option<String>,
}

impl Inbox {
    pub fn queued(&self) -> impl Iterator<Item = &InboxItem> {
        self.items.iter().filter(|i| i.delivery == Delivery::Queue)
    }

    pub fn steering(&self) -> impl Iterator<Item = &InboxItem> {
        self.items.iter().filter(|i| i.delivery == Delivery::Steer)
    }

    /// Adds `item`, or replaces the one with its id in place.
    pub fn upsert(&mut self, item: InboxItem) {
        match self.items.iter_mut().find(|i| i.id == item.id) {
            Some(slot) => *slot = item,
            None => self.items.push(item),
        }
    }

    pub fn remove(&mut self, id: &str) -> Option<InboxItem> {
        let at = self.items.iter().position(|i| i.id == id)?;
        Some(self.items.remove(at))
    }

    pub fn set_delivery(&mut self, id: &str, delivery: Delivery) -> bool {
        self.items.iter_mut().find(|i| i.id == id).is_some_and(|i| std::mem::replace(&mut i.delivery, delivery) != delivery)
    }

    /// Applies one `session.inbox.*` event, or `session.inbox.listed` (see
    /// [`crate::client::Request::Inbox`]), which replaces the list.
    pub fn apply(&mut self, ty: &str, d: &Value) -> InboxChange {
        let id = d["inboxID"].as_str().unwrap_or("");
        let compaction = match ty {
            "session.inbox.listed" => {
                d["items"].as_array().into_iter().flatten().find(|i| i["type"] == "compaction").map(|i| s(&i["id"]))
            }
            "session.inbox.enqueued" if d["item"]["type"] == "compaction" => Some(id.to_owned()),
            "session.inbox.cancelled" | "session.inbox.delivered" if self.compaction.as_deref() == Some(id) => None,
            _ => self.compaction.clone(),
        };
        let moved = compaction != self.compaction;
        self.compaction = compaction;
        let changed = match ty {
            "session.inbox.listed" => {
                let items: Vec<InboxItem> = d["items"].as_array().into_iter().flatten().filter_map(parse_inbox_item).collect();
                let same = items == self.items;
                self.items = items;
                !same
            }
            "session.inbox.enqueued" => match parse_inbox_item(d) {
                Some(item) => {
                    let same = self.items.iter().any(|i| *i == item);
                    self.upsert(item);
                    !same
                }
                None => false,
            },
            "session.inbox.delivery.changed" => Delivery::parse(&d["delivery"]).is_some_and(|to| self.set_delivery(id, to)),
            "session.inbox.cancelled" => self.remove(id).is_some(),
            "session.inbox.delivered" => match self.remove(id) {
                Some(item) => return InboxChange::Delivered(item),
                None => false,
            },
            _ => false,
        } || moved;
        if changed { InboxChange::Changed } else { InboxChange::None }
    }
}
