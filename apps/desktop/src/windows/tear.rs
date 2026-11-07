//! Tearing a tab out into a window of its own. Desktop's strip keeps a drag inside it, so this
//! is the browser's gesture, carried out in Kit's motion: pull a tab off the strip and a window
//! opens with it under the pointer, the tab where the pointer holds it, and follows the pointer
//! while the button stays down. Bring it back to the strip and the window goes; let go over
//! another window's strip and the tab moves into that strip instead; let go anywhere else and
//! the new window stays, in front. The window opens inactive, so the strip that holds the press
//! keeps it to the end.

use rune::{AppHandle, Tx, WindowHandle};
use rune_kit::shell::{TabTearMove, TabTearOut};

use super::open::{Seed, open};
use super::store::{self, Frame};
use super::{OPEN, Window};
use crate::App;
use crate::tabs::Tab;

/// A tab on its way out of this window.
#[derive(Default)]
pub enum Tear {
    #[default]
    Idle,
    Out(Out),
}

pub struct Out {
    tab: Tab,
    /// The new window's id and handle.
    id: String,
    handle: WindowHandle,
    /// The pointer from the new window's top-left, in desktop pixels.
    offset: (i32, i32),
    /// Hidden while the pointer is over another window's strip, where the tab would land.
    hidden: bool,
}

/// Another window's strip under a desktop point: its handle, its app, and the point in its
/// window points.
struct Target {
    handle: WindowHandle,
    app: AppHandle<App>,
    at: [f32; 2],
}

impl App {
    /// The strip's tear-out gesture, on each move while the button is down.
    pub(crate) fn tear_move(&mut self, m: TabTearMove, tx: &mut Tx) {
        let Some(point) = m.screen_point else { return };
        match (matches!(self.window.tear, Tear::Out(_)), m.torn) {
            (false, true) => self.tear_open(&m, point),
            (true, true) => self.tear_follow(point),
            (true, false) => self.tear_back(),
            (false, false) => {}
        }
        tx.live_only();
    }

    /// The torn tab's window under the pointer; hidden while the pointer is over another
    /// window's strip, where letting go would put the tab.
    fn tear_follow(&mut self, point: (i32, i32)) {
        let me = self.window.id.clone();
        let Tear::Out(out) = &mut self.window.tear else { return };
        out.handle.set_position((point.0 - out.offset.0, point.1 - out.offset.1));
        let over = target(point, &[&me, &out.id]).is_some();
        if over != out.hidden {
            out.hidden = over;
            if over { out.handle.hide() } else { out.handle.show() }
        }
    }

    /// The button came up with the tab off the strip.
    pub(crate) fn tear_release(&mut self, t: TabTearOut, tx: &mut Tx) {
        let Some(point) = t.screen_point else { return };
        let out = match std::mem::take(&mut self.window.tear) {
            Tear::Out(out) => out,
            // No window followed the pointer: open one where it let go.
            Tear::Idle => {
                let Some(tab) = self.tabs.iter().find(|x| x.id == t.id).cloned().filter(|_| self.tabs.len() > 1) else { return };
                if let Some(target) = target(point, &[&self.window.id]) {
                    return self.move_tab_to(tab, target, tx);
                }
                let Some((frame, _)) = self.torn_frame(point, [0.0, 0.0]) else { return };
                let seed = Seed::from(self, vec![tab.clone()], tab.route());
                if open(self, seeded(seed), Some(frame), false).is_some() {
                    self.tab_moved_out(&tab.id, tx);
                }
                return;
            }
        };
        match target(point, &[&self.window.id, &out.id]) {
            Some(target) => {
                out.handle.close();
                self.move_tab_to(out.tab, target, tx);
            }
            None => {
                out.handle.show();
                out.handle.focus();
                self.tab_moved_out(&out.tab.id, tx);
            }
        }
    }

    /// `poll`: a gesture that ended without a release (Escape, the window losing focus) takes
    /// its window back.
    pub(crate) fn poll_tear(&mut self) {
        if matches!(self.window.tear, Tear::Out(_)) && !self.tab_drag.torn_out() {
            self.tear_back();
        }
    }

    /// The tab is back on the strip: its window goes.
    fn tear_back(&mut self) {
        if let Tear::Out(out) = std::mem::take(&mut self.window.tear) {
            out.handle.close();
        }
    }

    /// Pulled off the strip: a window opens with the tab, under the pointer, inactive, as wide
    /// and tall as this one.
    fn tear_open(&mut self, m: &TabTearMove, point: (i32, i32)) {
        if self.tabs.len() < 2 || !self.persists() {
            return;
        }
        let Some(tab) = self.tabs.iter().find(|t| t.id == m.id).cloned() else { return };
        let Some((frame, offset)) = self.torn_frame(point, m.grab) else { return };
        let seed = Seed::from(self, vec![tab.clone()], tab.route());
        let w = seeded(seed);
        let id = w.id.clone();
        let Some(handle) = open(self, w, Some(frame), true) else { return };
        self.tab_drag.hand_off();
        self.window.tear = Tear::Out(Out { tab, id, handle, offset, hidden: false });
    }

    /// Where a window showing the torn tab goes so the pointer at `point` holds it at `grab`
    /// (points in the tab) as it held it here: the tab leads the new window's strip. And the
    /// pointer's offset from the window's top-left.
    fn torn_frame(&self, point: (i32, i32), grab: [f32; 2]) -> Option<(Frame, (i32, i32))> {
        let p = super::handle(&self.window.id)?.placement()?;
        let strip = self.window.strip.unwrap_or([0.0; 4]);
        let scale = p.scale as f32;
        let inset = (p.content_position.0 - p.position.0, p.content_position.1 - p.position.1);
        let x = ((strip[0] + grab[0] * self.zoom) * scale).round() as i32 + inset.0;
        let y = ((strip[1] + grab[1] * self.zoom) * scale).round() as i32 + inset.1;
        let frame = Frame { x: point.0 - x, y: point.1 - y, width: p.content_size.0, height: p.content_size.1, ..Frame::default() };
        Some((frame, (x, y)))
    }

    /// Sends `tab` to the window under the pointer, where it joins the strip, and drops it here.
    fn move_tab_to(&mut self, tab: Tab, target: Target, tx: &mut Tx) {
        let draft = self.draft_text(&tab.id);
        let id = tab.id.clone();
        let at = target.at;
        target.app.update(move |a, tx| a.receive_tab(tab, draft, at, tx));
        target.handle.focus();
        self.tab_moved_out(&id, tx);
    }

    /// A tab dropped onto this window's strip from another, at `at` (window points).
    fn receive_tab(&mut self, tab: Tab, draft: Option<String>, at: [f32; 2], tx: &mut Tx) {
        if let Some(text) = draft {
            self.restore.drafts.insert(tab.id.clone(), text);
        }
        if !self.tabs.iter().any(|t| t.id == tab.id) {
            let index = self.drop_index(at);
            self.tabs.insert(index, tab.clone());
        }
        self.select_tab(&tab.id, tx);
        self.save_tabs();
    }

    /// The strip place a point lands on: the slot whose middle it passed.
    fn drop_index(&self, at: [f32; 2]) -> usize {
        let n = self.tabs.len();
        let Some(r) = self.window.strip.filter(|_| n > 0) else { return n };
        let vertical = self.tab_layout().has_sidebar();
        let (from, span, most) = if vertical { (r[1], r[3], 31.0) } else { (r[0], r[2], 230.0) };
        let pitch = (span / n as f32).min(most * self.zoom).max(1.0);
        let along = if vertical { at[1] } else { at[0] };
        (((along - from) / pitch).round().max(0.0) as usize).min(n)
    }

    /// Tab `id` went to another window: it leaves this strip without a closed-tab entry, and its
    /// words go with it.
    fn tab_moved_out(&mut self, id: &str, tx: &mut Tx) {
        let shown = self.current_tab().as_deref() == Some(id);
        let Some(i) = self.tabs.iter().position(|t| t.id == id) else { return };
        self.tabs.remove(i);
        self.restore.drafts.remove(id);
        if shown {
            self.restore.shown = None;
            self.composer = rune::TextEdit::new("");
            match self.tabs.get(i).or_else(|| self.tabs.get(i.wrapping_sub(1))).map(|t| t.id.clone()) {
                Some(next) => self.select_tab(&next, tx),
                None => self.go_home(tx),
            }
        }
        self.save_tabs();
    }
}

/// A new window holding what `seed` moves.
fn seeded(seed: Seed) -> Window {
    let mut w = Window::new(store::new_id(), false);
    w.seed = Some(seed);
    w
}

/// The window, other than `skip`, whose strip is under desktop point `point`.
fn target(point: (i32, i32), skip: &[&str]) -> Option<Target> {
    let open: Vec<(String, WindowHandle, AppHandle<App>, Option<[f32; 4]>)> =
        OPEN.with(|o| o.borrow().iter().map(|w| (w.id.clone(), w.handle.clone(), w.app.clone(), w.strip)).collect());
    open.into_iter().filter(|(id, ..)| !skip.contains(&id.as_str())).find_map(|(_, handle, app, strip)| {
        let r = strip?;
        let p = handle.placement().filter(|p| !p.minimized)?;
        let s = p.scale as f32;
        let at = [(point.0 - p.content_position.0) as f32 / s, (point.1 - p.content_position.1) as f32 / s];
        // A little slack around the strip, as the strip's own reattach distance gives.
        let slack = 12.0;
        let inside = at[0] >= r[0] - slack && at[0] <= r[0] + r[2] + slack && at[1] >= r[1] - slack && at[1] <= r[1] + r[3] + slack;
        inside.then_some(Target { handle, app, at })
    })
}
