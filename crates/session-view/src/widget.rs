//! Agent widgets in the timeline (`specs/v2/widgets.md`): each is a `widget` tool call, on a row
//! of its own where the agent called it. The host runs the widget (its template program, its
//! lifecycle machine, its view-local state) and draws it in the call's place through
//! [`TimelineHost::widget`], with [`frame`] here doing the drawing: `opencode-widget-view` on
//! this timeline's Kit, its Markdown and its pictures. A host that doesn't run widgets shows the
//! call as any other.

use std::rc::Rc;

use opencode_widget::Face;
use opencode_widget_view::{Action, Buttons, Hooks, Local, Look, Words};
use rune::El;

use crate::TimelineHost;
use crate::palette::Pal;

pub use opencode_widget as runtime;
pub use opencode_widget_view as view;

/// Padding inside a widget's frame, each side (`opencode-widget-view`'s frame).
const FRAME_PAD: f32 = 16.0;

/// The frame's words in the interface's language, made once per language.
fn words() -> Rc<Words> {
    thread_local! {
        static WORDS: std::cell::RefCell<Option<(u64, Rc<Words>)>> = const { std::cell::RefCell::new(None) };
    }
    let lang = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        opencode_i18n::locale().id().hash(&mut h);
        h.finish()
    };
    WORDS.with_borrow_mut(|w| {
        if let Some((_, words)) = w.as_ref().filter(|(at, _)| *at == lang) {
            return words.clone();
        }
        let words = Rc::new(Words {
            loading: opencode_i18n::t!("rune.widget.loading").to_owned(),
            failed: opencode_i18n::t!("rune.widget.failed").to_owned(),
            retry: opencode_i18n::t!("rune.widget.retry").to_owned(),
            restart: opencode_i18n::t!("rune.widget.restart").to_owned(),
            fullscreen: opencode_i18n::t!("rune.widget.fullscreen").to_owned(),
            exit_fullscreen: opencode_i18n::t!("rune.widget.exitFullscreen").to_owned(),
            elsewhere: opencode_i18n::t!("rune.widget.elsewhere").to_owned(),
            release: opencode_i18n::t!("rune.widget.release").to_owned(),
            superseded: opencode_i18n::t!("rune.widget.superseded").to_owned(),
            ask: opencode_i18n::t!("rune.widget.ask").to_owned(),
            asking: opencode_i18n::t!("rune.widget.asking").to_owned(),
            told: opencode_i18n::t!("rune.widget.told").to_owned(),
            sending: opencode_i18n::t!("rune.widget.sending").to_owned(),
            submitted: opencode_i18n::t!("rune.widget.submitted").to_owned(),
            dismissed: opencode_i18n::t!("rune.widget.dismissed").to_owned(),
            previous: opencode_i18n::t!("rune.widget.previous").to_owned(),
            next: opencode_i18n::t!("rune.widget.next").to_owned(),
        });
        *w = Some((lang, words.clone()));
        words
    })
}

/// Where a widget draws.
pub enum Place<S> {
    /// In the timeline, with its frame's buttons.
    Inline(Buttons<S>),
    /// In the timeline while it is full screen: a placeholder.
    Elsewhere,
    /// Full screen, filling a window `size` points big; `close` leaves it.
    Screen { size: [f32; 2], close: Action<S> },
}

/// Widget `id` (its key) of session `session`, called `label`, as `face` shows it, where
/// `place` says; `chip` names the capabilities granted to it.
#[allow(clippy::too_many_arguments)]
pub fn frame<S: TimelineHost>(s: &S, p: &Pal, session: &str, id: &str, label: &str, face: Face<'_>, local: &Local, hooks: Option<&Hooks<S>>, place: Place<S>, chip: Option<&str>) -> El {
    let fullscreen = matches!(place, Place::Screen { .. });
    let width = match &place {
        Place::Screen { size, .. } => (size[0] - 48.0).max(200.0),
        _ => crate::style::content_w(),
    };
    let store = s.media_store().markdown.clone();
    let directory = s.session_directory(session);
    // Pictures come through the timeline's own store: web images and the session's files fetched
    // once on its worker, `data:` URIs decoded once, every one by Rune's image cache.
    let image = |src: &str| if src.starts_with("data:") { store.data_image(src) } else { store.image(directory, src) };
    let pad = if fullscreen { 0.0 } else { 2.0 * FRAME_PAD };
    let markdown = |src: &str| crate::style::markdown_tinted::<S>(p, src, 1.0, false, p.text, width - pad);
    let words = words();
    let look = Look { kit: crate::palette::kit(p), markdown: &markdown, image: &image, words: &words, width, scale: crate::scroller::device_scale(), dark: p.dark, scope: id, chip, fullscreen };
    match place {
        Place::Inline(buttons) => opencode_widget_view::frame(&look, local, hooks, face, label, buttons).key(format!("widget-{id}")),
        Place::Elsewhere => opencode_widget_view::placeholder(&look, label).key(format!("widget-{id}")),
        Place::Screen { size, close } => opencode_widget_view::screen(&look, local, hooks, face, label, size[1], close).key(format!("widget-screen-{id}")),
    }
}

/// A widget a newer one replaced: one quiet line, "Updated below", where it was.
pub fn superseded<S: TimelineHost>(p: &Pal, id: &str) -> El {
    let words = opencode_i18n::t!("rune.widget.superseded").to_owned();
    rune::row()
        .key(format!("widget-{id}"))
        .gap(8.0)
        .min_h(20.0)
        .center_y()
        .role("status")
        .label(words.clone())
        .child(rune::text("◈").font(crate::style::UI).color(p.faint).a11y_hidden())
        .child(rune::text(words).font(crate::style::UI).color(p.faint))
}

/// The chip naming a widget's granted network hosts.
pub fn network_chip(hosts: &[String]) -> String {
    opencode_i18n::t!("rune.widget.network", hosts = hosts.join(", ")).to_string()
}
