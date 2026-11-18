//! Desktop's tab hover preview (`tab-popover.tsx`, `tab-popover-machine.ts`, `tab-popover.css`):
//! resting on a session tab for 750 ms shows a card with its project, its whole title and its
//! folder; once one has shown, the next tab's card shows at once while the pointer hops across
//! tabs within 500 ms. It comes and goes without animating, under a top tab (`bottom-start`, 6
//! away) or beside a side tab (`right-start`, 6 away), following the pointer down the side tabs.
//! A drag, a press, a rename or the tab's menu keeps it away.
//!
//! Two machines, as in Desktop: [`Card`] is Kobalte's `HoverCard` on the hovered tab with the
//! tab's `popoverOpen` (`tab-nav.tsx:182-185, 535-550`), and [`State`] is the preview shared by
//! every strip (`TabPreviewState`), which hears the card open and close.

use rune::machine::Attempt;

use super::*;

/// The hover before a card first shows (`TAB_PREVIEW_OPEN_DELAY_MS`).
const OPEN_DELAY: f64 = 0.75;
/// After a card closes, the next tab's opens at once within this (`TAB_PREVIEW_SKIP_WINDOW_MS`).
const SKIP_WINDOW: f64 = 0.5;
/// The card (`width: 256px; padding: 12px; gap: 4px; border-radius: 6px`).
const W: f32 = 256.0;
// The card's `font-variant-numeric: tabular-nums` reaches every line.
const PROJECT: TextStyle = TextStyle::new(13.0, 440.0).lh(19.5).tracking(-0.04).features("\"tnum\"");
const TITLE: TextStyle = TextStyle::new(13.0, 530.0).lh(19.5).tracking(-0.04).features("\"tnum\"");
/// The server's line (`[data-slot="server"]`: 13 at 440 on `--line-height-compact`, 16).
const SERVER: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04).features("\"tnum\"");

/// `TabPreviewState`: `cold` waits the full open delay; `open` and `warm` (just closed) open the
/// next preview at once. `instant` marks a preview opened during such a streak (Desktop's card
/// has no entrance either way, `tab-popover.css:28-45`).
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Cold { attempt: Attempt },
    /// `pointer`: over a side tab, the pointer's height in window points and the row's top,
    /// which the card follows (`follow`, `pointerY`).
    Open { attempt: Attempt, tab: String, instant: bool, pointer: Option<(f32, f32)> },
    /// Closed a moment ago, until `until` on the app's clock (`warm.start`).
    Warm { attempt: Attempt, until: f64, instant: bool },
}

impl Default for State {
    fn default() -> State {
        State::Cold { attempt: Attempt(0) }
    }
}

/// `TabPreviewEvent`. `Closed` is also `owner.disposed`; `Pointer` stands in for `follow.attached`
/// and the trigger's `pointermove`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event<'a> {
    Opened(&'a str),
    Closed(&'a str),
    Pointer { tab: &'a str, at: (f32, f32) },
    WarmExpired { attempt: Attempt },
}

/// `transitionTabPreview`. The warm timer is the `until` the state carries, so `warm.start`
/// and `warm.clear` have nothing left to do.
pub fn step(state: &mut State, event: Event<'_>, now: f64) {
    match event {
        Event::Opened(tab) => {
            if matches!(state, State::Open { tab: t, .. } if t == tab) {
                return;
            }
            let attempt = attempt(state);
            let instant = !matches!(state, State::Cold { .. });
            // Another tab's preview takes over at once and closes the one that was up.
            *state = State::Open { attempt, tab: tab.to_owned(), instant, pointer: None };
        }
        Event::Closed(tab) => {
            let State::Open { attempt, tab: t, instant, .. } = state else { return };
            if t != tab {
                return;
            }
            *state = State::Warm { attempt: Attempt(attempt.0 + 1), until: now + SKIP_WINDOW, instant: *instant };
        }
        Event::Pointer { tab, at } => {
            if let State::Open { tab: t, pointer, .. } = state
                && t == tab
            {
                *pointer = Some(at);
            }
        }
        Event::WarmExpired { attempt: due } => {
            if let State::Warm { attempt, .. } = state
                && *attempt == due
            {
                *state = State::Cold { attempt: due };
            }
        }
    }
}

fn attempt(state: &State) -> Attempt {
    match state {
        State::Cold { attempt } | State::Open { attempt, .. } | State::Warm { attempt, .. } => *attempt,
    }
}

/// `selectTabPreviewOpenDelay`: Kobalte reads it as the pointer enters a tab.
fn open_delay(state: &State) -> f64 {
    if matches!(state, State::Cold { .. }) { OPEN_DELAY } else { 0.0 }
}

/// Kobalte's `HoverCard` on the hovered tab, with that tab's `popoverOpen`.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Card {
    /// The pointer is on no tab.
    #[default]
    Away,
    /// The pointer rests on `tab` and its card is closed, refused or shut by its menu: it asks
    /// again only when the pointer comes back (Kobalte opens on `pointerenter`), however often
    /// the tab says the pointer is on it.
    Resting { tab: String },
    /// The pointer entered `tab`: its card asks to open at `at` (`openWithDelay`), unless the
    /// pointer leaves first (`cancelOpening`).
    Waiting { tab: String, at: f64 },
    /// `tab` took the card (`popoverOpen`): it shows while nothing blocks it.
    Asked { tab: String },
}

/// The card, and the preview it feeds.
#[derive(Default)]
pub struct Preview {
    card: Card,
    state: State,
}

impl App {
    /// The pointer came onto (`inside`) or left tab `id`.
    pub(crate) fn preview_hover(&mut self, id: &str, inside: bool, tx: &mut Tx) {
        let now = tx.clock();
        self.expire_preview(now);
        let shown = self.previewed().map(str::to_owned);
        // A draft has no session, so it never takes the card (`!props.session`).
        let draft = self.tab(id).is_some_and(crate::tabs::Tab::is_draft);
        let p = &mut *self.tab_preview;
        if !inside {
            // Leaving the tab cancels its opening, and the pointer off the tab and its card
            // closes it (`onHoverOutside`, `closeDelay` 0).
            if matches!(&p.card, Card::Waiting { tab, .. } | Card::Asked { tab } | Card::Resting { tab } if tab == id) {
                p.card = Card::Away;
            }
        } else if !matches!(&p.card, Card::Waiting { tab, .. } | Card::Asked { tab } | Card::Resting { tab } if tab == id) {
            p.card = if draft { Card::Resting { tab: id.to_owned() } } else { Card::Waiting { tab: id.to_owned(), at: now + open_delay(&p.state) } };
        }
        self.settle_preview(now, tx);
        // The strip draws its hover by hover groups, so only the card opening or closing changes
        // what the view draws; a hover that doesn't runs no view.
        if self.previewed() != shown.as_deref() {
            tx.instant();
        } else {
            tx.live_only();
        }
    }

    /// Over a side tab: the pointer's height, which the card follows.
    pub(crate) fn preview_pointer(&mut self, id: &str, local_y: f32, window_y: f32, tx: &mut Tx) {
        if !matches!(&self.tab_preview.state, State::Open { tab, .. } if tab == id) {
            tx.live_only();
            return;
        }
        let at = (window_y, window_y - local_y);
        step(&mut self.tab_preview.state, Event::Pointer { tab: id, at }, tx.clock());
        tx.instant();
    }

    /// What keeps tab `id`'s card away (`previewBlocked`): a drag or press in the strip, its
    /// rename, its menu.
    fn preview_blocked(&self, id: &str) -> bool {
        self.tab_drag.phase() != rune_kit::shell::TabDragPhase::Idle
            || self.menus.renaming().is_some_and(|t| self.names(&t, id))
            || self.menus.on.as_ref().is_some_and(|t| self.names(t, id))
    }

    /// Whether a menu or rename target is tab `id` (`Tab::uid`).
    fn names(&self, target: &crate::session_menu::Target, id: &str) -> bool {
        target.tab(self).is_some_and(|t| t.uid == id)
    }

    /// Whether tab `id`'s card shows once it has it (`open() && !previewBlocked()`): a narrow
    /// window's tabs show none (`tab-popover.tsx:87`).
    fn preview_open(&self, id: &str) -> bool {
        !self.preview_blocked(id) && !self.narrow()
    }

    /// The tab whose card shows now.
    fn previewed(&self) -> Option<&str> {
        let State::Open { tab, .. } = &self.tab_preview.state else { return None };
        (self.preview_open(tab) && self.tab_layout() != crate::layout_state::TabLayout::Inbox).then_some(tab.as_str())
    }

    fn expire_preview(&mut self, now: f64) {
        if let State::Warm { attempt, until, .. } = self.tab_preview.state
            && now + 1e-9 >= until
        {
            step(&mut self.tab_preview.state, Event::WarmExpired { attempt }, now);
        }
    }

    /// The card's opening comes due, a menu takes the tab, and the preview hears whether the
    /// card is open (`createComputed` over `open() && !previewBlocked()`).
    fn settle_preview(&mut self, now: f64, tx: &mut Tx) {
        if let Card::Waiting { tab, at } = &self.tab_preview.card {
            if now + 1e-3 < *at {
                tx.wake_at(*at);
            } else {
                // The card asks to open; a blocked tab refuses, and the card doesn't ask again
                // until the pointer comes back (`if (value && previewBlocked()) return`).
                let tab = tab.clone();
                self.tab_preview.card = if self.preview_blocked(&tab) { Card::Resting { tab } } else { Card::Asked { tab } };
            }
        }
        // Its menu opening closes the tab's card (`if (open) setPopoverOpen(false)`).
        if let Card::Asked { tab } = &self.tab_preview.card
            && self.menus.on.as_ref().is_some_and(|t| self.names(t, tab))
        {
            self.tab_preview.card = Card::Resting { tab: tab.clone() };
        }
        let open = match &self.tab_preview.card {
            Card::Asked { tab } if self.preview_open(tab) => Some(tab.clone()),
            _ => None,
        };
        // Every poll comes here: the preview is written only when the card opens or closes.
        match (&self.tab_preview.state, open) {
            (State::Open { tab, .. }, None) => {
                let tab = tab.clone();
                step(&mut self.tab_preview.state, Event::Closed(&tab), now);
            }
            (State::Open { tab: shown, .. }, Some(tab)) if *shown == tab => {}
            (_, Some(tab)) => step(&mut self.tab_preview.state, Event::Opened(&tab), now),
            _ => {}
        }
    }
}

/// `poll`: the warm streak ends, a card's opening comes due, and blocks come and go.
pub(crate) fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    s.expire_preview(now);
    let shown = s.previewed().map(str::to_owned);
    s.settle_preview(now, tx);
    let changed = s.previewed() != shown.as_deref();
    if changed {
        tx.instant();
    }
    changed
}

/// The card, at the window's root.
pub(crate) fn view(s: &App, p: &Pal) -> Option<El> {
    let id = s.previewed()?;
    let tab = s.tab(id)?;
    // Its session, on its own server (`props.session`): none, no card.
    let face = super::tab_strip::face(s, tab);
    let title = face.session_title?;
    let dir = if face.dir.is_empty() { tab.directory.as_str() } else { face.dir };
    let vertical = s.tab_layout().has_sidebar();
    let side = if vertical { Side::Right } else { Side::Bottom };
    let (project, title) = (project_or_dir(face.project, dir), if title.is_empty() { opencode_i18n::t!("rune.session.untitled").to_owned() } else { title.to_owned() });
    // Paths break after their separators, not inside a folder's name (`<wbr>`).
    let detail = breakable(&s.tilde(&tab.server, dir));
    // The server, only while several are listed and it isn't the app's own (`serverLabel`).
    let server = crate::servers::runtime::tag(&tab.server);
    let card_h = vertical.then(|| card_height(&project, &title, &detail, server.as_deref()));
    let mut card = col()
        .key("tab-preview")
        .a11y_hidden()
        .anchored(format!("t-{id}"), side, Align::Start, 6.0)
        .w(W)
        .padding(PAD)
        .gap(GAP)
        .radius(6.0)
        .fill(p.bg_base)
        .elevate(p.floating)
        .transition(rune::Transition::none())
        .motion(Spring::NONE)
        .child(
            col().key("header").w_full().gap(GAP)
                .child(text(project).key("project-name").font(PROJECT).color(p.muted))
                .child(text(title).key("title").font(TITLE).color(p.text).wrap()),
        )
        .child(row().key("row").w_full().min_w(0.0).child(text(detail).key("detail").font(PROJECT).color(p.muted).wrap().grow(1.0).shrink().min_w(0.0)))
        .children(server.map(|name| text(name).key("server").font(SERVER).color(p.muted).w_full().truncate()));
    let pointer = match &s.tab_preview.state {
        State::Open { pointer, .. } => *pointer,
        _ => None,
    };
    if let (Some(card_h), Some((y, top))) = (card_h, pointer) {
        // The pointer stays level with the middle of a row-sized slot, clamped to the strip
        // (`cursorOffset`), in the view's points; the strip is kept in the window's.
        let row_h = 28.0;
        let (lo, hi) = s.window.strip.map_or((f32::MIN, f32::MAX), |r| (r[1] / *s.zoom, (r[1] + r[3]) / *s.zoom));
        let clamped = (y - row_h / 2.0).min(hi - card_h).max(lo);
        card = card.offset(0.0, clamped - top).offset_motion(Spring::NONE);
    }
    Some(card)
}

/// The card's padding and the gap between its lines.
const PAD: f32 = 12.0;
const GAP: f32 = 4.0;

/// How tall the card lays out, measured as the runtime sets its lines at the card's inner width
/// (Desktop's `card.offsetHeight`), so the clamp to the strip is exact from its first frame.
fn card_height(project: &str, title: &str, detail: &str, server: Option<&str>) -> f32 {
    let inner = W - 2.0 * PAD;
    let height = |t: &str, style, wrap: bool| rune::text::measure(t, &[], style, wrap.then_some(inner)).1;
    let server = server.map_or(0.0, |name| GAP + height(name, SERVER, false));
    2.0 * PAD + height(project, PROJECT, false) + GAP + height(title, TITLE, true) + GAP + height(detail, PROJECT, true) + server
}

/// `path` with a break allowed after each separator.
fn breakable(path: &str) -> String {
    path.chars().flat_map(|c| if c == '/' || c == '\\' { vec![c, '\u{200B}'] } else { vec![c] }).collect()
}
