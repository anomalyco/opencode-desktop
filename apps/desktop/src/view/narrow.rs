//! What gives way in a narrow window (p263). The header drops its lower items so the title
//! keeps room and the cost keeps its place; the composer's toolbar folds its pickers before
//! Send could clip. Each picks the fewest steps that fit the width its row had at the last
//! layout (a read that settles in the same frame), from the widths its pieces measure to.

use std::cell::Cell;

use rune::TextStyle;

/// `text`'s width in `style`, as layout will set it on one line.
pub(super) fn text_w(text: &str, style: TextStyle) -> f32 {
    rune::text::measure(text, &[], style, None).0
}

/// The fewest steps `0..=last` whose width fits `room` (`last` when none does). `need(k)` is the
/// row's width after `k` steps and only shrinks as `k` grows.
pub(super) fn steps(room: Option<f32>, last: usize, need: impl Fn(usize) -> f32) -> usize {
    let Some(room) = room else { return 0 };
    (0..=last).find(|&k| need(k) <= room + 0.5).unwrap_or(last)
}

/// The header's pieces in the order they go, the first first. The title takes what's left.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum HeaderDrop {
    /// The project's name and the slash after it (its badge stays).
    Name,
    /// The actions' details: plugins' header slots draw compact (the context ring alone).
    Tokens,
    /// A subagent's parent crumb.
    Parent,
    /// The ⋯ button (the title's right-click still opens its menu).
    More,
    /// The project's badge.
    Badge,
}

pub(crate) const HEADER_DROPS: [HeaderDrop; 5] = [HeaderDrop::Name, HeaderDrop::Tokens, HeaderDrop::Parent, HeaderDrop::More, HeaderDrop::Badge];

/// How much of the title must show before the header drops anything else for it.
pub(super) const TITLE_FLOOR: f32 = 96.0;

/// Narrower than this, the title goes rather than show a sliver.
pub(super) const TITLE_MIN: f32 = 40.0;

/// How the composer's toolbar folds, a step at a time.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Toolbar {
    /// The pickers' chevrons show.
    pub chevrons: bool,
    /// Effort has its own button; folded, it is chosen in the model picker (← →, ⇧⌘D).
    pub effort: bool,
    /// The agent's button: its name, an icon, or gone (⌘. still cycles it).
    pub agent: Agent,
    /// The model's name shows whole; otherwise it fades at its edge from `MODEL_FLOOR` down.
    pub model_whole: bool,
    /// The dictation mic shows (⇧⌘M still dictates).
    pub mic: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Agent {
    Name,
    Icon,
    Gone,
}

impl Toolbar {
    pub(crate) const FOLDS: usize = 6;

    /// The toolbar after `k` folds: chevrons, effort, the agent to an icon, the model's name
    /// fading, the mic, then the agent's icon.
    pub(crate) fn after(k: usize) -> Self {
        Toolbar {
            chevrons: k < 1,
            effort: k < 2,
            agent: match k { 0..=2 => Agent::Name, 3..=5 => Agent::Icon, _ => Agent::Gone },
            model_whole: k < 4,
            mic: k < 5,
        }
    }
}

/// How narrow the model's button gets before the toolbar folds further.
pub(super) const MODEL_FLOOR: f32 = 72.0;

thread_local! {
    static SHOWN: Cell<Toolbar> = const { Cell::new(Toolbar { chevrons: true, effort: true, agent: Agent::Name, model_whole: true, mic: true }) };
}

/// Notes how the composer's toolbar was last laid out, for keys that walk its buttons.
pub(super) fn note_toolbar(t: Toolbar) {
    SHOWN.set(t);
}

/// How the composer's toolbar was last laid out.
pub(crate) fn toolbar_shown() -> Toolbar {
    SHOWN.get()
}
