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
            Category::Shell => opencode_i18n::t!("settings.timeline.category.shell"),
            Category::Edit => opencode_i18n::t!("settings.timeline.category.edit"),
            Category::Thinking => opencode_i18n::t!("settings.timeline.category.thinking"),
            Category::Subagents => opencode_i18n::t!("settings.timeline.category.subagents"),
            Category::Notices => opencode_i18n::t!("settings.timeline.category.notices"),
            Category::Tools => opencode_i18n::t!("settings.timeline.category.tools"),
            Category::Commentary => opencode_i18n::t!("settings.timeline.category.commentary"),
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
pub const PRESETS: [(&str, opencode_i18n::Key, Detail); 5] = [
    ("everything", opencode_i18n::key!("settings.timeline.preset.everything"), Detail([a(S, true), a(S, true), a(S, true), a(S, false), a(S, false), a(S, false), a(S, false)])),
    ("detailed", opencode_i18n::key!("settings.timeline.preset.detailed"), Detail([a(S, true), a(S, true), a(G, false), a(S, false), a(G, false), a(G, false), a(S, false)])),
    ("compact", opencode_i18n::key!("settings.timeline.preset.compact"), Detail([a(G, false), a(G, false), a(G, false), a(S, false), a(G, false), a(G, false), a(S, false)])),
    ("quiet", opencode_i18n::key!("settings.timeline.preset.quiet"), Detail([a(H, false), a(G, false), a(H, false), a(S, false), a(H, false), a(H, false), a(S, false)])),
    ("text-only", opencode_i18n::key!("settings.timeline.preset.text-only"), Detail([a(H, false), a(H, false), a(H, false), a(H, false), a(H, false), a(H, false), a(S, false)])),
];

/// What each preset shows, as Settings says it (`settings.timeline.description.*`).
pub fn preset_description(id: &str) -> &'static str {
    match id {
        "everything" => opencode_i18n::t!("settings.timeline.description.everything"),
        "detailed" => opencode_i18n::t!("settings.timeline.description.detailed"),
        "compact" => opencode_i18n::t!("settings.timeline.description.compact"),
        "quiet" => opencode_i18n::t!("settings.timeline.description.quiet"),
        "text-only" => opencode_i18n::t!("settings.timeline.description.text-only"),
        _ => opencode_i18n::t!("settings.timeline.description.custom"),
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

/// Where a notice shows (`timeline-next/rules.ts`'s `noticePlacement`): a switched agent or
/// model and a move always keep their own row; the rest follow Notices. Background work that
/// came back follows the setting its launch follows, Subagents or Shell, as an event of its own,
/// and shows wherever it failed.
pub fn notice_placement(n: &opencode_kit::model::Notice, d: &Detail) -> Placement {
    use opencode_kit::model::NoticeKind;
    match (&n.kind, d.placement(Category::Notices)) {
        (NoticeKind::Idle { .. }, _) => Placement::Hidden,
        (NoticeKind::Finished(f), _) => {
            let placement = d.placement(if f.subagent { Category::Subagents } else { Category::Shell });
            if placement == Placement::Hidden && !f.required { Placement::Hidden } else { Placement::Separate }
        }
        // Failed background job notices (including failed Code Mode scripts) remain visible even
        // when Notices are hidden (`timelineNoticeRequired` in `session-ui/src/timeline/detail.ts`).
        (NoticeKind::Job { required: true, .. }, Placement::Hidden) => Placement::Separate,
        // What changed the session always stands on its own, whatever the setting
        // (`timeline-next/rules.ts`'s `sessionEvent`).
        (NoticeKind::Agent { .. } | NoticeKind::Model { .. } | NoticeKind::Moved { .. }, _) => Placement::Separate,
        // An answered widget says so under it whatever the setting, as the TUI always shows it.
        (NoticeKind::Widget { .. }, _) => Placement::Separate,
        (_, p) => p,
    }
}

/// Whether notice `n` folds with the turn's work: one Notices groups, and background work
/// reporting back, which joins an events row or the group that launched it (`groupNotices`).
pub fn notice_folds(n: &opencode_kit::model::Notice, d: &Detail) -> bool {
    match notice_placement(n, d) {
        Placement::Grouped => true,
        Placement::Separate => matches!(n.kind, opencode_kit::model::NoticeKind::Finished(_)),
        Placement::Hidden => false,
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
