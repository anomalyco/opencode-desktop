//! Standalone movie links in assistant Markdown. The original link remains the caption and
//! fallback. No HTML or provider iframe is needed; the host resolves a source, Rune plays it.
mod transport;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Range;
use std::rc::{Rc, Weak};

use pulldown_cmark::{Event, Options, Parser, Tag};
use rune::{Color, El, Player, PlayerState, SurfaceFit, Tx, col, el, text};

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

struct Clip {
    player: Player,
    face: Cell<Face>,
    transport: transport::Transport,
}

impl Clip {
    fn face(&self) -> Face {
        if matches!(self.player.state(), PlayerState::Failed(_)) { Face::Failed }
        else if self.player.is_playing() { Face::Playing }
        else { Face::Paused }
    }
}

impl Clips {
    pub(crate) fn version(&self) -> u64 { self.version.get() }

    pub(crate) fn poll(&self, active: Option<&str>, tx: &mut Tx) -> bool {
        let mut changed = false;
        self.players.borrow_mut().retain(|(sid, _, _), weak| {
            let Some(c) = weak.upgrade() else { return false };
            if active != Some(sid) && c.player.is_playing() { c.player.pause(); }
            if active == Some(sid) {
                c.transport.sync(&c.player);
                if c.player.is_playing() { tx.wake_at(tx.clock() + 1.0 / 60.0); }
            }
            let face = c.face();
            changed |= c.face.replace(face) != face;
            true
        });
        if changed { self.version.set(self.version.get().wrapping_add(1)); }
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
            let c = Rc::new(Clip { player, face: Cell::new(Face::Paused), transport: Default::default() });
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
        col().key(format!("clip-{at}")).w_full().fill(p.layer1).clip_rounded(6.0)
            .child(el().key("picture").w_full().aspect_ratio(16.0/9.0).max_h(360.0)
                .fill(Color::BLACK).video(&clip.player, SurfaceFit::Fit))
            .child(transport::controls::<S>(p, clip, name))
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
