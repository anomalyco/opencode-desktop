//! How far the session header gives way so its title fits (`session/header/fit-machine.ts`): the
//! worktree's name hides first, then the project's name, then the actions drop their details
//! (`compact`); only then does the title truncate. A plain state machine fed the widths the
//! header's pieces had at the last layout, returning the same state when nothing changes.

/// The fit, as Desktop's `HeaderFitState`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum HeaderFit {
    Full,
    WorktreeHidden,
    ProjectHidden,
    Compact {
        /// The actions' expanded width on entry, until their compact width is measured.
        expanded: Option<f32>,
        /// The width compacting freed: expanding takes it back.
        saved: f32,
    },
}

/// What one layout measured (`HeaderFitMeasuredEvent`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Measured {
    pub location: f32,
    pub title_box: f32,
    /// What the title section's content needs, the title at its full width.
    pub title: f32,
    /// The project's name and the gap before it.
    pub project: f32,
    /// The worktree's name and the gap before it, if it shows one.
    pub worktree: Option<f32>,
    /// The location's widest (60% of the header beside a subagent's ancestors).
    pub limit: f32,
    pub actions: f32,
}

impl HeaderFit {
    /// 0 shows the whole location, 1 hides the worktree's name, 2 the project's too.
    pub(crate) fn collapse(self) -> u8 {
        match self {
            Self::Full => 0,
            Self::WorktreeHidden => 1,
            _ => 2,
        }
    }

    /// The title doesn't fit even with the location collapsed: the actions show without details.
    pub(crate) fn compact(self) -> bool {
        matches!(self, Self::Compact { .. })
    }

    /// `compact.settled`: the actions were measured after they drew compact.
    pub(crate) fn settled(self, actions: f32) -> Self {
        match self {
            Self::Compact { expanded: Some(e), .. } => Self::Compact { expanded: None, saved: (e - actions).max(0.0) },
            other => other,
        }
    }

    /// `measured`: hidden labels keep their own width, so a fit never depends on an earlier one.
    pub(crate) fn measured(self, m: Measured) -> Self {
        let collapse = self.collapse();
        let worktree = m.worktree.unwrap_or(0.0);
        let expanded = m.location + if collapse >= 1 { worktree } else { 0.0 } + if collapse >= 2 { m.project } else { 0.0 };
        let saved = match self {
            Self::Compact { expanded: None, saved } => saved,
            Self::Compact { expanded: Some(e), .. } => (e - m.actions).max(0.0),
            _ => 0.0,
        };
        // Everything past the location and title is the actions', fitted against their expanded
        // width: compact actions expand again once the title fits beside them.
        let available = m.location + m.title_box - m.title - saved;
        if expanded.min(m.limit) <= available {
            return Self::Full;
        }
        if m.worktree.is_some() && (expanded - worktree).min(m.limit) <= available {
            return Self::WorktreeHidden;
        }
        if (expanded - worktree - m.project).min(m.limit) <= available {
            return Self::ProjectHidden;
        }
        match self {
            Self::Compact { .. } => Self::Compact { expanded: None, saved },
            _ => Self::Compact { expanded: Some(m.actions), saved: 0.0 },
        }
    }
}
