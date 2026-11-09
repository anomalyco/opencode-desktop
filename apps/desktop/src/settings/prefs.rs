//! The choices Desktop's settings model keeps (`settings/model.tsx`) beyond a switch: each a
//! closed set saved by its name, read back as Desktop's default when missing or unknown.

/// A closed choice saved by name.
pub trait Choice: Copy + PartialEq + Default + 'static {
    const ALL: &'static [Self];
    fn name(self) -> &'static str;
    /// What Settings' select shows, in the current language.
    fn label(self) -> &'static str;

    fn parse(name: Option<&str>) -> Self {
        Self::ALL.iter().copied().find(|c| Some(c.name()) == name).unwrap_or_default()
    }
}

macro_rules! choice {
    ($(#[$doc:meta])* $ty:ident { $($(#[$vdoc:meta])* $v:ident => $name:literal, $label:literal),+ $(,)? } default $d:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $ty {
            $($(#[$vdoc])* $v,)+
        }

        impl Default for $ty {
            fn default() -> Self { $ty::$d }
        }

        impl Choice for $ty {
            const ALL: &'static [Self] = &[$($ty::$v),+];
            fn name(self) -> &'static str { match self { $($ty::$v => $name),+ } }
            fn label(self) -> &'static str { match self { $($ty::$v => opencode_i18n::t!($label)),+ } }
        }
    };
}

choice! {
    /// Where a new session starts (`workspaces.defaultDestination`).
    Destination { LastUsed => "last-used", "settings.workspaces.default.lastUsed", Local => "local", "settings.workspaces.default.local", New => "new", "settings.workspaces.default.new" } default LastUsed
}

choice! {
    /// What Enter does while the agent works (`general.followUpBehavior`); the alternate chord does
    /// the other.
    FollowUp { Queue => "queue", "settings.general.row.followUpBehavior.queue", Steer => "steer", "settings.general.row.followUpBehavior.steer" } default Steer
}

choice! {
    /// The timeline's column (`general.timelineWidth`).
    TimelineWidth { Focused => "focused", "settings.timeline.row.width.focused", Comfortable => "comfortable", "settings.timeline.row.width.comfortable", Wide => "wide", "settings.timeline.row.width.wide" } default Comfortable
}

impl TimelineWidth {
    pub fn width(self) -> session_view::style::Width {
        match self {
            TimelineWidth::Focused => session_view::style::Width::Focused,
            TimelineWidth::Comfortable => session_view::style::Width::Comfortable,
            TimelineWidth::Wide => session_view::style::Width::Wide,
        }
    }
}

choice! {
    /// Where running subagents and shells show over the composer (`general.backgroundActivity`).
    BackgroundActivity { Off => "off", "settings.timeline.row.backgroundActivity.off", Left => "left", "settings.timeline.row.backgroundActivity.left", Center => "center", "settings.timeline.row.backgroundActivity.center", Right => "right", "settings.timeline.row.backgroundActivity.right" } default Left
}

choice! {
    /// How side tabs list sessions (`general.sidebarOrganize`).
    Organize { List => "list", "settings.experimental.sidebarOrganize.list", Projects => "projects", "settings.experimental.sidebarOrganize.projects" } default List
}

/// Desktop's sound effects (`shell/notifications/sound.ts`), by id and label key.
pub const SOUNDS: &[(&str, opencode_i18n::Key)] = &[
    ("alert-01", opencode_i18n::key!("sound.option.alert01")), ("alert-02", opencode_i18n::key!("sound.option.alert02")), ("alert-03", opencode_i18n::key!("sound.option.alert03")), ("alert-04", opencode_i18n::key!("sound.option.alert04")), ("alert-05", opencode_i18n::key!("sound.option.alert05")),
    ("alert-06", opencode_i18n::key!("sound.option.alert06")), ("alert-07", opencode_i18n::key!("sound.option.alert07")), ("alert-08", opencode_i18n::key!("sound.option.alert08")), ("alert-09", opencode_i18n::key!("sound.option.alert09")), ("alert-10", opencode_i18n::key!("sound.option.alert10")),
    ("bip-bop-01", opencode_i18n::key!("sound.option.bipbop01")), ("bip-bop-02", opencode_i18n::key!("sound.option.bipbop02")), ("bip-bop-03", opencode_i18n::key!("sound.option.bipbop03")), ("bip-bop-04", opencode_i18n::key!("sound.option.bipbop04")), ("bip-bop-05", opencode_i18n::key!("sound.option.bipbop05")),
    ("bip-bop-06", opencode_i18n::key!("sound.option.bipbop06")), ("bip-bop-07", opencode_i18n::key!("sound.option.bipbop07")), ("bip-bop-08", opencode_i18n::key!("sound.option.bipbop08")), ("bip-bop-09", opencode_i18n::key!("sound.option.bipbop09")), ("bip-bop-10", opencode_i18n::key!("sound.option.bipbop10")),
    ("staplebops-01", opencode_i18n::key!("sound.option.staplebops01")), ("staplebops-02", opencode_i18n::key!("sound.option.staplebops02")), ("staplebops-03", opencode_i18n::key!("sound.option.staplebops03")), ("staplebops-04", opencode_i18n::key!("sound.option.staplebops04")),
    ("staplebops-05", opencode_i18n::key!("sound.option.staplebops05")), ("staplebops-06", opencode_i18n::key!("sound.option.staplebops06")), ("staplebops-07", opencode_i18n::key!("sound.option.staplebops07")),
    ("nope-01", opencode_i18n::key!("sound.option.nope01")), ("nope-02", opencode_i18n::key!("sound.option.nope02")), ("nope-03", opencode_i18n::key!("sound.option.nope03")), ("nope-04", opencode_i18n::key!("sound.option.nope04")), ("nope-05", opencode_i18n::key!("sound.option.nope05")), ("nope-06", opencode_i18n::key!("sound.option.nope06")),
    ("nope-07", opencode_i18n::key!("sound.option.nope07")), ("nope-08", opencode_i18n::key!("sound.option.nope08")), ("nope-09", opencode_i18n::key!("sound.option.nope09")), ("nope-10", opencode_i18n::key!("sound.option.nope10")), ("nope-11", opencode_i18n::key!("sound.option.nope11")), ("nope-12", opencode_i18n::key!("sound.option.nope12")),
    ("yup-01", opencode_i18n::key!("sound.option.yup01")), ("yup-02", opencode_i18n::key!("sound.option.yup02")), ("yup-03", opencode_i18n::key!("sound.option.yup03")), ("yup-04", opencode_i18n::key!("sound.option.yup04")), ("yup-05", opencode_i18n::key!("sound.option.yup05")), ("yup-06", opencode_i18n::key!("sound.option.yup06")),
];

/// What each event plays (`sounds`): a sound id, or `None` for none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sounds {
    pub agent: Option<String>,
    pub permissions: Option<String>,
    pub questions: Option<String>,
    pub errors: Option<String>,
    /// Interface sounds: the wordmark's.
    pub interface: bool,
}

impl Default for Sounds {
    fn default() -> Self {
        let on = |id: &str| Some(id.to_owned());
        Sounds { agent: on("staplebops-01"), permissions: on("staplebops-02"), questions: on("staplebops-05"), errors: on("nope-03"), interface: true }
    }
}

impl Sounds {
    pub fn to_json(&self) -> serde_json::Value {
        let pair = |s: &Option<String>, d: &str| (s.is_some(), s.clone().unwrap_or_else(|| d.to_owned()));
        let d = Sounds::default();
        let (ae, a) = pair(&self.agent, d.agent.as_deref().unwrap_or_default());
        let (pe, p) = pair(&self.permissions, d.permissions.as_deref().unwrap_or_default());
        let (qe, q) = pair(&self.questions, d.questions.as_deref().unwrap_or_default());
        let (ee, e) = pair(&self.errors, d.errors.as_deref().unwrap_or_default());
        serde_json::json!({
            "agentEnabled": ae, "agent": a, "permissionsEnabled": pe, "permissions": p,
            "questionsEnabled": qe, "questions": q, "errorsEnabled": ee, "errors": e, "interfaceEnabled": self.interface,
        })
    }

    /// Desktop's shape: each event's `<kind>Enabled` and its sound id.
    pub fn from_json(v: &serde_json::Value) -> Sounds {
        let d = Sounds::default();
        let read = |kind: &str, d: &Option<String>| {
            let on = v[format!("{kind}Enabled")].as_bool().unwrap_or(true);
            on.then(|| v[kind].as_str().filter(|id| SOUNDS.iter().any(|(s, _)| s == id)).map(str::to_owned).or_else(|| d.clone())).flatten()
        };
        Sounds {
            agent: read("agent", &d.agent),
            permissions: read("permissions", &d.permissions),
            questions: read("questions", &d.questions),
            errors: read("errors", &d.errors),
            interface: v["interfaceEnabled"].as_bool().unwrap_or(true),
        }
    }
}

choice! {
    /// Where a narrow window keeps its titlebar and view switcher (`general.mobileTitlebarPosition`).
    TitlebarPosition { Top => "top", "settings.general.row.mobileTitlebarBottom.title", Bottom => "bottom", "settings.general.row.mobileTitlebarBottom.title" } default Top
}
