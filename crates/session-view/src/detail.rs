//! How much of a turn's work the timeline shows (Desktop's `timeline/detail.ts`, Settings ›
//! Preferences › Timeline detail): for each kind of activity whether it stands on its own row,
//! folds into the turn's group or doesn't show, and for shells, edits and thinking whether they
//! show open. Five presets name the common mixes; anything else is custom.

use opencode_kit::model::{Part, ToolStatus};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    Shell,
    Edit,
    Thinking,
    Subagents,
    Notices,
    Tools,
    Commentary,
}

impl Category {
    /// In the order Settings lists them (`timelineCategories`).
    pub const ALL: [Category; 7] = [Category::Shell, Category::Edit, Category::Thinking, Category::Subagents, Category::Notices, Category::Tools, Category::Commentary];

    pub fn id(self) -> &'static str {
        match self {
            Category::Shell => "shell",
            Category::Edit => "edit",
            Category::Thinking => "thinking",
            Category::Subagents => "subagents",
            Category::Notices => "notices",
            Category::Tools => "tools",
            Category::Commentary => "commentary",
        }
    }

    /// Its name in Settings (`settings.timeline.category.*`).
    pub fn label(self) -> &'static str {
        match self {
            Category::Shell => "Shell",
            Category::Edit => "Edits",
            Category::Thinking => "Thinking",
            Category::Subagents => "Subagents",
            Category::Notices => "Notices",
            Category::Tools => "Other tools",
            Category::Commentary => "Commentary",
        }
    }

    /// Shells, edits and thinking show open or folded; the rest only place.
    pub fn has_details(self) -> bool {
        matches!(self, Category::Shell | Category::Edit | Category::Thinking)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Placement {
    Separate,
    Grouped,
    Hidden,
}

impl Placement {
    pub fn name(self) -> &'static str {
        match self {
            Placement::Separate => "separate",
            Placement::Grouped => "grouped",
            Placement::Hidden => "hidden",
        }
    }

    pub fn parse(name: Option<&str>) -> Option<Placement> {
        [Placement::Separate, Placement::Grouped, Placement::Hidden].into_iter().find(|p| Some(p.name()) == name)
    }
}

/// One category's setting: where it shows, and (shells, edits, thinking) whether it shows open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Activity {
    pub placement: Placement,
    pub expanded: bool,
}

const fn a(placement: Placement, expanded: bool) -> Activity {
    Activity { placement, expanded }
}

/// Every category's setting, in [`Category::ALL`]'s order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Detail(pub [Activity; 7]);

use Placement::{Grouped as G, Hidden as H, Separate as S};

/// The presets, quietest last (`timelinePresets`): Everything, Detailed, Compact, Quiet, Messages only.
pub const PRESETS: [(&str, &str, Detail); 5] = [
    ("everything", "Everything", Detail([a(S, true), a(S, true), a(S, true), a(S, false), a(S, false), a(S, false), a(S, false)])),
    ("detailed", "Detailed", Detail([a(S, true), a(S, true), a(G, false), a(S, false), a(G, false), a(G, false), a(S, false)])),
    ("compact", "Compact", Detail([a(G, false), a(G, false), a(G, false), a(S, false), a(G, false), a(G, false), a(S, false)])),
    ("quiet", "Quiet", Detail([a(H, false), a(G, false), a(H, false), a(S, false), a(H, false), a(H, false), a(S, false)])),
    ("text-only", "Messages only", Detail([a(H, false), a(H, false), a(H, false), a(H, false), a(H, false), a(H, false), a(S, false)])),
];

/// What each preset shows, as Settings says it (`settings.timeline.description.*`).
pub fn preset_description(id: &str) -> &'static str {
    match id {
        "everything" => "Show all activity separately. Expand shell output, edits, and thinking.",
        "detailed" => "Expand shell output and edits. Show subagents separately and group other activity.",
        "compact" => "Show subagents separately. Group other activity with details collapsed.",
        "quiet" => "Show subagents separately and group edits. Hide other activity.",
        "text-only" => "Hide all activity.",
        _ => "Uses advanced settings.",
    }
}

impl Default for Detail {
    /// Desktop's default, Compact.
    fn default() -> Self {
        PRESETS[2].2
    }
}

impl Detail {
    pub fn get(&self, c: Category) -> Activity {
        self.0[c as usize]
    }

    pub fn set(&mut self, c: Category, v: Activity) {
        self.0[c as usize] = v;
    }

    pub fn placement(&self, c: Category) -> Placement {
        self.get(c).placement
    }

    /// Whether category `c` shows open: its own setting for shells, edits and thinking.
    pub fn expanded(&self, c: Category) -> bool {
        c.has_details() && self.get(c).expanded
    }

    /// The preset this is (`timelinePreset`): a hidden category's details don't count.
    pub fn preset(&self) -> Option<usize> {
        PRESETS.iter().position(|(_, _, p)| {
            Category::ALL.iter().all(|c| {
                let (now, want) = (self.get(*c), p.get(*c));
                now.placement == want.placement && (now.placement == Placement::Hidden || !c.has_details() || now.expanded == want.expanded)
            })
        })
    }

    pub fn to_json(&self) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        for c in Category::ALL {
            let v = self.get(c);
            let mut o = serde_json::json!({ "placement": v.placement.name() });
            if c.has_details() {
                o["details"] = (if v.expanded { "expanded" } else { "collapsed" }).into();
            }
            m.insert(c.id().into(), o);
        }
        m.into()
    }

    /// Desktop's shape; a category missing or unknown keeps the default's.
    pub fn from_json(v: &serde_json::Value) -> Detail {
        let mut d = Detail::default();
        for c in Category::ALL {
            let o = &v[c.id()];
            let base = d.get(c);
            let placement = Placement::parse(o["placement"].as_str()).unwrap_or(base.placement);
            let expanded = o["details"].as_str().map_or(base.expanded, |x| x == "expanded");
            d.set(c, Activity { placement, expanded });
        }
        d
    }
}

/// The category a tool call is in (`timelineCategory`).
pub fn tool_category(name: &str) -> Category {
    match name {
        "shell" | "execute" | "bash" => Category::Shell,
        "edit" | "write" | "patch" | "apply_patch" | "multiedit" => Category::Edit,
        "subagent" | "task" => Category::Subagents,
        _ => Category::Tools,
    }
}

/// The category a part is in, none for text (`timelineCategory`).
pub fn part_category(part: &Part) -> Option<Category> {
    match part {
        Part::Text { .. } => None,
        Part::Reasoning { .. } => Some(Category::Thinking),
        Part::Tool { name, .. } => Some(tool_category(name)),
    }
}

/// Where a notice shows (`timelineNoticePlacement`, `groupMessages`' separate set): a switched
/// model or a move keeps its own row unless notices are hidden; the rest follow Notices.
pub fn notice_placement(n: &opencode_kit::model::Notice, d: &Detail) -> Placement {
    use opencode_kit::model::NoticeKind;
    match (&n.kind, d.placement(Category::Notices)) {
        (NoticeKind::Idle, _) => Placement::Hidden,
        (NoticeKind::Model { .. } | NoticeKind::Moved { .. }, Placement::Grouped) => Placement::Separate,
        (_, p) => p,
    }
}

/// A failed call (`currentToolFailed`): an error, an execute whose nested calls failed, or a
/// shell that exited non-zero or timed out. It shows even where its category is hidden.
pub fn failed(name: &str, status: &ToolStatus, metadata: &serde_json::Value) -> bool {
    match (name, status) {
        (_, ToolStatus::Error) => true,
        ("execute", _) => metadata["error"] == true || metadata["toolCalls"].as_array().is_some_and(|c| c.iter().any(|c| c["status"] == "error")),
        ("shell", ToolStatus::Completed) => metadata["timeout"] == true || metadata["exit"].as_i64().is_some_and(|e| e != 0),
        _ => false,
    }
}
