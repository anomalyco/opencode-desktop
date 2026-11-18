//! What gives way in a narrow window (p263). The composer's toolbar folds its pickers before
//! Send could clip, picking the fewest steps that fit the width its row had at the last layout
//! (a read that settles in the same frame), from the widths its pieces measure to. The session
//! header's fit is `header_fit`.

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

