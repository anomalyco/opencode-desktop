//! Standalone movie links in assistant Markdown. The original link remains the caption and
//! fallback. No video in a reply loads without a user click: local video files (inside the
//! workspace, or outside on local sessions) render a play control disclosing the real path and
//! verify + load only on click; `http(s)` video links remain ordinary links that open in the
//! embedded browser pane.
mod seek;
mod transport;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Range;
use std::rc::{Rc, Weak};

use opencode_ui::video_overlay::{self, Effect, Event as OverlayEvent, Overlay};
use pulldown_cmark::{Event, Options, Parser, Tag};
use rune::{Cursor, Direction, El, Player, PlayerState, Tx, col, el, icon, row, text};

use crate::{TimelineHost, palette::Pal};

type ClipKey = (String, String, usize);

/// Explicit slot lifecycle for an inline reply clip.
enum Slot {
    /// User clicked Play and host verified `source`; next `view()` builds and starts `Player`.
    Starting { href: String, source: String },
    /// `view()` built `Player`; only the row element owns the strong `Rc<Clip>` so scrolling out
    /// of the memo cache releases decoders and drops the slot in `poll()`.
    Active { href: String, clip: Weak<Clip> },
}

#[derive(Default)]
pub(crate) struct Clips {
    slots: RefCell<HashMap<ClipKey, Slot>>,
    version: Cell<u64>,
}

#[derive(Clone, Copy, PartialEq)]
enum Face { Paused, Playing, Failed }

struct Embed {
    end: usize,
    display_path: String,
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

    fn activate(&self, key: ClipKey, href: String, source: String) {
        self.slots.borrow_mut().insert(key, Slot::Starting { href, source });
        self.invalidate();
    }

    pub(crate) fn poll(&self, active: Option<&str>, tx: &mut Tx) -> bool {
        let mut changed = false;
        self.slots.borrow_mut().retain(|(sid, _, _), slot| match slot {
            Slot::Starting { .. } => active == Some(sid),
            Slot::Active { clip, .. } => {
                let Some(c) = clip.upgrade() else { return false };
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
            }
        });
        if changed { self.invalidate(); }
        changed
    }

    fn build_clip<S: TimelineHost>(&self, s: &S, source: &str) -> Rc<Clip> {
        let player = Player::new(rune::MediaSource::from(source));
        player.set_muted(false);
        if let Some(wake) = &s.media_store().waker { player.set_waker(wake.clone()); }
        player.play();
        Rc::new(Clip { player, overlay: Cell::default(), reported: Cell::default(), track: Default::default() })
    }

    fn render_clip<S: TimelineHost>(&self, p: &Pal, at: usize, href: &str, name: &str, clip: Rc<Clip>) -> El {
        if clip.face() == Face::Failed {
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

    fn view<S: TimelineHost>(&self, s: &S, p: &Pal, row_key: &str, embed: &Embed) -> El {
        let Embed { end: at, display_path, href, name } = embed;
        let key = (s.active_session_id().unwrap_or("").to_owned(), row_key.to_owned(), *at);
        let mut slots = self.slots.borrow_mut();
        let resolved = match slots.get(&key) {
            Some(Slot::Starting { href: h, source }) if h == href => {
                let c = self.build_clip(s, source);
                slots.insert(key.clone(), Slot::Active { href: href.clone(), clip: Rc::downgrade(&c) });
                Some(c)
            }
            Some(Slot::Starting { .. }) => {
                slots.remove(&key);
                None
            }
            Some(Slot::Active { href: h, clip }) if h == href => clip.upgrade(),
            _ => None,
        };
        drop(slots);
        if let Some(clip) = resolved {
            return self.render_clip::<S>(p, *at, href, name, clip);
        }
        let play = opencode_i18n::t!("rune.video.playInline");
        let shown_path = middle_truncate(display_path, 56);
        let click_href = href.clone();
        row()
            .key(format!("clip-play-{at}"))
            .self_start()
            .shrink()
            .min_w(0.0)
            .h(32.0)
            .px(10.0)
            .gap(6.0)
            .radius(8.0)
            .fill(p.layer2)
            .hover(p.hover)
            .press(p.pressed)
            .hover_motion(crate::style::INSTANT, crate::style::INSTANT)
            .border(1.0, p.border_muted)
            .center_y()
            .cursor(Cursor::Pointer)
            .role("button")
            .focusable(true)
            .label(format!("{play}: {display_path}"))
            .help(display_path.clone())
            .child(icon(&transport::PLAY, 14.0).color(p.text))
            .child(text(play).font(crate::style::UI).color(p.text))
            .child(
                text(format!("\u{b7} {shown_path}"))
                    .font(crate::style::UI_SMALL)
                    .color(p.muted)
                    .dir(Direction::Ltr)
                    .shrink()
                    .min_w(0.0)
                    .truncate_fade(),
            )
            .on_press_tx(move |s: &mut S, tx: &mut Tx| {
                if let Some(verified) = s.load_reply_video(&click_href, tx) {
                    s.media_store_mut().videos.activate(key.clone(), click_href.clone(), verified);
                    tx.instant();
                }
            })
    }
}

fn middle_truncate(path: &str, max_chars: usize) -> String {
    let chars: Vec<char> = path.chars().collect();
    if chars.len() <= max_chars || max_chars < 8 {
        return path.to_owned();
    }
    let budget = max_chars - 1;
    let file_tail = path.rfind(['/', '\\']).map(|i| path[i..].chars().count()).unwrap_or(0);
    let tail_len = if file_tail > 0 && file_tail + 12 <= budget {
        file_tail.max((budget * 3) / 5)
    } else {
        (budget * 3) / 5
    };
    let head_len = budget.saturating_sub(tail_len).max(6);
    let tail_len = budget.saturating_sub(head_len);
    let head: String = chars[..head_len].iter().collect();
    let tail: String = chars[chars.len() - tail_len..].iter().collect();
    format!("{head}\u{2026}{tail}")
}

fn format_reference_defs(defs: &pulldown_cmark::RefDefs) -> String {
    let mut out = String::new();
    for (label, def) in defs.iter() {
        let dest = def.dest.replace(['\r', '\n'], "");
        if dest.contains(['<', '>']) {
            continue;
        }
        out.push_str("\n\n[");
        out.push_str(label.as_ref());
        out.push_str("]: <");
        out.push_str(&dest);
        out.push('>');
        if let Some(title) = &def.title {
            let clean_title = title.replace(['\r', '\n'], " ").replace('"', "\\\"");
            out.push_str(" \"");
            out.push_str(&clean_title);
            out.push('"');
        }
    }
    out
}

fn has_visible_markdown(slice: &str, ref_defs: &str) -> bool {
    if slice.trim().is_empty() {
        return false;
    }
    if ref_defs.is_empty() {
        return true;
    }
    let full = format!("{slice}{ref_defs}");
    Parser::new_ext(&full, Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS)
        .next()
        .is_some()
}

pub(crate) fn reply<S: TimelineHost>(s: &S, p: &Pal, row_key: &str, src: &str, streaming: bool, render: impl Fn(Range<usize>, &str) -> El) -> Option<El> {
    // During streaming the final paragraph may still acquire more prose.
    if streaming || !src.contains('[') || !opencode_kit::path::mentions_video(src) { return None; }
    let parser = Parser::new_ext(src, Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS);
    let ref_defs = format_reference_defs(parser.reference_definitions());
    let mut depth = 0;
    let mut paragraph = false;
    let mut clips = Vec::new();
    for (event, range) in parser.into_offset_iter() {
        // Only a complete link on a line of top-level prose. Fences, tables, quotes,
        // inline code, image syntax and link text inside sentences keep their Markdown.
        let candidate = if paragraph && depth == 1 {
            match &event {
                Event::Start(Tag::Link { dest_url, .. }) => Some(dest_url.as_ref()),
                _ => None,
            }
        } else { None };
        if let Some(href) = candidate {
            let before = src[..range.start].rsplit('\n').next().unwrap_or("");
            let after = src[range.end..].split('\n').next().unwrap_or("");
            if before.trim().is_empty() && after.trim().is_empty() && !src[range.clone()].contains('\n')
                && let Some(display_path) = s.reply_video_source(href)
            {
                let raw = &src[range.clone()];
                let label = raw.strip_prefix('[').and_then(|s| s.split_once(']').map(|(label, _)| label)).unwrap_or(raw);
                clips.push(Embed { end: range.end, display_path, href: href.to_owned(), name: label.to_owned() });
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
    // Keep every clip control inline beside its link even when document-wide reference definitions
    // are present, appending reference definitions invisibly to each Markdown slice.
    let mut out = col().w_full().gap(8.0);
    let mut from = 0;
    for embed in clips {
        let end = embed.end;
        out.push_child(render(from..end, &ref_defs).key(format!("md-{from}")));
        out.push_child(s.media_store().videos.view(s, p, row_key, &embed));
        from = end;
    }
    if has_visible_markdown(&src[from..], &ref_defs) {
        out.push_child(render(from..src.len(), &ref_defs).key(format!("md-{from}")));
    }
    Some(out)
}
