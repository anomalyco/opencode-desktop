//! Subagents on one line (Rune-only). Official gives each subagent event a row of its own
//! (`timeline-next/event-row.tsx`'s `TimelineEventItem`, anomalyco/opencode#64017), which a long
//! turn interleaves with its "Ran N commands" groups. Here, a deliberate divergence agreed by the
//! team, a stretch of work that called on subagents is one line of constant height
//! (`turns::agents` gathers it): what its commands did, "N subagents", then while they fit in the
//! column the agents' tasks, each once in its latest state, its ink saying which (text while it
//! runs, muted once done or stopped, danger when it failed), with no marks. When they no longer
//! fit, counts take their place in fixed places, running, done, failed and stopped, each as wide
//! whether it shows or not and in tabular digits, so a status changing changes a number and
//! moves nothing beside it. A press opens the group: its agents, a row each, then its steps.
//! What a server event changes moves on the render clock ([`motion`]).

pub mod motion;

use rune::{Beat, Color, Easing, El, Sequence, TextStyle, Transition, col, el, row, text};

use super::events::{Event, Verb};
use super::label::{Activity, step_event};
use crate::TimelineHost;
use crate::palette::Pal;
use crate::style::{ROW_GAP, UI};
use crate::turns::agents::Called;
use motion::{Look, Shows, State};

/// Tabular digits, so a number changing keeps its width.
const TNUM: TextStyle = UI.features("\"tnum\"");
/// Between the line's parts, as between a group label's.
const GAP: f32 = 4.0;
/// Room each side of a middle dot, past the gap.
const DOT_PAD: f32 = 2.0;
/// Between "N subagents" and the names or counts, past the gap.
const AFTER_TOTAL: f32 = 4.0;
/// Between two count places.
const PLACE_GAP: f32 = 6.0;
/// The arrow after the line (16 wide, 2 in), which names leave room for.
const ARROW: f32 = 18.0;
/// The line's text.
const LINE: f32 = 16.0;
/// How far a rolling digit travels, and the soft edge it rolls through.
const ROLL: f32 = 10.0;
const EDGE: f32 = 2.0;
const DOT: &str = "\u{00b7}";

const FADE: f64 = 0.28;
const ROLL_T: f64 = 0.3;
/// The odometer's quick roll without its bounce (`opencode_ui::odometer::SPRING`), and the curve
/// the swap comes in on.
const SNAPPY: Easing = Easing::cubic(0.2, 0.8, 0.2, 1.0);
/// What something new or gone fades on: a name joining, a count place appearing or emptied.
static SHOW: &[Beat] = &[Beat::new(0.0).opacity(0.0), Beat::new(FADE).easing(Easing::EASE)];
static HIDE: &[Beat] = &[Beat::new(0.0), Beat::new(FADE).opacity(0.0).easing(Easing::EASE)];
/// The names and the counts trade places as Compacting turns Compacted (`compaction::quiet_label`):
/// what goes leaves quickly, and what comes follows it in, so the two barely overlap.
static SWAP_OUT: &[Beat] = &[Beat::new(0.0), Beat::new(0.15).opacity(0.0).easing(Easing::EASE_OUT)];
static SWAP_IN: &[Beat] = &[Beat::new(0.0).opacity(0.0), Beat::new(0.1).opacity(0.0), Beat::new(0.38).easing(SNAPPY)];
/// The same words in a new ink, or a word gaining a letter, come in over the old ones, which go
/// once covered: a crossfade of one shape that never dims.
static UNDER: &[Beat] = &[Beat::new(0.0), Beat::new(FADE - 0.06), Beat::new(FADE + 0.04).opacity(0.0)];
static IN_UP: &[Beat] = &[Beat::new(0.0).opacity(0.0).translate(0.0, ROLL), Beat::new(ROLL_T).easing(SNAPPY)];
static OUT_UP: &[Beat] = &[Beat::new(0.0), Beat::new(ROLL_T).opacity(0.0).translate(0.0, -ROLL).easing(SNAPPY)];
static IN_DOWN: &[Beat] = &[Beat::new(0.0).opacity(0.0).translate(0.0, -ROLL), Beat::new(ROLL_T).easing(SNAPPY)];
static OUT_DOWN: &[Beat] = &[Beat::new(0.0), Beat::new(ROLL_T).opacity(0.0).translate(0.0, ROLL).easing(SNAPPY)];

/// Where an agent is, which its ink says and its count place counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Status {
    Running,
    Done,
    Failed,
    Stopped,
}

impl Status {
    /// The count places, in their fixed order.
    const ALL: [Status; 4] = [Status::Running, Status::Done, Status::Failed, Status::Stopped];

    fn of(e: &Event) -> Status {
        if e.failed {
            return Status::Failed;
        }
        match e.verb {
            Verb::Started | Verb::Updated => Status::Running,
            Verb::Ran | Verb::Finished => Status::Done,
            Verb::Stopped => Status::Stopped,
        }
    }

    /// "4 running", "1 done", "1 failed", "2 stopped".
    fn count(self, n: usize) -> String {
        use opencode_i18n::t;
        match self {
            Status::Running => t!("rune.timeline.subagents.running", count = n),
            Status::Done => t!("rune.timeline.subagents.done", count = n),
            Status::Failed => t!("ui.timeline.event.failed", count = n),
            Status::Stopped => t!("rune.timeline.subagents.stopped", count = n),
        }
    }

    /// A name's ink: text while it runs, muted once done or stopped, danger when it failed.
    fn name_ink(self, p: &Pal) -> Color {
        match self {
            Status::Running => p.text,
            Status::Done | Status::Stopped => p.muted,
            Status::Failed => p.danger,
        }
    }

    /// A count's ink: muted, failures in danger.
    fn count_ink(self, p: &Pal) -> Color {
        if self == Status::Failed { p.danger } else { p.muted }
    }

    /// Its count place's width: its words with a two-digit count.
    fn place_w(self) -> f32 {
        measure(&self.count(88), TNUM)
    }
}

/// One agent on the line.
pub struct Agent {
    /// Who it is (`turns::agents::who`), which its motion keys by.
    id: u64,
    /// Its task, else its agent's name.
    name: String,
    status: Status,
    /// Its latest event, which its row in the open group shows.
    event: Event,
}

/// A group's agents (`Block::Group`'s `agents`): each where its latest event says, named by the
/// task its first one gave it.
pub fn agents(called: &[Called]) -> Vec<Agent> {
    called
        .iter()
        .filter_map(|c| {
            let who = crate::turns::agents::who(&c.latest)?;
            let mut event = step_event(&c.latest)?;
            if let Some(task) = step_event(&c.first).and_then(|e| e.title) {
                event.title = Some(task);
            }
            let mut h = std::hash::DefaultHasher::new();
            std::hash::Hash::hash(&who, &mut h);
            let name = event.title.clone().unwrap_or_else(|| agent_name(&event));
            Some(Agent { id: std::hash::Hasher::finish(&h), name, status: Status::of(&event), event })
        })
        .collect()
}

/// "General Agent", as a subagent row names one.
fn agent_name(e: &Event) -> String {
    match e.agent.as_deref().filter(|a| !a.is_empty()) {
        Some(agent) => opencode_i18n::t!("ui.timeline.subagent.name", name = opencode_kit::model::capitalize(agent)),
        None => opencode_i18n::t!("ui.tool.agent.default").to_owned(),
    }
}

fn measure(s: &str, font: TextStyle) -> f32 {
    rune::text::measure(s, &[], font, None).0
}

/// How wide a group label's activity words (`turns::view`'s `activity_words`) set `a`.
pub fn activity_w(a: &Activity) -> f32 {
    let verb = measure(&a.verb, UI);
    match (a.main.is_empty(), a.others.is_empty()) {
        (true, true) => verb,
        (false, true) => verb + 6.0 + measure(&a.main, UI),
        (true, false) => verb + 12.0 + measure(DOT, UI) + measure(&a.others.join(", "), UI),
        (false, false) => verb + 6.0 + measure(&format!("{}  \u{00b7}  {}", a.main, a.others.join(", ")), UI),
    }
}

/// What a line's commands did, as its group would label them: their words, how wide they set,
/// and what a reader hears.
pub struct Lead {
    pub words: El,
    pub w: f32,
    pub said: String,
}

/// The line, for a group's trigger, and what a reader hears of it: the commands' words
/// (`lead`), "N subagents", then the agents' names while they fit in the column, else their
/// counts. `key`: the group's row, which the line's motion goes by.
pub fn line<S: TimelineHost>(s: &S, p: &Pal, session: &str, key: &str, lead: Option<Lead>, agents: &[Agent]) -> (El, String) {
    let total = agents.len();
    let mut counts = [0; 4];
    for a in agents {
        counts[a.status as usize] += 1;
    }
    let say_total = |n: usize| opencode_i18n::plural!("ui.timeline.event.subagents", n);
    // The count holds its plural's room, so "1 subagent" becoming "2 subagents" moves nothing.
    let total_w = measure(&say_total(total.max(2)), TNUM);
    let dot_w = measure(DOT, UI) + 2.0 * DOT_PAD;
    let lead_w = lead.as_ref().map_or(0.0, |l| l.w + GAP + dot_w + GAP);
    // Decided by width, not by how many: the names show while all of them fit whole.
    let room = crate::style::content_w() - lead_w - total_w - GAP - AFTER_TOTAL - GAP - ARROW;
    let names_w = agents.iter().map(|a| measure(&a.name, UI)).sum::<f32>() + total.saturating_sub(1) as f32 * (dot_w + 2.0 * GAP);
    let counts_w = Status::ALL.iter().map(|st| st.place_w()).sum::<f32>() + 3.0 * PLACE_GAP;
    let shows = if names_w <= room { Shows::Names } else { Shows::Counts };
    let look = Look { shows, total, counts, agents: agents.iter().map(|a| (a.id, a.status)).collect() };
    let state = motion::drawn(session, key, look.clone(), s.clock(), s.reduced_motion());
    let (was, nth) = match &state {
        State::Moving { from, nth, .. } => (Some(from), *nth),
        State::Still { nth, .. } => (None, *nth),
        State::Unseen => (None, 0),
    };

    let mut said = lead.as_ref().map(|l| format!("{} \u{00b7} ", l.said)).unwrap_or_default();
    said.push_str(&say_total(total));
    let mut out = row().key("agents").gap(GAP).center_y().shrink().min_w(0.0).transition(Transition::none());
    if let Some(l) = lead {
        out = out.child(l.words).child(text(DOT).key("lead-dot").font(UI).color(p.faint).mx(DOT_PAD));
    }
    out = out.child(counted("total", &say_total, total, was.map(|w| w.total), nth, p.text).w(total_w));
    let names_layer = |look: &Look, was: Option<&Look>| names(p, agents, look, was, nth);
    let counts_layer = |look: &Look, was: Option<&Look>| places(p, look, was, nth);
    let layer = |look: &Look, was: Option<&Look>| match look.shows {
        Shows::Names => names_layer(look, was),
        Shows::Counts => counts_layer(look, was),
    };
    let mut swap = el().key("swap").w(if shows == Shows::Names { names_w } else { counts_w }).h(LINE).ml(AFTER_TOTAL).transition(Transition::none());
    swap = match was {
        // The names give way to the counts, or the counts to the names, crossfading in place.
        Some(from) if from.shows != shows => swap
            .child(layer(from, None).key(format!("was-{nth}")).absolute(0.0, 0.0).sequence(Sequence::new(SWAP_OUT)).transition(Transition::none()))
            .child(layer(&look, None).key(format!("now-{nth}")).absolute(0.0, 0.0).sequence(Sequence::new(SWAP_IN)).transition(Transition::none())),
        _ => swap.child(layer(&look, was).key("now").absolute(0.0, 0.0).transition(Transition::none())),
    };
    said.push_str(": ");
    said.push_str(&match shows {
        Shows::Names => agents.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", "),
        Shows::Counts => Status::ALL.iter().filter(|st| counts[**st as usize] > 0).map(|st| st.count(counts[*st as usize])).collect::<Vec<_>>().join(", "),
    });
    (out.child(swap), said)
}

/// The agents' names in `look`, a faint dot between, each in its state's ink. Moving from
/// `was`, a name new to the line fades in, and one whose state changed crossfades its ink.
fn names(p: &Pal, agents: &[Agent], look: &Look, was: Option<&Look>, nth: u64) -> El {
    let mut r = row().gap(GAP).center_y().transition(Transition::none());
    let mut first = true;
    for &(id, status) in &look.agents {
        let Some(a) = agents.iter().find(|a| a.id == id) else { continue };
        let before = was.map(|w| w.agents.iter().find(|(i, _)| *i == id).map(|(_, st)| *st));
        let fresh = matches!(before, Some(None));
        let name = |key: String, st: Status| text(a.name.clone()).key(key).font(UI).color(st.name_ink(p)).transition(Transition::none());
        if !first {
            let dot = text(DOT).key(format!("dot-{id:x}")).font(UI).color(p.faint).mx(DOT_PAD).transition(Transition::none());
            r = r.child(if fresh { dot.key(format!("dot-{id:x}-{nth}")).sequence(Sequence::new(SHOW)) } else { dot });
        }
        first = false;
        r = r.child(match before {
            Some(None) => name(format!("name-{id:x}-{nth}"), status).sequence(Sequence::new(SHOW)),
            Some(Some(old)) if old != status => el()
                .key(format!("name-{id:x}-{nth}"))
                .transition(Transition::none())
                .child(name("was".into(), old).absolute(0.0, 0.0).sequence(Sequence::new(UNDER)))
                .child(name("now".into(), status).sequence(Sequence::new(SHOW))),
            _ => name(format!("name-{id:x}"), status),
        });
    }
    r
}

/// The counts in `look`, each status in its fixed place, as wide whether it shows or not.
/// Moving from `was`, a place appearing fades in, one emptied fades out on its last count, and
/// a count that changed rolls.
fn places(p: &Pal, look: &Look, was: Option<&Look>, nth: u64) -> El {
    let mut r = row().gap(PLACE_GAP).center_y().transition(Transition::none());
    for (k, st) in Status::ALL.into_iter().enumerate() {
        let (n, before) = (look.counts[k], was.map(|w| w.counts[k]));
        let place = |key: String| row().key(key).w(st.place_w()).h(LINE).center_y().transition(Transition::none());
        let say = |n: usize| st.count(n);
        let ink = st.count_ink(p);
        r = r.child(match before {
            Some(0) if n > 0 => place(format!("place-{k}-{nth}")).sequence(Sequence::new(SHOW)).child(counted("n", &say, n, None, nth, ink)),
            Some(b) if b > 0 && n == 0 => place(format!("place-{k}-{nth}")).sequence(Sequence::new(HIDE)).child(counted("n", &say, b, None, nth, ink)),
            _ if n == 0 => place(format!("place-{k}")).opacity(0.0),
            _ => place(format!("place-{k}")).child(counted("n", &say, n, before, nth, ink)),
        });
    }
    r
}

/// The words around number `n` in `words`, and the room between them and it.
struct Around {
    before: String,
    after: String,
    gaps: (f32, f32),
}

fn around(words: &str, n: usize) -> Option<Around> {
    let digits = n.to_string();
    let at = words.find(&digits)?;
    let (before, after) = (&words[..at], &words[at + digits.len()..]);
    // A space between the words and the number, as one run of text sets it.
    let space = measure("0 0", TNUM) - measure("00", TNUM);
    let spaces = |s: &str, trimmed: &str| (s.chars().count() - trimmed.chars().count()) as f32 * space;
    Some(Around {
        before: before.trim().to_owned(),
        after: after.trim().to_owned(),
        gaps: (spaces(before, before.trim_end()), spaces(after, after.trim_start())),
    })
}

/// `say(n)`, its number in tabular digits that roll from `was` (move `nth`), and words that
/// change with it ("1 subagent", "2 subagents") coming in over the old. Keyed `key`.
fn counted(key: &str, say: &dyn Fn(usize) -> String, n: usize, was: Option<usize>, nth: u64, ink: Color) -> El {
    let r = row().key(key.to_owned()).h(LINE).center_y().transition(Transition::none());
    let Some(now) = around(&say(n), n) else { return r.child(text(say(n)).font(TNUM).color(ink)) };
    let then = was.filter(|&w| w != n).and_then(|w| around(&say(w), w));
    let words = |k: &'static str, s: &str, old: Option<&str>| {
        let t = |key: String, s: &str| text(s.to_owned()).key(key).font(TNUM).color(ink).transition(Transition::none());
        (!s.is_empty()).then(|| match old.filter(|o| *o != s && !o.is_empty()) {
            Some(o) => el()
                .key(format!("{k}-{nth}"))
                .transition(Transition::none())
                .child(t("was".into(), o).absolute(0.0, 0.0).sequence(Sequence::new(UNDER)))
                .child(t("now".into(), s).sequence(Sequence::new(SHOW))),
            None => t(k.into(), s),
        })
    };
    r.children(words("before", &now.before, then.as_ref().map(|t| t.before.as_str())).map(|w| w.mr(now.gaps.0)))
        .child(number(n, was, nth, ink))
        .children(words("after", &now.after, then.as_ref().map(|t| t.after.as_str())).map(|w| w.ml(now.gaps.1)))
}

/// `n` in tabular digits; moving from `was` (move `nth`), the digits that changed roll up
/// through a soft edge as it rises and down as it falls, and all of them when it gains or loses one.
fn number(n: usize, was: Option<usize>, nth: u64, ink: Color) -> El {
    let now = n.to_string();
    let digit = |k: String, s: &str| text(s.to_owned()).key(k).font(TNUM).color(ink).transition(Transition::none());
    let Some(was) = was.filter(|&w| w != n) else { return digit("number".into(), &now) };
    let before = was.to_string();
    let (enter, leave) = if n > was { (IN_UP, OUT_UP) } else { (IN_DOWN, OUT_DOWN) };
    let roll = |k: String, old: &str, new: &str| {
        el().key(k)
            .transition(Transition::none())
            .child(digit("was".into(), old).absolute(0.0, 0.0).sequence(Sequence::new(leave)))
            .child(digit("now".into(), new).sequence(Sequence::new(enter)))
    };
    let mut r = row()
        .key(format!("number-{nth}"))
        .h(LINE + 2.0 * EDGE)
        .py(EDGE)
        .mt(-EDGE)
        .mb(-EDGE)
        .center_y()
        .clip_fade(0.0, EDGE, 0.0, EDGE)
        .transition(Transition::none());
    if before.len() == now.len() {
        for (i, (a, b)) in before.chars().zip(now.chars()).enumerate() {
            r = r.child(if a == b { digit(format!("d{i}"), &b.to_string()) } else { roll(format!("d{i}"), &a.to_string(), &b.to_string()) });
        }
    } else {
        r = r.child(roll("all".into(), &before, &now));
    }
    r
}

/// An open line's agents, a row each in its latest event (`event_row::subagent`), which a press
/// opens as a subagent's row does.
pub fn list<S: TimelineHost>(s: &S, p: &Pal, session: &str, agents: &[Agent]) -> El {
    let mut c = col().key("agents").w_full().gap(ROW_GAP);
    for a in agents {
        c = c.child(el().key(format!("agent-{:x}", a.id)).w_full().child(super::event_row::subagent(s, p, session, &a.event)));
    }
    c
}
