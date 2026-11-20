//! Agent widgets in the timeline (`Message::Widget`, `specs/v2/widgets.md`): each sits on a
//! row of its own where the agent posted it. The host runs the widget (its template program,
//! its lifecycle machine, its view-local state) and draws it through [`TimelineHost::widget`],
//! with [`frame`] here doing the drawing: `opencode-widget-view` on this timeline's Kit, its
//! Markdown and its pictures. A host that doesn't run widgets gets [`fallback`].

use std::rc::Rc;

use opencode_kit::model::{Widget, WidgetStatus};
use opencode_widget::{Closed, Face};
use opencode_widget_view::{Action, Hooks, Local, Look, Words};
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

/// Widget `w` of session `session` in its frame, as `face` shows it. `retry` is what Try again
/// does on a failed one.
#[allow(clippy::too_many_arguments)]
pub fn frame<S: TimelineHost>(s: &S, p: &Pal, session: &str, w: &Widget, face: Face<'_>, local: &Local, hooks: Option<&Hooks<S>>, retry: Option<Action<S>>) -> El {
    let width = crate::style::content_w();
    let store = s.media_store().markdown.clone();
    let directory = s.session_directory(session);
    // Pictures come through the timeline's own store: web images and the session's files fetched
    // once on its worker, `data:` URIs decoded once, every one by Rune's image cache.
    let image = |src: &str| if src.starts_with("data:") { store.data_image(src) } else { store.image(directory, src) };
    let markdown = |src: &str| crate::style::markdown_tinted::<S>(p, src, 1.0, false, p.text, width - 2.0 * FRAME_PAD);
    let words = words();
    let look = Look { kit: crate::palette::kit(p), markdown: &markdown, image: &image, words: &words, width };
    opencode_widget_view::frame(&look, local, hooks, face, w.label(), retry).key(format!("widget-{}", w.id))
}

/// How the widget ended, from its message alone.
pub fn closed(w: &Widget) -> Option<Closed> {
    match &w.status {
        WidgetStatus::Open => None,
        WidgetStatus::Submitted(v) => Some(Closed::Submitted(v.clone())),
        WidgetStatus::Dismissed => Some(Closed::Dismissed),
    }
}

/// A widget on a host that doesn't run widgets: its name and how it ended, or that it can't be
/// shown here while it's open.
pub fn fallback<S: TimelineHost>(s: &S, p: &Pal, session: &str, w: &Widget) -> El {
    let local = Local::default();
    let unsupported = opencode_i18n::t!("rune.widget.unsupported").to_owned();
    let how = closed(w);
    let face = match &how {
        Some(how) => Face::Closed { view: None, how },
        None => Face::Failed(&unsupported),
    };
    frame(s, p, session, w, face, &local, None, None)
}
