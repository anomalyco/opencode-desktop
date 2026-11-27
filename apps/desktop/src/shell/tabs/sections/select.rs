//! Multi-select on side tabs: ⌘-click (Ctrl-click off macOS) toggles a tab, ⇧-click takes the
//! run from the anchor to it, a plain click or Esc clears. The selection is its own machine so a
//! pick and its anchor can't drift apart.

/// The tabs picked (`Tab::uid`), in the order they were picked, and the tab a ⇧-click runs from.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Selection {
    #[default]
    Empty,
    Picked { uids: Vec<String>, anchor: String },
}

pub enum Event<'a> {
    /// ⌘-click on `uid`.
    Toggle(&'a str),
    /// ⇧-click on `to`: the run of `order` (the tabs as listed) from the anchor, or from `from`
    /// (the open tab) with none.
    Range { to: &'a str, order: &'a [String], from: Option<&'a str> },
    /// A plain click, Esc, or the picked tabs went somewhere.
    Clear,
    /// The tabs still pickable (open and shown): the rest leave the selection.
    Retain(&'a [String]),
}

impl Selection {
    /// Runs `event`; whether the selection changed (the caller redraws).
    pub fn step(&mut self, event: Event<'_>) -> bool {
        let before = self.clone();
        *self = match (std::mem::take(self), event) {
            (Selection::Empty, Event::Toggle(uid)) => Selection::Picked { uids: vec![uid.to_owned()], anchor: uid.to_owned() },
            (Selection::Picked { mut uids, .. }, Event::Toggle(uid)) => {
                match uids.iter().position(|u| u == uid) {
                    Some(i) => _ = uids.remove(i),
                    None => uids.push(uid.to_owned()),
                }
                if uids.is_empty() { Selection::Empty } else { Selection::Picked { uids, anchor: uid.to_owned() } }
            }
            (was, Event::Range { to, order, from }) => {
                let anchor = match &was {
                    Selection::Picked { anchor, .. } if order.contains(anchor) => anchor.clone(),
                    _ => from.filter(|f| order.iter().any(|o| o == f)).unwrap_or(to).to_owned(),
                };
                match (order.iter().position(|u| *u == anchor), order.iter().position(|u| u == to)) {
                    (Some(a), Some(b)) => Selection::Picked { uids: order[a.min(b)..=a.max(b)].to_vec(), anchor },
                    _ => was,
                }
            }
            (_, Event::Clear) => Selection::Empty,
            (Selection::Picked { mut uids, anchor }, Event::Retain(keep)) => {
                uids.retain(|u| keep.contains(u));
                match uids.first().cloned() {
                    None => Selection::Empty,
                    Some(first) => Selection::Picked { anchor: if uids.contains(&anchor) { anchor } else { first }, uids },
                }
            }
            (Selection::Empty, Event::Retain(_)) => Selection::Empty,
        };
        *self != before
    }

    pub fn uids(&self) -> &[String] {
        match self {
            Selection::Empty => &[],
            Selection::Picked { uids, .. } => uids,
        }
    }

    pub fn contains(&self, uid: &str) -> bool {
        self.uids().iter().any(|u| u == uid)
    }

    pub fn len(&self) -> usize {
        self.uids().len()
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, Selection::Empty)
    }
}
