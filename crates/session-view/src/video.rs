//! Standalone movie links in assistant Markdown. The original link remains the caption and
//! fallback. No HTML or provider iframe is needed; the host resolves a source, Rune plays it.
mod seek;
mod transport;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Range;
use std::rc::{Rc, Weak};

use opencode_ui::video_overlay::{self, Effect, Event as OverlayEvent, Overlay};
use pulldown_cmark::{Event, Options, Parser, Tag};
use rune::{El, Player, PlayerState, Tx, col, el, text};

use crate::{TimelineHost, palette::Pal};

#[derive(Default)]
pub(crate) struct Clips {
    // Only row elements own players. Scrolling a row out of the memo cache releases it.
    players: RefCell<HashMap<(String, String, usize), Weak<Clip>>>,
    version: Cell<u64>,
}

#[derive(Clone, Copy, PartialEq)]
enum Face { Paused, Playing, Failed }

struct Embed {
    end: usize,
    source: String,
    href: String,
    name: String,
}

/// Only discrete changes invalidate a memoized message. Progress and pointer samples
/// paint through Live bindings. Expose a fresh slider value to assistive clients each second.
#[derive(Clone, Copy, PartialEq)]
struct Presentation {
    face: Face,
    size: Option<(u32, u32)>,
    length: Option<f64>,
    shown: bool,
    inspecting: bool,
    compact: bool,
    accessible_second: Option<u64>,
}

struct Clip {
    player: Player,
    overlay: Cell<Overlay>,
    reported: Cell<Option<Presentation>>,
    track: seek::Track,
}

impl Clip {
    fn face(&self) -> Face {
        if matches!(self.player.state(), PlayerState::Failed(_)) { Face::Failed }
        else if self.player.is_playing() { Face::Playing }
        else { Face::Paused }
    }

    fn observe(&self, event: OverlayEvent, tx: &mut Tx) -> bool {
        let mut state = self.overlay.get();
        let was = state.shown();
        if let Some(Effect::WakeAt(due)) = video_overlay::step(&mut state, event) { tx.wake_at(due); }
        self.overlay.set(state);
        was != state.shown()
    }

    fn shown(&self) -> bool {
        self.overlay.get().shown() || self.track.dragging() || crate::style::hover_none()
    }

    fn presentation_changed(&self, tx: &mut Tx) -> bool {
        let next = Presentation {
            face: self.face(), size: self.player.natural_size(), length: self.player.duration(),
            inspecting: self.track.dragging(), shown: self.shown(), compact: self.track.compact(&self.player),
            accessible_second: tx.assistive_technology().then(|| self.track.position(&self.player) as u64),
        };
        let last = self.reported.replace(Some(next));
        if last == Some(next) { return false; }
        if last.is_some_and(|p| p.shown && !next.shown) { tx.animated(); } else { tx.instant(); }
        true
    }

    fn refresh<S: TimelineHost>(&self, s: &mut S, tx: &mut Tx) {
        if self.presentation_changed(tx) { s.media_store_mut().videos.invalidate(); }
        else { tx.live_only(); }
    }

    fn toggle(&self) {
        if self.player.is_playing() { self.player.pause(); } else { self.player.play(); }
    }
}

impl Clips {
    pub(crate) fn version(&self) -> u64 { self.version.get() }

    fn invalidate(&self) { self.version.set(self.version.get().wrapping_add(1)); }

    pub(crate) fn poll(&self, active: Option<&str>, tx: &mut Tx) -> bool {
        let mut changed = false;
        self.players.borrow_mut().retain(|(sid, _, _), weak| {
            let Some(c) = weak.upgrade() else { return false };
            if active != Some(sid) && c.player.is_playing() { c.player.pause(); }
            if active == Some(sid) {
                let now = tx.clock();
                let paused = !c.player.is_playing();
                if c.overlay.get() == Overlay::Loading {
                    if !matches!(c.player.state(), PlayerState::Loading) {
                        c.observe(OverlayEvent::Loaded { paused, now }, tx);
                    }
                } else {
                    c.observe(OverlayEvent::Playback { paused, now }, tx);
                    c.observe(OverlayEvent::Tick { now }, tx);
                }
                c.track.poll(&c.player);
                // When tucked away the movie's own frame clock suffices.
                if !paused && c.shown() { tx.wake_at(now + 1.0 / 60.0); }
            }
            changed |= c.presentation_changed(tx);
            true
        });
        if changed { self.invalidate(); }
        changed
    }

    fn view<S: TimelineHost>(&self, s: &S, p: &Pal, row: &str, embed: &Embed) -> El {
        let Embed { end: at, source, href, name } = embed;
        let key = (s.active_session_id().unwrap_or("").to_owned(), row.to_owned(), *at);
        let mut players = self.players.borrow_mut();
        let src = rune::MediaSource::from(source.as_str());
        let clip = players.get(&key).and_then(Weak::upgrade).filter(|c| c.player.source() == &src).unwrap_or_else(|| {
            let player = Player::new(src);
            player.set_muted(false);
            if let Some(wake) = &s.media_store().waker { player.set_waker(wake.clone()); }
            let c = Rc::new(Clip { player, overlay: Cell::default(), reported: Cell::default(), track: Default::default() });
            players.insert(key, Rc::downgrade(&c));
            c
        });
        let face = clip.face();
        if face == Face::Failed {
            let target = href.to_owned();
            let label = opencode_i18n::t!("common.open");
            return el().key(format!("clip-{at}")).role("button").focusable(true)
                .label(format!("{label}: {name}")).child(text(label).font(crate::style::UI).color(p.muted))
                .on_press_tx(move |s: &mut S, tx| {
                    // Retain the failed player with the fallback so a redraw doesn't retry it.
                    if clip.face() == Face::Failed { s.open_link(&target, tx); }
                });
        }
        transport::stage::<S>(p, clip, name).key(format!("clip-{at}"))
    }
}

pub(crate) fn reply<S: TimelineHost>(s: &S, p: &Pal, row: &str, src: &str, streaming: bool, render: impl Fn(Range<usize>) -> El) -> Option<El> {
    // During streaming the final paragraph may still acquire more prose.
    if streaming || !(src.contains('[') || src.contains("://")) || !opencode_kit::path::mentions_video(src) { return None; }
    let parser = Parser::new_ext(src, Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS);
    let references = parser.reference_definitions().iter().next().is_some();
    let mut depth = 0;
    let mut paragraph = false;
    let mut clips = Vec::new();
    for (event, range) in parser.into_offset_iter() {
        // Only a complete link on a line of top-level prose. Fences, tables, quotes,
        // inline code, image syntax and link text inside sentences keep their Markdown.
        let candidate = if paragraph && depth == 1 {
            match &event {
                Event::Start(Tag::Link { dest_url, .. }) => Some(dest_url.as_ref()),
                Event::Text(t) if (t.starts_with("https://") || t.starts_with("http://")) && !t.trim().contains(char::is_whitespace) => Some(t.trim()),
                _ => None,
            }
        } else { None };
        if let Some(href) = candidate {
            let before = src[..range.start].rsplit('\n').next().unwrap_or("");
            let after = src[range.end..].split('\n').next().unwrap_or("");
            if before.trim().is_empty() && after.trim().is_empty() && !src[range.clone()].contains('\n')
                && let Some(source) = s.reply_video_source(href)
            {
                let raw = &src[range.clone()];
                let label = raw.strip_prefix('[').and_then(|s| s.split_once(']').map(|(label, _)| label)).unwrap_or(raw);
                clips.push(Embed { end: range.end, source, href: href.to_owned(), name: label.to_owned() });
            }
        }
        match event {
            Event::Start(tag) => {
                if depth == 0 { paragraph = tag == Tag::Paragraph; }
                depth += 1;
            }
            Event::End(_) => { depth -= 1; if depth == 0 { paragraph = false; } }
            _ => {}
        }
    }
    if clips.is_empty() { return None; }
    // Keep document-wide reference definitions in scope when present.
    let mut out = if references { render(0..src.len()) } else { col().w_full().gap(8.0) };
    let mut from = 0;
    for embed in clips {
        let end = embed.end;
        if !references { out.push_child(render(from..end).key(format!("md-{from}"))); }
        out.push_child(s.media_store().videos.view(s, p, row, &embed));
        from = end;
    }
    if !references && !src[from..].trim().is_empty() { out.push_child(render(from..src.len()).key(format!("md-{from}"))); }
    Some(out)
}
