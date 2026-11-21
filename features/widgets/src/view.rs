//! How the timeline and the full-screen layer draw widgets: in their call's place with their
//! frame's buttons, a placeholder there while full screen, or filling the window.

use std::rc::Rc;

use opencode_kit::model::{ToolStatus, WidgetStatus, widget_key};
use opencode_widget::{Closed, Face, WidgetEvent};
use opencode_widget_view::{Buttons, Hooks, Local};
use rune::{El, KeyEvent, Tx};
use session_view::TimelineHost;
use session_view::palette::Pal;
use session_view::widget::Place;

use crate::fullscreen::{Screen, ScreenEvent};
use crate::{Entry, Grants, Kind, States, Template, WidgetsHost, input, retry};

/// Widget call `call` of assistant message `message` in session `session` as the timeline draws
/// it: the widget, a placeholder while its state or program is on the way (or while it is full
/// screen), or `None` to show the call as any other (the plugin isn't there, or the call failed
/// before it posted a widget).
pub fn view<H: WidgetsHost + TimelineHost>(s: &H, p: &Pal, session: &str, message: &str, call: &session_view::tools::Call) -> Option<El> {
    let key = widget_key(message, call.id);
    // Replaced by a newer widget (which follows in the timeline): one quiet line, not interactive.
    let superseded = match s.widgets().states(session) {
        Some(States::Ready(map)) => map.get(&key).is_some_and(|w| w.status == WidgetStatus::Superseded),
        _ => false,
    };
    if superseded {
        return Some(session_view::widget::superseded::<H>(p, &key));
    }
    let Some(e) = s.widgets().entries.get(&key) else {
        // Not opened yet (the next frame's sync opens it): a closed one already says how it
        // ended; an open one, or one whose state is on the way, shows its placeholder.
        let label = call.input["title"].as_str().filter(|t| !t.is_empty()).or_else(|| call.input["template"].as_str()).unwrap_or("widget");
        let how = match s.widgets().states(session) {
            Some(States::Ready(map)) => match map.get(&key) {
                Some(w) => closed(&w.status),
                None if *call.status == ToolStatus::Running => None,
                None => return None,
            },
            Some(States::Fetching) | None if *call.status != ToolStatus::Error => None,
            _ => return None,
        };
        let face = match &how {
            Some(how) => Face::Closed { view: None, how },
            None => Face::Loading,
        };
        return Some(session_view::widget::frame(s, p, session, &key, label, face, &Local::default(), None, Place::Inline(Buttons::default()), None));
    };
    if s.widgets().screen.is(&key) {
        return Some(session_view::widget::frame(s, p, session, &key, &e.label, e.machine.face(), &e.local, None, Place::Elsewhere, None));
    }
    let again = key.clone();
    let retry: opencode_widget_view::Action<H> = Rc::new(move |s: &mut H, _tx: &mut Tx| retry(s, &again));
    let expand = offers(e).then(|| {
        let id = key.clone();
        Rc::new(move |s: &mut H, tx: &mut Tx| {
            let now = tx.clock();
            crate::fullscreen::send(s, ScreenEvent::Enter(id.clone()), now, tx);
        }) as opencode_widget_view::Action<H>
    });
    let failure = recorded::<H>(s, e, session, &key);
    let buttons = Buttons { retry: Some(retry), expand, failure };
    Some(session_view::widget::frame(s, p, session, &key, &e.label, e.machine.face(), &e.local, Some(&hooks::<H>(&key)), Place::Inline(buttons), chip(s, e).as_deref()))
}

/// Widget `key`'s recorded program error (`State.failure`, any client's) while it's open: its
/// words, unless the widget shows its own error, then "The agent was told about this error." or
/// the "Ask agent to fix it" button.
fn recorded<H: WidgetsHost + 'static>(s: &H, e: &Entry, session: &str, key: &str) -> Option<opencode_widget_view::Recorded<H>> {
    let f = crate::failure_of(s.widgets(), session, key)?;
    let shows_own = matches!(e.machine.face(), opencode_widget::Face::Failed(_) | opencode_widget::Face::Live { error: Some(_), .. });
    let at = match (f.line, f.column) {
        (Some(l), Some(c)) => format!(" ({l}:{c})"),
        (Some(l), None) => format!(" (line {l})"),
        _ => String::new(),
    };
    let error = format!("{}: {}{at}", f.phase, f.message);
    let text = (!shows_own).then(|| opencode_i18n::t!("rune.widget.failedOn", client = f.client.clone(), error = error).to_string());
    let id = key.to_owned();
    let ask = (!e.asking()).then(|| Rc::new(move |s: &mut H, _tx: &mut Tx| crate::ask(s, &id)) as opencode_widget_view::Action<H>);
    Some(opencode_widget_view::Recorded { text, sent: f.sent, ask })
}

/// Where widget `key`'s view sends what the user does: its events, and its keys and pointer
/// for keyboard focus and capture.
fn hooks<H: WidgetsHost + 'static>(key: &str) -> Hooks<H> {
    let (lens, emit, take) = (key.to_owned(), key.to_owned(), key.to_owned());
    Hooks {
        local: Rc::new(move |s: &mut H| s.widgets_mut().local_mut(&lens)),
        emit: Rc::new(move |s: &mut H, ev: WidgetEvent, _tx: &mut Tx| input(s, &emit, ev)),
        input: Some(Rc::new(move |s: &mut H, i: opencode_widget_view::Input<'_>, tx: &mut Tx| crate::input::take(s, &take, i, tx))),
    }
}

/// Whether widget `e` offers full screen now: it's open here, its view says so (its root's
/// `fullscreen`, else it is live or draws a canvas or shader), and it shows a view.
fn offers(e: &Entry) -> bool {
    if !e.machine.open_here() {
        return false;
    }
    let live = e.machine.running();
    e.machine.view().is_some_and(|v| v.offers_fullscreen(live))
}

/// The hosts it may reach, when its template is granted the network.
fn chip<H: WidgetsHost>(s: &H, e: &Entry) -> Option<String> {
    match s.widgets().templates.get(&e.template) {
        Some(Template::Ready(c)) => match &c.kind {
            Kind::Live { grants: Grants { https: Some(hosts), .. }, .. } => Some(session_view::widget::network_chip(hosts)),
            _ => None,
        },
        _ => None,
    }
}

/// How a widget ended, from its state alone.
pub(crate) fn closed(status: &WidgetStatus) -> Option<Closed> {
    match status {
        WidgetStatus::Open => None,
        WidgetStatus::Submitted(v) => Some(Closed::Submitted(v.clone())),
        WidgetStatus::Dismissed => Some(Closed::Dismissed),
        WidgetStatus::Superseded => Some(Closed::Superseded),
    }
}

/// The widget full screen, filling the window, for the host's overlay layer.
pub fn screen_view<H: WidgetsHost + TimelineHost>(s: &H, p: &Pal) -> Vec<El> {
    let Screen::Full { id, .. } = &s.widgets().screen else { return Vec::new() };
    let Some(e) = s.widgets().entries.get(id) else { return Vec::new() };
    let close: opencode_widget_view::Action<H> = Rc::new(|s: &mut H, tx: &mut Tx| {
        let now = tx.clock();
        crate::fullscreen::send(s, ScreenEvent::Exit, now, tx);
    });
    let place = Place::Screen { size: s.widgets().screen_size, close };
    let screen = session_view::widget::frame(s, p, &e.session, id, &e.label, e.machine.face(), &e.local, Some(&hooks::<H>(id)), place, chip(s, e).as_deref());
    // Its size follows the window's: a resize lays the widget out anew, its canvases with it.
    vec![screen.on_layout(|s: &mut H, [_, _, w, h]| {
        let size = &mut s.widgets_mut().screen_size;
        if *size != [w, h] {
            *size = [w, h];
        }
    })]
}

/// Esc leaves full screen.
pub fn screen_key<H: WidgetsHost>(s: &mut H, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.key != rune::Key::Escape || matches!(s.widgets().screen, Screen::Inline) {
        return false;
    }
    let now = tx.clock();
    crate::fullscreen::send(s, ScreenEvent::Exit, now, tx);
    true
}
