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
use crate::{Entry, Grants, Kind, States, WidgetTemplate, WidgetsHost, input, retry};

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
        let label = call.input["title"].as_str().filter(|t| !t.is_empty()).or_else(|| call.input["widgetTemplate"].as_str()).unwrap_or("widget");
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
        return Some(session_view::widget::frame(s, p, session, &key, label, face, &Local::default(), None, Place::Inline(Buttons::default()), &[]));
    };
    if s.widgets().screen.is(&key) {
        return Some(session_view::widget::frame(s, p, session, &key, &e.label, e.machine.face(), &e.local, None, Place::Elsewhere, &[]));
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
    // A program of another protocol can't run: nothing to try again or restart.
    let refused = matches!(s.widgets().widget_templates.get(&e.widget_template), Some(WidgetTemplate::Ready(c)) if c.refused());
    let buttons = Buttons { retry: (!refused).then_some(retry), expand, failure, corner: s.widget_corner(session, &key) };
    let frame = session_view::widget::frame(s, p, session, &key, &e.label, e.machine.face(), &e.local, Some(&hooks::<H>(&key)), Place::Inline(buttons), &chips(s, e));
    // Its size, for a window of its own to open at.
    let id = key.clone();
    let frame = frame.on_layout(move |s: &mut H, [_, _, w, h]| {
        if s.widgets().laid(&id) != Some([w, h]) {
            s.widgets_mut().laid.insert(id.clone(), [w, h]);
        }
    });
    Some(draggable(s, p, session, &key, frame))
}

/// Widget `key` as the host lets the user drag it out of the window (onto the desktop): its
/// frame under a grab strip, shown on hover, that the drag starts from. The strip is outside the
/// frame, so a press there is the wrapper's (the frame takes its own presses, for keyboard focus)
/// and the drag shows the whole widget.
fn draggable<H: WidgetsHost + 'static>(s: &H, p: &Pal, session: &str, key: &str, frame: El) -> El {
    if !s.widget_drags_out() {
        return frame;
    }
    let (session, id) = (session.to_owned(), key.to_owned());
    let words = opencode_i18n::t!("rune.widget.dragOut").to_owned();
    let grip = rune::row()
        .key("grip")
        .w_full()
        .h(12.0)
        .center()
        .label(words.clone())
        .help(words)
        .child(rune::el().w(36.0).h(4.0).radius(2.0).fill(p.faint).group_opacity(0.0, 1.0));
    rune::col()
        .key(format!("widget-drag-{key}"))
        .w_full()
        .hover_group()
        .drag_out(move |s: &H| s.widget_drag_out(&session, &id))
        .child(grip)
        .child(frame)
}

/// The widget instance key `key` (`sessionID:messageID:callID`, [`crate::instances`]) names
/// now, `width` points wide, live and interactive, wherever the host shows it (a dashboard tile,
/// its own window). `None` while the server's widgets aren't read or it isn't among them; it
/// runs once [`crate::instances::open`] opened it.
pub fn instance_view<H: WidgetsHost + TimelineHost>(s: &H, p: &Pal, key: &str, width: f32) -> Option<El> {
    let i = s.widgets().instances.resolve(key)?;
    let st = &i.state;
    Some(tile(s, p, &st.session, &st.key(), st.label(), Some(&st.status), width))
}

/// The widget instance key `key` (`sessionID:messageID:callID`) names now, filling a window of
/// its own `size` points big (a standalone widget): live and interactive as in the timeline, its
/// events going to its own session, following updates as a dashboard tile does
/// ([`crate::instances`]). A placeholder while it isn't listed or open yet.
pub fn window_view<H: WidgetsHost + TimelineHost>(s: &H, p: &Pal, key: &str, size: [f32; 2]) -> El {
    let place = Place::Window { size };
    let Some(i) = s.widgets().instances.resolve(key) else {
        return session_view::widget::frame(s, p, "", key, "", Face::Loading, &Local::default(), None, place, &[]);
    };
    let (session, id) = (i.state.session.clone(), i.state.key());
    match s.widgets().entries.get(&id) {
        Some(e) => session_view::widget::frame(s, p, &session, &id, &e.label, e.machine.face(), &e.local, Some(&hooks::<H>(&id)), place, &chips(s, e)),
        None => session_view::widget::frame(s, p, &session, &id, i.state.label(), Face::Loading, &Local::default(), None, place, &[]),
    }
}

/// Widget `key` (`message/call`) of session `session` outside the timeline (a Mini apps tile),
/// `width` points wide: live and interactive as in the timeline, its events going to its own
/// session. `label` names it until it opens; `closed` is how it ended, from its state, until then.
pub fn tile<H: WidgetsHost + TimelineHost>(s: &H, p: &Pal, session: &str, key: &str, label: &str, closed_as: Option<&WidgetStatus>, width: f32) -> El {
    let Some(e) = s.widgets().entries.get(key) else {
        let how = closed_as.and_then(closed);
        let face = match &how {
            Some(how) => Face::Closed { view: None, how },
            None => Face::Loading,
        };
        return session_view::widget::frame(s, p, session, key, label, face, &Local::default(), None, Place::Tile { buttons: Buttons::default(), width }, &[]);
    };
    if s.widgets().screen.is(key) {
        return session_view::widget::frame(s, p, session, key, &e.label, e.machine.face(), &e.local, None, Place::Elsewhere, &[]);
    }
    let again = key.to_owned();
    let retry: opencode_widget_view::Action<H> = Rc::new(move |s: &mut H, _tx: &mut Tx| retry(s, &again));
    let expand = offers(e).then(|| {
        let id = key.to_owned();
        Rc::new(move |s: &mut H, tx: &mut Tx| {
            let now = tx.clock();
            crate::fullscreen::send(s, ScreenEvent::Enter(id.clone()), now, tx);
        }) as opencode_widget_view::Action<H>
    });
    let refused = matches!(s.widgets().widget_templates.get(&e.widget_template), Some(WidgetTemplate::Ready(c)) if c.refused());
    let buttons = Buttons { retry: (!refused).then_some(retry), expand, failure: recorded::<H>(s, e, session, key), corner: Vec::new() };
    session_view::widget::frame(s, p, session, key, &e.label, e.machine.face(), &e.local, Some(&hooks::<H>(key)), Place::Tile { buttons, width }, &chips(s, e))
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
/// `fullscreen`, else it is live or draws a canvas), and it shows a view.
fn offers(e: &Entry) -> bool {
    if !e.machine.open_here() {
        return false;
    }
    let live = e.machine.running();
    e.machine.view().is_some_and(|v| v.offers_fullscreen(live))
}

/// What its widget template is granted: the hosts it may reach, and its storage.
fn chips<H: WidgetsHost>(s: &H, e: &Entry) -> Vec<String> {
    let Some(WidgetTemplate::Ready(c)) = s.widgets().widget_templates.get(&e.widget_template) else { return Vec::new() };
    let Kind::Live { grants: Grants { https, storage }, .. } = &c.kind else { return Vec::new() };
    https.iter().map(|hosts| session_view::widget::network_chip(hosts)).chain(storage.then(session_view::widget::storage_chip)).collect()
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
    let [_, _, w, h] = s.widgets().screen_rect;
    let place = Place::Screen { size: [w, h], close };
    let screen = session_view::widget::frame(s, p, &e.session, id, &e.label, e.machine.face(), &e.local, Some(&hooks::<H>(id)), place, &chips(s, e));
    // Its size follows the window's: a resize lays the widget out anew, its canvases with it.
    vec![screen.on_layout(|s: &mut H, rect| crate::fullscreen::laid(s, rect))]
}

/// Esc and Alt+← (back) leave full screen.
pub fn screen_key<H: WidgetsHost>(s: &mut H, ev: &KeyEvent, tx: &mut Tx) -> bool {
    crate::fullscreen::back_key(ev) && crate::back(s, tx)
}
