//! Desktop's tab hover preview (`tab-popover.tsx`, `tab-popover-machine.ts`, `tab-popover.css`):
//! resting on a session tab for 750 ms shows a card with its project, its whole title and its
//! folder; once one has shown, the next tab's card shows at once while the pointer hops across
//! tabs within 500 ms. It comes and goes without animating, under a top tab (`bottom-start`, 6
//! away) or beside a side tab (`right-start`, 6 away), following the pointer down the side tabs.
//! A drag, a press, a rename or the tab's menu keeps it away.

use super::*;

/// The hover before a card first shows (`TAB_PREVIEW_OPEN_DELAY_MS`).
const OPEN_DELAY: f64 = 0.75;
/// After a card closes, the next tab's opens at once within this (`TAB_PREVIEW_SKIP_WINDOW_MS`).
const SKIP_WINDOW: f64 = 0.5;
/// The card (`width: 256px; padding: 12px; gap: 4px; border-radius: 6px`).
const W: f32 = 256.0;
const PROJECT: TextStyle = TextStyle::new(13.0, 440.0).lh(19.5).tracking(-0.04);
const TITLE: TextStyle = TextStyle::new(13.0, 530.0).lh(19.5).tracking(-0.04);

/// The preview machine: one card at a time across the window's strips.
#[derive(Default)]
pub struct Preview {
    /// The session tab under the pointer, and when it came (`App::clock`).
    hovered: Option<(String, f64)>,
    /// The tab whose card shows.
    open: Option<String>,
    /// Until when the next tab's card shows at once.
    warm_until: f64,
    /// Over a side tab: the pointer's height in its row, and the row's top, in window points.
    pointer: Option<(f32, f32)>,
}

impl App {
    /// The pointer came onto (`inside`) or left tab `id`.
    pub(crate) fn preview_hover(&mut self, id: &str, inside: bool, tx: &mut Tx) {
        let now = self.clock;
        let p = &mut self.tab_preview;
        if !inside {
            if p.hovered.as_ref().is_some_and(|(h, _)| h == id) {
                p.hovered = None;
            }
            if p.open.as_deref() == Some(id) {
                p.open = None;
                p.warm_until = now + SKIP_WINDOW;
                tx.instant();
            } else {
                tx.live_only();
            }
            return;
        }
        if self.tabs.iter().any(|t| t.id == id && t.is_draft()) {
            tx.live_only();
            return;
        }
        let warm = p.open.is_some() || now < p.warm_until;
        p.hovered = Some((id.to_owned(), now));
        p.pointer = None;
        if warm {
            p.open = Some(id.to_owned());
            tx.instant();
        } else {
            tx.live_only();
            tx.wake_after(std::time::Duration::from_secs_f64(OPEN_DELAY));
        }
    }

    /// Over a side tab: the pointer's height, which the card follows.
    pub(crate) fn preview_pointer(&mut self, id: &str, local_y: f32, window_y: f32, tx: &mut Tx) {
        let p = &mut self.tab_preview;
        if p.open.as_deref() != Some(id) {
            tx.live_only();
            return;
        }
        p.pointer = Some((window_y, window_y - local_y * self.zoom));
        tx.instant();
    }

    /// The tab whose card shows now, unless something keeps it away.
    fn previewed(&self) -> Option<&str> {
        let id = self.tab_preview.open.as_deref()?;
        let blocked = self.tab_drag.phase != rune_kit::shell::TabDragPhase::Idle
            || self.menus.rename.as_ref().is_some_and(|t| t.id == id)
            || self.menus.on.as_ref().is_some_and(|t| t.id == id);
        (!blocked && self.tab_layout() != crate::layout_state::TabLayout::Inbox).then_some(id)
    }
}

/// `poll`: the cold delay ran out over the same tab.
pub(crate) fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    let p = &mut s.tab_preview;
    let Some((id, since)) = p.hovered.clone().filter(|_| p.open.is_none()) else { return false };
    let left = OPEN_DELAY - (now - since);
    if left > 1e-3 {
        tx.wake_after(std::time::Duration::from_secs_f64(left));
        return false;
    }
    p.open = Some(id);
    tx.instant();
    true
}

/// The card, at the window's root.
pub(crate) fn view(s: &App, p: &Pal) -> Option<El> {
    let id = s.previewed()?;
    let info = s.session(id)?;
    let tab = s.tabs.iter().find(|t| t.id == id)?;
    let dir = if info.directory.is_empty() { tab.directory.as_str() } else { info.directory.as_str() };
    let project = s.project_for_session(info).or_else(|| s.project_at(dir));
    let vertical = s.tab_layout().has_sidebar();
    let side = if vertical { Side::Right } else { Side::Bottom };
    let (project, title) = (project_or_dir(project, dir), title_of(info));
    // Paths break after their separators, not inside a folder's name (`<wbr>`).
    let detail = breakable(&s.tilde(dir));
    let card_h = vertical.then(|| card_height(&project, &title, &detail));
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
                .child(text(project).key("project").font(PROJECT).color(p.muted))
                .child(text(title).key("title").font(TITLE).color(p.text).wrap()),
        )
        .child(row().key("row").w_full().child(text(detail).key("detail").font(PROJECT).color(p.muted).wrap()));
    if let (Some(card_h), Some((y, top))) = (card_h, s.tab_preview.pointer) {
        // The pointer stays level with the middle of a row-sized slot, clamped to the strip
        // (`cursorOffset`), in window points; the offset is the card's own.
        let (row_h, card_h, z) = (28.0 * s.zoom, card_h * s.zoom, s.zoom);
        let (lo, hi) = s.window.strip.map_or((f32::MIN, f32::MAX), |r| (r[1], r[1] + r[3]));
        let clamped = (y - row_h / 2.0).min(hi - card_h).max(lo);
        card = card.offset(0.0, (clamped - top) / z).offset_motion(Spring::NONE);
    }
    Some(card)
}

/// The card's padding and the gap between its lines.
const PAD: f32 = 12.0;
const GAP: f32 = 4.0;

/// How tall the card lays out, measured as the runtime sets its lines at the card's inner width
/// (Desktop's `card.offsetHeight`), so the clamp to the strip is exact from its first frame.
fn card_height(project: &str, title: &str, detail: &str) -> f32 {
    let inner = W - 2.0 * PAD;
    let height = |t: &str, style, wrap: bool| rune::text::measure(t, &[], style, wrap.then_some(inner)).1;
    2.0 * PAD + height(project, PROJECT, false) + GAP + height(title, TITLE, true) + GAP + height(detail, PROJECT, true)
}

/// `path` with a break allowed after each separator.
fn breakable(path: &str) -> String {
    path.chars().flat_map(|c| if c == '/' || c == '\\' { vec![c, '\u{200B}'] } else { vec![c] }).collect()
}
