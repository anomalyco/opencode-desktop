//! Keep late results from taking a pick out from under the pointer. Record only the identities
//! and sections from the last answer, never old actions, labels, or session records.

use std::collections::HashMap;
use crate::popover::Highlight;
use super::Entry;

#[derive(Default)]
pub(super) enum Order {
    #[default]
    Fresh,
    Showing { query: String, keys: Vec<(String, &'static str)> },
}

impl Order {
    pub(super) fn present(&mut self, q: &str, mut rows: Vec<Entry>, active: &Highlight) -> Vec<Entry> {
        if let Self::Showing { query, keys } = self && query == q && !q.is_empty() {
            if keys.iter().map(|(k,g)| (k.as_str(), *g)).eq(rows.iter().map(|r| (r.key.as_str(), r.group))) {
                return rows;
            }
            let mut groups = Vec::new();
            let mut old = HashMap::with_capacity(keys.len());
            for (n, (key, group)) in keys.iter().enumerate() {
                if groups.last() != Some(group) { groups.push(*group); }
                old.insert(key.as_str(), n);
            }
            let group_no = |group: &str| groups.iter().position(|g| *g == group).unwrap_or(usize::MAX);
            let anchor = match active {
                Highlight::Chosen { key, .. } if rows.iter().any(|e| &e.key == key) => old.get(key.as_str()).map(|&i| group_no(keys[i].1)),
                _ => None,
            };
            // Additions to sections above a chosen row wait. Others follow the survivors in
            // their section. New sections follow in source order; deleted rows are never kept.
            rows.retain(|r| old.contains_key(r.key.as_str()) || anchor.is_none_or(|g| group_no(r.group) >= g));
            rows.sort_by_key(|r| (group_no(r.group), old.get(r.key.as_str()).copied().unwrap_or(usize::MAX)));
        }
        *self = Self::Showing { query: q.to_owned(), keys: rows.iter().map(|r| (r.key.clone(), r.group)).collect() };
        rows
    }
}
