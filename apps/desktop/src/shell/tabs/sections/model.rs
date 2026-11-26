//! Experimental › Sidebar sections' saved shape (`sidebar.sections`, one row per window): named,
//! collapsible sections of side tabs, each tab in at most one. Tabs in no section stay below them
//! in the window's tab order.

use serde_json::{Value, json};

/// A section: its name, whether it's folded, and its tabs (`Tab::uid`) in the order it shows them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Section {
    pub id: String,
    pub name: String,
    pub collapsed: bool,
    pub members: Vec<String>,
}

/// Every section of a window, top to bottom.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Doc {
    pub sections: Vec<Section>,
}

impl Doc {
    pub fn from_json(v: &Value) -> Doc {
        let sections = v["sections"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|s| {
                        let id = s["id"].as_str()?.to_owned();
                        let members: Vec<String> = s["tabs"].as_array()?.iter().filter_map(|t| t.as_str().map(str::to_owned)).collect();
                        let name = s["name"].as_str().unwrap_or_default().to_owned();
                        Some(Section { id, name, collapsed: s["collapsed"].as_bool().unwrap_or(false), members })
                    })
                    .filter(|s| !s.members.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let mut doc = Doc { sections };
        doc.dedupe();
        doc
    }

    pub fn to_json(&self) -> Value {
        let sections: Vec<Value> = self.sections.iter().map(|s| json!({ "id": s.id, "name": s.name, "collapsed": s.collapsed, "tabs": s.members })).collect();
        json!({ "sections": sections })
    }

    pub fn get(&self, id: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Section> {
        self.sections.iter_mut().find(|s| s.id == id)
    }

    /// The section tab `uid` is in.
    pub fn section_of(&self, uid: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.members.iter().any(|m| m == uid))
    }

    /// Takes `uids` out of every section; a section left empty goes.
    pub fn pull(&mut self, uids: &[String]) {
        for s in &mut self.sections {
            s.members.retain(|m| !uids.contains(m));
        }
        self.sections.retain(|s| !s.members.is_empty());
    }

    /// Keeps only the tabs still open; whether anything went.
    pub fn retain_open(&mut self, open: &[String]) -> bool {
        let before = self.clone();
        for s in &mut self.sections {
            s.members.retain(|m| open.contains(m));
        }
        self.sections.retain(|s| !s.members.is_empty());
        *self != before
    }

    /// `name`, or with " 2", " 3"… after it while another section has it (case aside).
    pub fn unique_name(&self, name: &str, own: &str) -> String {
        let taken = |n: &str| self.sections.iter().any(|s| s.id != own && s.name.eq_ignore_ascii_case(n));
        if !taken(name) {
            return name.to_owned();
        }
        (2..).map(|i| format!("{name} {i}")).find(|n| !taken(n)).unwrap_or_else(|| name.to_owned())
    }

    /// A file edited by hand could list a tab twice: it stays in its first section.
    fn dedupe(&mut self) {
        let mut seen: Vec<String> = Vec::new();
        for s in &mut self.sections {
            s.members.retain(|m| {
                let fresh = !seen.contains(m);
                seen.push(m.clone());
                fresh
            });
        }
        self.sections.retain(|s| !s.members.is_empty());
    }
}

/// Where a dragged tab lands.
#[derive(Clone, Debug, PartialEq)]
pub enum Drop {
    /// At the end of a section, over its header.
    Into(String),
    /// Just before or after a tab, in whatever holds it.
    Before(String),
    After(String),
    /// Ungrouped, at the end.
    End,
}

/// The tabs as the sidebar lists them: each section's (indices into the window's tabs), then the
/// ungrouped ones in tab order.
pub struct Laid {
    pub sections: Vec<(Section, Vec<usize>)>,
    pub ungrouped: Vec<usize>,
}

impl Laid {
    /// The tabs a reader sees, in order: a folded section's are hidden.
    pub fn visible(&self) -> impl Iterator<Item = usize> + '_ {
        self.sections.iter().filter(|(s, _)| !s.collapsed).flat_map(|(_, t)| t.iter().copied()).chain(self.ungrouped.iter().copied())
    }
}

/// Lays `shown` (the tabs the sidebar lists: their index and uid) out by `doc`.
pub fn lay(doc: &Doc, shown: &[(usize, String)]) -> Laid {
    let at = |uid: &str| shown.iter().find(|(_, u)| u == uid).map(|(i, _)| *i);
    let sections: Vec<(Section, Vec<usize>)> = doc
        .sections
        .iter()
        .map(|s| (s.clone(), s.members.iter().filter_map(|m| at(m)).collect::<Vec<usize>>()))
        .filter(|(_, t)| !t.is_empty())
        .collect();
    let ungrouped = shown.iter().filter(|(_, u)| doc.section_of(u).is_none()).map(|(i, _)| *i).collect();
    Laid { sections, ungrouped }
}

/// What a drop does to the doc: `uids` leave where they were and land at `drop`. Landing among
/// ungrouped tabs also asks for the window's tabs in a new order (`Some`), from `order` (every
/// tab's uid in tab order).
pub fn land(doc: &mut Doc, uids: &[String], drop: &Drop, order: &[String]) -> Option<Vec<String>> {
    let target = match drop {
        Drop::Into(id) => Some((id.clone(), None)),
        Drop::Before(u) | Drop::After(u) => doc.section_of(u).map(|s| (s.id.clone(), Some(u.clone()))),
        Drop::End => None,
    };
    for s in &mut doc.sections {
        s.members.retain(|m| !uids.contains(m));
    }
    let tabs = match target {
        Some((id, near)) => {
            if let Some(s) = doc.get_mut(&id) {
                let at = near.and_then(|n| s.members.iter().position(|m| *m == n)).map(|i| if matches!(drop, Drop::After(_)) { i + 1 } else { i });
                let at = at.unwrap_or(s.members.len());
                for (k, u) in uids.iter().enumerate() {
                    s.members.insert(at + k, u.clone());
                }
            }
            None
        }
        None => {
            let mut rest: Vec<String> = order.iter().filter(|u| !uids.contains(u)).cloned().collect();
            let at = match drop {
                Drop::Before(u) => rest.iter().position(|x| x == u).unwrap_or(rest.len()),
                Drop::After(u) => rest.iter().position(|x| x == u).map_or(rest.len(), |i| i + 1),
                _ => rest.len(),
            };
            for (k, u) in uids.iter().enumerate() {
                rest.insert(at + k, u.clone());
            }
            Some(rest)
        }
    };
    // Sections a drag emptied go.
    doc.sections.retain(|s| !s.members.is_empty());
    tabs
}
