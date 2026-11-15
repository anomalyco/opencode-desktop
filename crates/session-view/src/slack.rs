//! Slack permalinks in a prompt (Desktop's prototype `message/slack-link.tsx`, `message-part.css`'s
//! `user-message-slack-*`). Each reads as a chip in the bubble: `#channel` once the server has read
//! the message with the reader's own Slack access, else "Slack message". A found one also gets a
//! card under the bubble: author, channel and time, its first three lines and its replies. Both
//! open the message in the browser. The host reads each permalink once
//! (`TimelineHost::slack_link`, usually through a [`Links`]) and keeps the answer for its life.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use opencode_kit::slack::{Lookup, Message, Reason};
use rune::machine::Effects;
use rune::{El, Icon, PillStyle, SurfaceFit, TextStyle, Tx, Waker, col, el, icon, row, text};

use crate::TimelineHost;
use crate::palette::Pal;
use crate::style::{HOVER_FADE, UI, UI_MED};

/// Where one permalink's preview stands.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// No bubble naming it has been built yet.
    Idle,
    Loading,
    Found(Rc<Message>),
    Unavailable(Reason),
}

pub enum Event {
    /// A bubble naming the permalink was built.
    Shown,
    /// The server's answer; a request that failed answers `Unavailable(Reason::Failed)`.
    Answered(Lookup),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Ask the server about the permalink.
    Fetch,
    /// The bubbles naming it build again.
    Redraw,
}

/// The first bubble that names a permalink asks for it once; the answer stays for good, a
/// failure included, so nothing asks again.
pub fn step(state: &mut State, event: Event) -> Effects<Effect> {
    match (&*state, event) {
        (State::Idle, Event::Shown) => {
            *state = State::Loading;
            Effects::one(Effect::Fetch)
        }
        (State::Loading, Event::Answered(lookup)) => {
            *state = match lookup {
                Lookup::Found(message) => State::Found(Rc::from(message)),
                Lookup::Unavailable(reason) => State::Unavailable(reason),
            };
            Effects::one(Effect::Redraw)
        }
        // Asked already, or answered for good.
        (State::Loading | State::Found(_) | State::Unavailable(_), Event::Shown)
        | (State::Idle | State::Found(_) | State::Unavailable(_), Event::Answered(_)) => Effects::none(),
    }
}

/// Every permalink's state by URL, kept for its holder's life. Building a bubble asks through
/// [`Links::shown`]; the host runs the fetches [`Links::take_fetches`] hands it and gives each
/// answer to [`Links::answer`].
#[derive(Default)]
pub struct Links {
    links: RefCell<HashMap<String, State>>,
    fetches: RefCell<Vec<String>>,
    version: Cell<u64>,
    waker: Option<Waker>,
}

impl Links {
    /// Wakes the host when a bubble asks for a permalink, so its poll starts the fetch.
    pub fn set_waker(&mut self, waker: Waker) {
        self.waker = Some(waker);
    }

    /// Permalink `url`'s state for a bubble being built; the first ask starts its fetch.
    pub fn shown(&self, url: &str) -> State {
        let mut links = self.links.borrow_mut();
        let state = links.entry(url.to_owned()).or_insert(State::Idle);
        let effects = step(state, Event::Shown);
        self.run(url, effects);
        state.clone()
    }

    /// Permalink `url`'s state without asking for it, for a reader other than its bubble (Find).
    pub fn peek(&self, url: &str) -> State {
        self.links.borrow().get(url).cloned().unwrap_or(State::Idle)
    }

    /// The permalinks to ask the server about now.
    pub fn take_fetches(&self) -> Vec<String> {
        std::mem::take(&mut *self.fetches.borrow_mut())
    }

    /// The server's answer for `url`: true when the bubbles naming it must build again.
    pub fn answer(&mut self, url: &str, lookup: Lookup) -> bool {
        let Some(state) = self.links.get_mut().get_mut(url) else { return false };
        let effects = step(state, Event::Answered(lookup));
        self.run(url, effects)
    }

    /// Changes whenever an answer lands, for the rows that show permalinks to key on.
    pub fn version(&self) -> u64 {
        self.version.get()
    }

    fn run(&self, url: &str, effects: Effects<Effect>) -> bool {
        let mut redraw = false;
        for effect in effects {
            match effect {
                Effect::Fetch => {
                    self.fetches.borrow_mut().push(url.to_owned());
                    if let Some(wake) = &self.waker {
                        wake();
                    }
                }
                Effect::Redraw => {
                    self.version.set(self.version.get() + 1);
                    redraw = true;
                }
            }
        }
        redraw
    }
}

/// Slack's logo (`SlackLogo`) as an icon's body, its ink at `opacity`.
macro_rules! logo {
    ($opacity:literal) => {
        concat!(r#"<path fill="currentColor" fill-opacity=""#, $opacity, r#"" d="M5.042 15.165a2.53 2.53 0 0 1-2.52 2.523A2.53 2.53 0 0 1 0 15.165a2.527 2.527 0 0 1 2.522-2.52h2.52zm1.271 0a2.527 2.527 0 0 1 2.521-2.52 2.527 2.527 0 0 1 2.521 2.52v6.313A2.53 2.53 0 0 1 8.834 24a2.53 2.53 0 0 1-2.521-2.522zM8.834 5.042a2.53 2.53 0 0 1-2.521-2.52A2.53 2.53 0 0 1 8.834 0a2.53 2.53 0 0 1 2.521 2.522v2.52zm0 1.271a2.53 2.53 0 0 1 2.521 2.521 2.53 2.53 0 0 1-2.521 2.521H2.522A2.53 2.53 0 0 1 0 8.834a2.53 2.53 0 0 1 2.522-2.521zm10.122 2.521a2.53 2.53 0 0 1 2.522-2.521A2.53 2.53 0 0 1 24 8.834a2.53 2.53 0 0 1-2.522 2.521h-2.522zm-1.268 0a2.53 2.53 0 0 1-2.523 2.521 2.527 2.527 0 0 1-2.52-2.521V2.522A2.527 2.527 0 0 1 15.165 0a2.53 2.53 0 0 1 2.523 2.522zm-2.523 10.122a2.53 2.53 0 0 1 2.523 2.522A2.53 2.53 0 0 1 15.165 24a2.527 2.527 0 0 1-2.52-2.522v-2.522zm0-1.268a2.527 2.527 0 0 1-2.52-2.523 2.526 2.526 0 0 1 2.52-2.52h6.313A2.527 2.527 0 0 1 24 15.165a2.53 2.53 0 0 1-2.522 2.523z"/>"#)
    };
}

/// Slack's logo in its element's colour.
pub static LOGO: Icon = Icon { name: "slack-logo", view: 24.0, body: logo!("1") };

/// The chip's logo: a pill draws its icon in its label's colour, so the logo's `text-muted` over
/// the label's `text-base` is that colour at 0.6 (0.60 on the dark theme's `layer-03`, 0.68 on
/// the light's).
static CHIP_LOGO: Icon = Icon { name: "slack-logo-chip", view: 24.0, body: logo!("0.6") };

/// What a chip's `on_link` id starts with; the permalink follows.
const LINK: &str = "slack:";

/// The `on_link` id of a permalink's chip, and back.
pub fn link(url: &str) -> String {
    format!("{LINK}{url}")
}

pub fn url_of_link(link: &str) -> Option<&str> {
    link.strip_prefix(LINK)
}

/// Whether `body` may name a Slack permalink: a cheap look before [`opencode_kit::slack::permalinks`].
pub fn any(body: &str) -> bool {
    body.contains(".slack.com/archives/")
}

/// A chip's words: the channel once the message is found in one, else "Slack message".
pub fn label(state: &State) -> String {
    match state {
        State::Found(m) => match &m.channel.name {
            Some(name) => format!("#{name}"),
            None => opencode_i18n::t!("rune.slack.message").to_owned(),
        },
        State::Idle | State::Loading | State::Unavailable(_) => opencode_i18n::t!("rune.slack.message").to_owned(),
    }
}

/// The chip (`user-message-slack-chip`): base ink on `layer-03`, the logo leading in muted ink.
pub fn chip_style(p: &Pal) -> PillStyle {
    PillStyle::new().bg(p.layer3).fg(p.text).icon(&CHIP_LOGO)
}

/// A bubble's text with its Slack permalinks drawn as chips ([`chips`]).
pub struct Chips {
    pub text: String,
    /// Each chip's range in `text`, and its permalink.
    pub chips: Vec<(Range<usize>, String)>,
    /// Where each of the ranges [`chips`] kept clear lands in `text`.
    pub taken: Vec<Range<usize>>,
}

/// `text` with each Slack permalink in it outside `taken` drawn as its chip's words, `state`
/// saying where each permalink stands.
pub fn chips(text: &str, taken: &[Range<usize>], state: impl Fn(&str) -> State) -> Chips {
    let found: Vec<Range<usize>> = if any(text) {
        opencode_kit::slack::permalinks(text).into_iter().filter(|r| !taken.iter().any(|t| t.start < r.end && r.start < t.end)).collect()
    } else {
        Vec::new()
    };
    if found.is_empty() {
        return Chips { text: text.to_owned(), chips: Vec::new(), taken: taken.to_vec() };
    }
    let (mut out, mut chips, mut at) = (String::with_capacity(text.len()), Vec::new(), 0);
    // Where byte `i` of `text` lands in `out`: shifted by the chips before it.
    let mut shifts: Vec<(usize, isize)> = Vec::new();
    for r in found {
        out.push_str(&text[at..r.start]);
        let url = &text[r.clone()];
        let words = label(&state(url));
        let start = out.len();
        out.push_str(&words);
        chips.push((start..out.len(), url.to_owned()));
        shifts.push((r.end, out.len() as isize - r.end as isize));
        at = r.end;
    }
    out.push_str(&text[at..]);
    let shift = |i: usize| shifts.iter().rev().find(|(end, _)| *end <= i).map_or(i, |(_, d)| (i as isize + d) as usize);
    let taken = taken.iter().map(|r| shift(r.start)..shift(r.end)).collect();
    Chips { text: out, chips, taken }
}

/// The card's text (`slack-card-text`): 14/20.
const BODY: TextStyle = TextStyle::new(14.0, 440.0).lh(20.0);
/// The avatar's letter: 10 at 530.
const INITIAL: TextStyle = TextStyle::new(10.0, 530.0).lh(16.0);
/// `max-width` of a card.
const CARD_W: f32 = 360.0;

/// The cards under a prompt's bubble that names permalinks: one for each found message, in the
/// order they're named; `None` when it names none.
pub fn cards<S: TimelineHost>(s: &S, p: &Pal, body: &str) -> Option<Vec<El>> {
    if !any(body) {
        return None;
    }
    let mut seen: Vec<&str> = Vec::new();
    let mut out = Vec::new();
    for r in opencode_kit::slack::permalinks(body) {
        let url = &body[r];
        if seen.contains(&url) {
            continue;
        }
        seen.push(url);
        if let State::Found(m) = s.slack_link(url) {
            out.push(card(s, p, url, &m).key(format!("slack-{}", out.len())));
        }
    }
    Some(out)
}

/// A found message's card (`user-message-slack-card`): `layer-01` with a hairline in
/// `border-base` that `border-strong` replaces under the pointer, 10 by 12 inside, 10 pt corners.
fn card<S: TimelineHost>(s: &S, p: &Pal, url: &str, m: &Message) -> El {
    let time = m.sent_ms().map(|at| sent(s, at));
    let place = match (&m.channel.name, time) {
        (Some(name), Some(time)) => format!("#{name} \u{00b7} {time}"),
        (Some(name), None) => format!("#{name}"),
        (None, Some(time)) => time,
        (None, None) => String::new(),
    };
    let head = row()
        .key("head")
        .w_full()
        .gap(6.0)
        .center_y()
        .min_w(0.0)
        .child(avatar(s, p, m))
        .child(text(m.author.name.clone()).key("author").font(UI_MED).color(p.text).single_line())
        .child(text(place).key("where").font(UI).color(p.faint).truncate());
    let replies = opencode_i18n::plural!("rune.slack.replies", m.reply_count);
    let foot = row().key("foot").gap(6.0).center_y().child(icon(&LOGO, 11.0).color(p.muted)).child(text(replies).font(UI).color(p.muted).single_line());
    let ring = el().key("ring").absolute(0.0, 0.0).w_full().h_full().radius(10.0).border(0.5, p.q.border_strong).group_opacity(0.0, 1.0);
    let href = url.to_owned();
    col()
        .w_full()
        .max_w(CARD_W)
        .gap(6.0)
        .px(12.0)
        .py(10.0)
        .radius(10.0)
        .fill(p.layer1)
        .border(0.5, p.border)
        .hover_group()
        .hover_motion(HOVER_FADE, HOVER_FADE)
        .role("link")
        .label(opencode_i18n::t!("rune.slack.card", author = m.author.name, text = m.text))
        .on_press_tx(move |s: &mut S, tx: &mut Tx| s.open_link(&href, tx))
        .child(head)
        .child(text(m.text.clone()).key("text").font(BODY).color(p.text).w_full().max_lines(3))
        .child(foot)
        .child(ring)
}

/// The author's picture once it has loaded, else their initial (`slack-card-avatar`): 16 pt
/// with 4 pt corners.
fn avatar<S: TimelineHost>(s: &S, p: &Pal, m: &Message) -> El {
    let tile = el().key("avatar").size(16.0, 16.0).radius(4.0).fill(p.layer3).clip_rounded(4.0);
    let picture = m.author.avatar_url.as_deref().and_then(|url| s.media_store().markdown.image(None, url)).filter(|i| i.is_ready() && i.error().is_none());
    if let Some(picture) = picture {
        return tile.child(el().size(16.0, 16.0).pixels_fit(picture.pixels(), SurfaceFit::Fill).transition(rune::Transition::none()));
    }
    let initial: String = m.author.name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
    row().key("avatar").size(16.0, 16.0).center().radius(4.0).fill(p.layer3).child(text(initial).font(INITIAL).color(p.muted))
}

/// When a message was sent: its time on the day it is now, with its day before then, and its
/// year before this one.
fn sent<S: TimelineHost>(s: &S, at: i64) -> String {
    use opencode_i18n::date::{Style, format};
    let (then, now) = (rune::LocalTime::at(at), rune::LocalTime::at(s.now_ms()));
    let style = if (then.year, then.month, then.day) == (now.year, now.month, now.day) {
        Style::Time
    } else if then.year == now.year {
        Style::MonthDayTime
    } else {
        Style::MonthDayYearTime
    };
    format(style, crate::turn_errors::moment(then))
}
