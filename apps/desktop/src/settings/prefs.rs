//! The choices Desktop's settings model keeps (`settings/model.tsx`) beyond a switch: each a
//! closed set saved by its name, read back as Desktop's default when missing or unknown.

/// A closed choice saved by name.
pub trait Choice: Copy + PartialEq + Default + 'static {
    const ALL: &'static [Self];
    fn name(self) -> &'static str;
    /// What Settings' select shows.
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
            fn label(self) -> &'static str { match self { $($ty::$v => $label),+ } }
        }
    };
}

choice! {
    /// Where a new session starts (`workspaces.defaultDestination`).
    Destination { LastUsed => "last-used", "Last used per project", Local => "local", "Local directory", New => "new", "New worktree" } default LastUsed
}

choice! {
    /// What Enter does while the agent works (`general.followUpBehavior`); the alternate chord does
    /// the other.
    FollowUp { Queue => "queue", "Queue", Steer => "steer", "Steer" } default Steer
}

choice! {
    /// The timeline's column (`general.timelineWidth`).
    TimelineWidth { Focused => "focused", "Focused", Comfortable => "comfortable", "Comfortable", Wide => "wide", "Wide" } default Comfortable
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
    BackgroundActivity { Off => "off", "Hidden", Left => "left", "Left", Center => "center", "Center", Right => "right", "Right" } default Left
}

choice! {
    /// How side tabs list sessions (`general.sidebarOrganize`).
    Organize { List => "list", "One list", Projects => "projects", "Projects" } default List
}

/// Desktop's sound effects (`shell/notifications/sound.ts`), by id and label.
pub const SOUNDS: &[(&str, &str)] = &[
    ("alert-01", "Alert 01"), ("alert-02", "Alert 02"), ("alert-03", "Alert 03"), ("alert-04", "Alert 04"), ("alert-05", "Alert 05"),
    ("alert-06", "Alert 06"), ("alert-07", "Alert 07"), ("alert-08", "Alert 08"), ("alert-09", "Alert 09"), ("alert-10", "Alert 10"),
    ("bip-bop-01", "Bip-bop 01"), ("bip-bop-02", "Bip-bop 02"), ("bip-bop-03", "Bip-bop 03"), ("bip-bop-04", "Bip-bop 04"), ("bip-bop-05", "Bip-bop 05"),
    ("bip-bop-06", "Bip-bop 06"), ("bip-bop-07", "Bip-bop 07"), ("bip-bop-08", "Bip-bop 08"), ("bip-bop-09", "Bip-bop 09"), ("bip-bop-10", "Bip-bop 10"),
    ("staplebops-01", "Staplebops 01"), ("staplebops-02", "Staplebops 02"), ("staplebops-03", "Staplebops 03"), ("staplebops-04", "Staplebops 04"),
    ("staplebops-05", "Staplebops 05"), ("staplebops-06", "Staplebops 06"), ("staplebops-07", "Staplebops 07"),
    ("nope-01", "Nope 01"), ("nope-02", "Nope 02"), ("nope-03", "Nope 03"), ("nope-04", "Nope 04"), ("nope-05", "Nope 05"), ("nope-06", "Nope 06"),
    ("nope-07", "Nope 07"), ("nope-08", "Nope 08"), ("nope-09", "Nope 09"), ("nope-10", "Nope 10"), ("nope-11", "Nope 11"), ("nope-12", "Nope 12"),
    ("yup-01", "Yup 01"), ("yup-02", "Yup 02"), ("yup-03", "Yup 03"), ("yup-04", "Yup 04"), ("yup-05", "Yup 05"), ("yup-06", "Yup 06"),
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
