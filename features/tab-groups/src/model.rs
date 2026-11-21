//! Side tab groups saved state and computed sections
//! (`packages/app/src/shell/titlebar/tab-groups.ts`, #64407).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::{Map, Value, json};

/// Automatic grouping mode (`TAB_GROUP_MODES`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    None,
    #[default]
    Server,
    Project,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::None, Mode::Server, Mode::Project];

    pub fn name(self) -> &'static str {
        match self {
            Mode::None => "none",
            Mode::Server => "server",
            Mode::Project => "project",
        }
    }

    /// Unknown saved mode reads as `None` (`Schema.catchDecoding(() => "none")`).
    pub fn parse(v: Option<&str>) -> Mode {
        Mode::ALL.into_iter().find(|m| Some(m.name()) == v).unwrap_or(Mode::None)
    }
}

/// Group color palette (`TAB_GROUP_COLORS`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Color {
    Blue,
    Green,
    Orange,
    Purple,
    Pink,
    Cyan,
    Gray,
}

pub const AUTO_COLORS: [Color; 6] = [Color::Blue, Color::Green, Color::Orange, Color::Purple, Color::Cyan, Color::Pink];
const ALL_COLORS: [Color; 7] = [Color::Blue, Color::Green, Color::Orange, Color::Purple, Color::Pink, Color::Cyan, Color::Gray];

impl Color {
    pub fn name(self) -> &'static str {
        match self {
            Color::Blue => "blue",
            Color::Green => "green",
            Color::Orange => "orange",
            Color::Purple => "purple",
            Color::Pink => "pink",
            Color::Cyan => "cyan",
            Color::Gray => "gray",
        }
    }

    pub fn parse(v: Option<&str>) -> Option<Color> {
        ALL_COLORS.into_iter().find(|c| Some(c.name()) == v)
    }
}

/// User-created manual group (`ManualTabGroupSchema`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManualGroup {
    pub id: String,
    pub name: String,
    pub color: Color,
}

/// Name/color override on an automatic server or project group (`TabGroupOverrideSchema`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Override {
    pub name: Option<String>,
    pub color: Option<Color>,
}

/// Window `tabs.groups` document (`TabGroupsStateSchema`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Doc {
    pub mode: Mode,
    pub group_order: Vec<String>,
    pub collapsed: BTreeMap<String, bool>,
    pub overrides: BTreeMap<String, Override>,
    pub manual_groups: Vec<ManualGroup>,
    pub manual_assignments: BTreeMap<String, String>,
    /// Default color remembered for each automatic group when it first appears (`colors`, #65165).
    pub colors: BTreeMap<String, Color>,
}

impl Default for Doc {
    /// `DEFAULT_TAB_GROUPS_STATE`: grouped by server.
    fn default() -> Doc {
        Doc {
            mode: Mode::Server,
            group_order: Vec::new(),
            collapsed: BTreeMap::new(),
            overrides: BTreeMap::new(),
            manual_groups: Vec::new(),
            manual_assignments: BTreeMap::new(),
            colors: BTreeMap::new(),
        }
    }
}

impl Doc {
    pub fn to_json(&self) -> Value {
        let overrides: Map<String, Value> = self
            .overrides
            .iter()
            .map(|(k, o)| {
                let mut v = Map::new();
                if let Some(n) = &o.name {
                    v.insert("name".into(), json!(n));
                }
                if let Some(c) = o.color {
                    v.insert("color".into(), json!(c.name()));
                }
                (k.clone(), Value::Object(v))
            })
            .collect();
        let colors: Map<String, Value> = self.colors.iter().map(|(k, c)| (k.clone(), json!(c.name()))).collect();
        json!({
            "mode": self.mode.name(),
            "groupOrder": self.group_order,
            "collapsed": self.collapsed,
            "overrides": overrides,
            "manualGroups": self.manual_groups.iter().map(|g| json!({
                "id": g.id,
                "name": g.name,
                "color": g.color.name(),
            })).collect::<Vec<_>>(),
            "manualAssignments": self.manual_assignments,
            "colors": colors,
        })
    }

    pub fn from_json(v: &Value) -> Doc {
        if !v.is_object() {
            return Doc::default();
        }
        let obj = |k: &str| v[k].as_object().into_iter().flatten();
        Doc {
            mode: Mode::parse(v["mode"].as_str()),
            group_order: v["groupOrder"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect(),
            collapsed: obj("collapsed").filter_map(|(k, x)| Some((k.clone(), x.as_bool()?))).collect(),
            overrides: obj("overrides")
                .map(|(k, o)| {
                    (
                        k.clone(),
                        Override {
                            name: o["name"].as_str().map(str::to_owned),
                            color: Color::parse(o["color"].as_str()),
                        },
                    )
                })
                .collect(),
            manual_groups: v["manualGroups"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|g| {
                    Some(ManualGroup {
                        id: g["id"].as_str()?.to_owned(),
                        name: g["name"].as_str()?.to_owned(),
                        color: Color::parse(g["color"].as_str())?,
                    })
                })
                .collect(),
            manual_assignments: obj("manualAssignments")
                .filter_map(|(k, x)| Some((k.clone(), x.as_str()?.to_owned())))
                .collect(),
            colors: obj("colors")
                .filter_map(|(k, x)| Some((k.clone(), Color::parse(x.as_str())?)))
                .collect(),
        }
    }
}

/// Tab metadata resolved for grouping & filtering (`ResolvedTabMeta`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Meta {
    pub key: String,
    pub title: String,
    pub server: String,
    pub server_label: String,
    pub project_id: Option<String>,
    pub project_name: Option<String>,
    pub directory: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Ungrouped,
    Manual(String),
    Server,
    Project,
}

/// Computed section (`ComputedTabGroup`): `all` and `visible` hold indices into the window's tabs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub key: String,
    pub kind: Kind,
    pub default_name: String,
    pub name: String,
    pub color: Color,
    pub default_color: Color,
    pub collapsed: bool,
    pub all: Vec<usize>,
    pub visible: Vec<usize>,
}

impl Group {
    pub fn manual_id(&self) -> Option<&str> {
        match &self.kind {
            Kind::Manual(id) => Some(id),
            _ => None,
        }
    }
}

/// Server grouping separates tabs only when more than one server is known.
pub fn effective_mode(mode: Mode, servers: usize) -> Mode {
    if mode == Mode::Server && servers <= 1 { Mode::None } else { mode }
}

/// `hashGroupColor`: Java string hash over UTF-16 code units, choosing among unused colors first.
pub fn hash_color(seed: &str, used: &BTreeSet<&'static str>) -> Color {
    let hash = seed.encode_utf16().fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(i32::from(c)));
    let free: Vec<Color> = AUTO_COLORS.into_iter().filter(|c| !used.contains(c.name())).collect();
    let pool = if free.is_empty() { AUTO_COLORS.to_vec() } else { free };
    pool[(i64::from(hash).unsigned_abs() % pool.len() as u64) as usize]
}

pub fn next_manual_color(existing: &[ManualGroup]) -> Color {
    AUTO_COLORS
        .into_iter()
        .find(|c| !existing.iter().any(|g| g.color == *c))
        .unwrap_or(AUTO_COLORS[existing.len() % AUTO_COLORS.len()])
}

pub fn next_manual_number(existing: &[ManualGroup], format: &dyn Fn(usize) -> String) -> usize {
    let taken: BTreeSet<String> = existing.iter().map(|g| g.name.trim().to_lowercase()).collect();
    (1..=existing.len() + 1)
        .find(|&n| !taken.contains(&format(n).trim().to_lowercase()))
        .unwrap_or(existing.len() + 1)
}

/// Keeps the default color each group was first given (`remembered`), and picks one for a new
/// group in sorted seed order, avoiding colors the other present groups already use. Recoloring,
/// reordering, filtering or closing one group never changes another group's color (`#65165`).
fn stable_colors(seeds: impl IntoIterator<Item = String>, prefix: &str, remembered: &BTreeMap<String, Color>) -> HashMap<String, Color> {
    let unique: BTreeSet<String> = seeds.into_iter().collect();
    let mut used: BTreeSet<&'static str> = unique
        .iter()
        .filter_map(|seed| remembered.get(&format!("{prefix}{seed}")).map(|c| c.name()))
        .collect();
    let mut out = HashMap::new();
    for seed in &unique {
        if prefix == "project:" && seed == "other" {
            out.insert(seed.clone(), Color::Gray);
            continue;
        }
        if let Some(&saved) = remembered.get(&format!("{prefix}{seed}")) {
            out.insert(seed.clone(), saved);
            continue;
        }
        let picked = hash_color(seed, &used);
        used.insert(picked.name());
        out.insert(seed.clone(), picked);
    }
    out
}

/// Records the position and default color of each automatic group the first time it appears
/// (`rememberAutoGroups`, #65165). Without this, group order follows the first tab of each group
/// in tab order and colors depend on which groups are open, so closing one tab could reorder or
/// recolor the groups that remain.
pub fn remember_auto_groups(groups: &[Group], doc: &Doc) -> Option<(Vec<String>, BTreeMap<String, Color>)> {
    let auto: Vec<&Group> = groups.iter().filter(|g| matches!(g.kind, Kind::Server | Kind::Project)).collect();
    let known: BTreeSet<&str> = doc.group_order.iter().map(String::as_str).collect();
    let added: Vec<&Group> = auto.iter().copied().filter(|g| !known.contains(g.key.as_str())).collect();
    let uncolored: Vec<&Group> = auto.iter().copied().filter(|g| !doc.colors.contains_key(&g.key)).collect();
    if added.is_empty() && uncolored.is_empty() {
        return None;
    }
    let mut group_order = doc.group_order.clone();
    group_order.extend(added.into_iter().map(|g| g.key.clone()));
    let mut colors = doc.colors.clone();
    for g in uncolored {
        colors.insert(g.key.clone(), g.default_color);
    }
    Some((group_order, colors))
}

pub fn matches(meta: &Meta, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    let has = |s: &str| s.to_lowercase().contains(&q);
    has(&meta.title) || has(&meta.server_label) || meta.project_name.as_deref().is_some_and(has) || meta.directory.as_deref().is_some_and(has)
}

fn apply_saved_order(groups: Vec<Group>, order: &[String]) -> Vec<Group> {
    if order.is_empty() {
        return groups;
    }
    let rank: HashMap<&str, usize> = order.iter().enumerate().map(|(i, k)| (k.as_str(), i)).collect();
    let mut ranked: Vec<(Option<usize>, usize, Group)> = groups
        .into_iter()
        .enumerate()
        .map(|(i, g)| (rank.get(g.key.as_str()).copied(), i, g))
        .collect();
    ranked.sort_by(|a, b| match (a.0, b.0) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.1.cmp(&b.1),
    });
    ranked.into_iter().map(|(_, _, g)| g).collect()
}

pub fn prune_empty(groups: &[ManualGroup], assignments: &BTreeMap<String, String>) -> Vec<ManualGroup> {
    let populated: BTreeSet<&String> = assignments.values().collect();
    groups.iter().filter(|g| populated.contains(&g.id)).cloned().collect()
}

/// `canonicalizeManualTabOrder`: keeps every manual group's tabs contiguous at the first tab's slot.
pub fn canonical_order(keys: &[String], assignments: &BTreeMap<String, String>) -> Vec<String> {
    let mut by_group: HashMap<&str, Vec<String>> = HashMap::new();
    for k in keys {
        if let Some(g) = assignments.get(k) {
            by_group.entry(g.as_str()).or_default().push(k.clone());
        }
    }
    let mut emitted = BTreeSet::new();
    let mut out = Vec::with_capacity(keys.len());
    for k in keys {
        match assignments.get(k) {
            None => out.push(k.clone()),
            Some(g) if emitted.insert(g.as_str()) => out.extend(by_group.remove(g.as_str()).unwrap_or_default()),
            Some(_) => {}
        }
    }
    out
}

/// `reconcileManualAssignments`: carries a promoted draft's group to its new session tab and drops
/// assignments for closed tabs.
pub fn reconcile(previous: &[String], next: &[String], assignments: &BTreeMap<String, String>) -> Option<BTreeMap<String, String>> {
    if assignments.is_empty() || next.is_empty() {
        return None;
    }
    let prev_set: BTreeSet<&String> = previous.iter().collect();
    let next_set: BTreeSet<&String> = next.iter().collect();
    let mut updated = assignments.clone();
    let mut changed = false;
    if previous.len() == next.len() {
        for (p, n) in previous.iter().zip(next) {
            if p == n || !p.starts_with("draft:") || prev_set.contains(n) {
                continue;
            }
            if let Some(g) = updated.remove(p) {
                updated.insert(n.clone(), g);
                changed = true;
            }
        }
    }
    let before = updated.len();
    updated.retain(|k, _| next_set.contains(k));
    changed |= updated.len() != before;
    changed.then_some(updated)
}

/// `buildComputedTabGroups`: returns `None` when the strip renders as a plain ungrouped list.
pub fn build(items: &[Meta], doc: &Doc, servers: usize, known_servers: &[String], query: &str, other: &str) -> Option<Vec<Group>> {
    let searching = !query.trim().is_empty();
    let filtered: Vec<usize> = (0..items.len()).filter(|&i| !searching || matches(&items[i], query)).collect();
    let collapsed = |key: &str| !searching && doc.collapsed.get(key).copied().unwrap_or(false);
    match effective_mode(doc.mode, servers) {
        Mode::None if !doc.manual_groups.is_empty() => Some(manual(items, &filtered, doc, searching)),
        Mode::None if !searching => None,
        Mode::None => Some(vec![Group {
            key: "ungrouped:search".into(),
            kind: Kind::Ungrouped,
            default_name: String::new(),
            name: String::new(),
            color: Color::Gray,
            default_color: Color::Gray,
            collapsed: false,
            all: filtered.clone(),
            visible: filtered,
        }]),
        mode => {
            let (prefix, kind) = if mode == Mode::Server { ("server:", Kind::Server) } else { ("project:", Kind::Project) };
            let seed = |m: &Meta| match mode {
                Mode::Server => m.server.clone(),
                _ => m.project_id.clone().or_else(|| m.directory.clone()).unwrap_or_else(|| "other".into()),
            };
            let seeds: Vec<String> = if mode == Mode::Server {
                known_servers.iter().cloned().chain(items.iter().map(seed)).collect()
            } else {
                items.iter().map(seed).collect()
            };
            let colors = stable_colors(seeds, prefix, &doc.colors);
            let mut buckets: Vec<(String, Vec<usize>)> = Vec::new();
            for &i in &filtered {
                let s = seed(&items[i]);
                match buckets.iter_mut().find(|(k, _)| *k == s) {
                    Some((_, rows)) => rows.push(i),
                    None => buckets.push((s, vec![i])),
                }
            }
            let groups = buckets
                .into_iter()
                .map(|(seed, rows)| {
                    let key = format!("{prefix}{seed}");
                    let first = &items[rows[0]];
                    let default_name = if mode == Mode::Server {
                        first.server_label.clone()
                    } else {
                        first.project_name.clone().unwrap_or_else(|| other.to_owned())
                    };
                    let override_entry = doc.overrides.get(&key);
                    let name = override_entry
                        .and_then(|o| o.name.as_deref())
                        .map(str::trim)
                        .filter(|n| !n.is_empty())
                        .map_or_else(|| default_name.clone(), str::to_owned);
                    let fallback = if mode == Mode::Server { Color::Blue } else { Color::Gray };
                    let default_color = colors.get(&seed).copied().unwrap_or(fallback);
                    let color = override_entry.and_then(|o| o.color).unwrap_or(default_color);
                    let c = collapsed(&key);
                    Group {
                        color,
                        default_color,
                        collapsed: c,
                        visible: if c { Vec::new() } else { rows.clone() },
                        all: rows,
                        key,
                        kind: kind.clone(),
                        default_name,
                        name,
                    }
                })
                .collect();
            Some(apply_saved_order(groups, &doc.group_order))
        }
    }
}

fn manual(items: &[Meta], filtered: &[usize], doc: &Doc, searching: bool) -> Vec<Group> {
    let group_of = |i: usize| {
        doc.manual_assignments
            .get(&items[i].key)
            .and_then(|id| doc.manual_groups.iter().find(|g| g.id == *id))
    };
    let mut out = Vec::new();
    let mut run: Vec<usize> = Vec::new();
    let mut emitted = BTreeSet::new();
    let flush = |run: &mut Vec<usize>, out: &mut Vec<Group>| {
        if let Some(&first) = run.first() {
            let all = std::mem::take(run);
            out.push(Group {
                key: format!("ungrouped:{}", items[first].key),
                kind: Kind::Ungrouped,
                default_name: String::new(),
                name: String::new(),
                color: Color::Gray,
                default_color: Color::Gray,
                collapsed: false,
                visible: all.clone(),
                all,
            });
        }
    };
    for &i in filtered {
        let Some(g) = group_of(i) else {
            run.push(i);
            continue;
        };
        if !emitted.insert(g.id.clone()) {
            continue;
        }
        flush(&mut run, &mut out);
        let rows: Vec<usize> = filtered.iter().copied().filter(|&j| group_of(j).is_some_and(|x| x.id == g.id)).collect();
        let key = format!("manual:{}", g.id);
        let collapsed = !searching && doc.collapsed.get(&key).copied().unwrap_or(false);
        out.push(Group {
            key,
            kind: Kind::Manual(g.id.clone()),
            default_name: g.name.clone(),
            name: g.name.clone(),
            color: g.color,
            default_color: g.color,
            collapsed,
            visible: if collapsed { Vec::new() } else { rows.clone() },
            all: rows,
        });
    }
    flush(&mut run, &mut out);
    out
}
