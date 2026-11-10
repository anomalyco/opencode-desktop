//! The question dock: Desktop's `session-question-dock.tsx` and `question-keys.ts`. When the
//! open session waits on a question, the composer's surface morphs into it (`ComposerMorph`,
//! `spring-height.tsx`): one card whose height springs on Desktop's `docks.morph` (0.4 s, no
//! bounce) while the composer's face fades out and the question's fades up. The curves and
//! timings are knobs (⌘⇧K → Question dock, `dock::knobs`) whose defaults are Desktop's.
//! Answering or dismissing it, here or anywhere else, morphs it back with the draft as it was.
//!
//! Keys: ↑/↓ (Home/End) move the highlight, 1–9 pick, Space toggles, ↵ picks the highlighted
//! answer and moves on (submitting on the last question), ←/→ and ⌘[ change question, ⌘↵ goes
//! on or submits, Esc dismisses. A printable key on "Type your own answer" starts typing it.
//! In the field, keys type (digits too), Esc leaves it, and ↑ on its first line goes up to the
//! answer above. Submitting with a required question unanswered goes to that question.
//!
//! A long question keeps to part of the pane (p273): the dock is at most ⌘⇧K → Question dock →
//! Max height of the pane it sits in, so the timeline still shows above it.

use std::collections::{HashMap, HashSet};

use opencode_kit::model::{Field, FieldKind, Pending};
use rune::{Key, KeyEvent, TextEdit, Tx};
use serde_json::Value;

use super::{DockHost, knobs, morph};
use crate::keymap::Bind;

mod classic;
mod face;

/// Whether the dock is Desktop's default Classic one (Settings › New question UI off) rather
/// than the keyboard-first one this module's keys describe.
pub fn classic<S: DockHost>(s: &S) -> bool {
    !s.dock_new_question_ui()
}

/// The question's face of the composer surface, Classic or the new one.
pub fn face<S: DockHost>(s: &S, p: &crate::palette::Pal) -> Option<Vec<rune::El>> {
    if classic(s) { classic::face(s, p) } else { face::face(s, p) }
}

pub(super) const LINE: f32 = 20.0;
/// The footer's height: its padding and the 32 pt buttons.
const FOOTER_H: f32 = 47.0;
/// How far the answers fade under the header and footer, and how clear of them a revealed one stops.
pub const FADE: f32 = 16.0;
/// Never shorter than this while the pane has room (Desktop's floor).
const MIN_H: f32 = 240.0;
/// The share of the pane the timeline keeps above a long question, however short the pane.
const TIMELINE_KEEP: f32 = 0.2;
/// The session header's row over the timeline, with the timeline's own padding under it.
const HEADER_H: f32 = 48.0 + 16.0;

/// Where the dock is for the form it shows: the question, the highlighted answer, and the
/// picked and typed answers in progress by `form/key`.
#[derive(Default)]
pub struct Dock {
    form: String,
    pub tab: usize,
    pub focus: usize,
    /// -1 or 1: the way the last question change went, for the arrival slide.
    step: i32,
    pub picks: HashMap<String, Vec<String>>,
    pub typed: HashMap<String, TextEdit>,
    /// `form/key`s whose typed answer counts.
    custom: HashSet<String>,
    /// The typed answer's laid-out width, to tell which wrapped line its caret is on.
    input_w: f32,
    /// The Classic dock folded down to its question (the header's chevron).
    pub minimized: bool,
    /// The Classic dock's last input was a key, so the highlighted answer shows its focus ring.
    keyboard: bool,
    /// The Classic dock's answer under the pointer.
    hover: Option<usize>,
}

impl Dock {
    /// Whether the typed answer at `form/key` counts towards the reply.
    pub fn custom_on(&self, key: &str) -> bool {
        self.custom.contains(key)
    }

    pub fn forget(&mut self, form: &str) {
        let prefix = format!("{form}/");
        self.picks.retain(|k, _| !k.starts_with(&prefix));
        self.typed.retain(|k, _| !k.starts_with(&prefix));
        self.custom.retain(|k| !k.starts_with(&prefix));
    }

    pub fn typed_mut(&mut self, form: &str, key: &str) -> &mut TextEdit {
        self.typed.entry(format!("{form}/{key}")).or_insert_with(|| TextEdit::new(""))
    }

    /// Form `id`'s answer to send, or None while a required field is empty.
    pub fn answer(&self, id: &str, fields: &[Field]) -> Option<Value> {
        let mut answer = serde_json::Map::new();
        for f in fields {
            let k = format!("{id}/{}", f.key);
            let typed = self.typed.get(&k).filter(|_| self.custom_on(&k)).map(|t| t.text.trim().to_owned()).filter(|t| !t.is_empty());
            let picks = self.picks.get(&k).cloned().unwrap_or_default();
            let v = match f.kind {
                FieldKind::Multi => {
                    let mut all = picks;
                    all.extend(typed);
                    (!all.is_empty()).then(|| Value::from(all))
                }
                FieldKind::Boolean => picks.first().map(|v| Value::Bool(v == "true")),
                FieldKind::Number => typed.and_then(|t| t.parse::<f64>().ok()).map(Value::from),
                FieldKind::Text => typed.or_else(|| picks.first().cloned()).map(Value::from),
            };
            match v {
                Some(v) => {
                    answer.insert(f.key.clone(), v);
                }
                None if f.required || fields.len() == 1 => return None,
                None => {}
            }
        }
        Some(Value::Object(answer))
    }
}

/// What a key does in the dock (`QuestionKeyIntent`).
#[derive(Clone, Debug, PartialEq)]
pub enum Intent {
    Dismiss,
    Advance,
    Question(i32),
    Move(i32),
    First,
    Last,
    Number(usize),
    Activate,
    Commit,
    Type(String),
    None,
}

/// Where the keyboard is: on an answer, on "Type your own answer", or typing in it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Zone {
    Option,
    Custom,
    Input,
}

/// Desktop's `questionKey`: `count` answers including the typed one.
pub fn intent(ev: &KeyEvent, zone: Zone, count: usize) -> Intent {
    let m = ev.mods;
    match &ev.key {
        Key::Escape => return Intent::Dismiss,
        _ if Bind::QuestionBack.matches(ev) => return Intent::Question(-1),
        _ if Bind::QuestionAdvance.matches(ev) => return Intent::Advance,
        _ => {}
    }
    if zone == Zone::Input || m.cmd || m.ctrl || matches!(ev.key, Key::Command(_)) {
        return Intent::None;
    }
    let ch = match &ev.key {
        Key::Text(s) if s.chars().count() == 1 => s.chars().next(),
        _ => None,
    };
    let digit = ch.and_then(|c| c.to_digit(10)).filter(|d| *d >= 1);
    if let (Zone::Custom, None, Some(c)) = (zone, digit, ch)
        && c != ' '
    {
        return Intent::Type(c.to_string());
    }
    if m.alt {
        return Intent::None;
    }
    if let Some(d) = digit {
        let index = d as usize - 1;
        return if index < count { Intent::Number(index) } else { Intent::None };
    }
    let arrow = matches!(ev.key, Key::Left | Key::Right | Key::Up | Key::Down);
    if m.shift && arrow {
        return Intent::None;
    }
    match &ev.key {
        Key::Left => Intent::Question(-1),
        Key::Right => Intent::Question(1),
        Key::Down => Intent::Move(1),
        Key::Up => Intent::Move(-1),
        Key::Home => Intent::First,
        Key::End => Intent::Last,
        Key::Text(s) if s == " " => Intent::Activate,
        Key::Enter if !m.shift => Intent::Commit,
        _ => Intent::None,
    }
}

/// One answer's value, label and description. A yes/no question's answers are Yes and No.
pub fn rows(f: &Field) -> Vec<(String, String, String)> {
    if f.kind == FieldKind::Boolean {
        return vec![("true".into(), opencode_i18n::t!("provider.connect.form.yes").into(), String::new()), ("false".into(), opencode_i18n::t!("provider.connect.form.no").into(), String::new())];
    }
    f.options.iter().map(|o| (o.value.clone(), if o.label.is_empty() { o.value.clone() } else { o.label.clone() }, o.description.clone())).collect()
}

pub fn has_custom(f: &Field) -> bool {
    f.kind != FieldKind::Boolean && (f.custom || f.options.is_empty())
}

fn count(f: &Field) -> usize {
    rows(f).len() + has_custom(f) as usize
}

/// The question form the open session waits on.
pub struct Form {
    pub id: String,
    pub fields: Vec<Field>,
}

impl Form {
    fn key(&self, tab: usize) -> String {
        format!("{}/{}", self.id, self.fields[tab].key)
    }
    fn multi(&self, tab: usize) -> bool {
        self.fields[tab].kind == FieldKind::Multi
    }
    fn last(&self, tab: usize) -> bool {
        tab + 1 >= self.fields.len()
    }
}

/// The form the open page waits on. A subagent's page has no composer to stand in for.
pub fn asking<S: DockHost>(s: &S) -> Option<Form> {
    let id = s.active_session_id()?;
    if s.dock_parent(id).is_some() {
        return None;
    }
    s.dock_pending(id).into_iter().find_map(|p| match p {
        Pending::Form { id, fields, .. } if !fields.is_empty() => Some(Form { id, fields }),
        _ => None,
    })
}

/// The form the question dock shows.
pub fn id<S: DockHost>(s: &S) -> Option<String> {
    asking(s).map(|f| f.id)
}

/// The question and highlighted answer for `form`, as the dock left them or fresh.
fn place<S: DockHost>(s: &S, f: &Form) -> (usize, usize) {
    let d = s.dock();
    if d.form == f.id { (d.tab.min(f.fields.len() - 1), d.focus) } else { (0, pick_focus(s, f, 0)) }
}

fn dock<'a, S: DockHost>(s: &'a mut S, f: &Form) -> &'a mut Dock {
    if s.dock().form != f.id {
        let focus = pick_focus(s, f, 0);
        let d = s.dock_mut();
        (d.form, d.tab, d.focus, d.step) = (f.id.clone(), 0, focus, 0);
        (d.minimized, d.keyboard, d.hover) = (false, false, None);
    }
    s.dock_mut()
}

/// The answer a question opens on: its typed one when that's on, else its first pick.
fn pick_focus<S: DockHost>(s: &S, f: &Form, tab: usize) -> usize {
    let k = f.key(tab);
    let list = rows(&f.fields[tab]);
    if s.dock().custom_on(&k) && has_custom(&f.fields[tab]) {
        return list.len();
    }
    let picks = s.dock().picks.get(&k);
    list.iter().position(|(v, ..)| picks.is_some_and(|p| p.contains(v))).unwrap_or(0)
}

/// Whether the form's reply is on its way or taken (`isQuestionBusy`): the dock takes no answers.
fn busy<S: DockHost>(s: &S, f: &Form) -> bool {
    s.dock_busy(&f.id)
}

fn editing<S: DockHost>(s: &S, f: &Form, tab: usize) -> bool {
    s.dock().typed.get(&f.key(tab)).is_some_and(|t| t.focused)
}

/// Whether the dock takes letters now: its typed answer is open or highlighted.
pub fn takes_text<S: DockHost>(s: &S) -> bool {
    let Some(f) = asking(s) else { return false };
    let (tab, focus) = place(s, &f);
    // Classic's "Type your own answer" is a field only once it's open.
    editing(s, &f, tab) || (!classic(s) && focus >= rows(&f.fields[tab]).len() && has_custom(&f.fields[tab]))
}

/// Opens the current question's typed answer with `text` in it (the storybook's typing stories).
pub fn type_text<S: DockHost>(s: &mut S, text: &str) {
    let Some(f) = asking(s) else { return };
    let (tab, _) = place(s, &f);
    start_typing(s, &f, tab);
    let t = s.dock_mut().typed_mut(&f.id, &f.fields[tab].key);
    *t = TextEdit::new(text);
    t.focused = true;
}

/// Goes to the last question (the storybook's Custom answer story).
pub fn go_last<S: DockHost>(s: &mut S) {
    let Some(f) = asking(s) else { return };
    let last = f.fields.len() - 1;
    let focus = pick_focus(s, &f, last);
    let d = dock(s, &f);
    d.tab = last;
    d.focus = focus;
}

/// Leaves the typed answer's field as Esc does (the storybook's Custom answer story).
pub fn leave_field<S: DockHost>(s: &mut S) {
    let Some(f) = asking(s) else { return };
    let (tab, _) = place(s, &f);
    stop_typing(s, &f, tab);
    dock(s, &f).focus = rows(&f.fields[tab]).len();
}

fn answered<S: DockHost>(s: &S, f: &Form, tab: usize) -> bool {
    let k = f.key(tab);
    let d = s.dock();
    d.picks.get(&k).is_some_and(|p| !p.is_empty()) || (d.custom_on(&k) && d.typed.get(&k).is_some_and(|t| !t.text.trim().is_empty()))
}

fn stop_typing<S: DockHost>(s: &mut S, f: &Form, tab: usize) {
    if let Some(t) = s.dock_mut().typed.get_mut(&f.key(tab)) {
        t.focused = false;
    }
}

/// Picks answer `i`: the one answer of a single choice, or one more (or one fewer) of many.
fn select<S: DockHost>(s: &mut S, f: &Form, tab: usize, i: usize) {
    if busy(s, f) {
        return;
    }
    let Some((value, ..)) = rows(&f.fields[tab]).into_iter().nth(i) else { return };
    let k = f.key(tab);
    let d = s.dock_mut();
    let picks = d.picks.entry(k.clone()).or_default();
    if f.multi(tab) {
        match picks.iter().position(|v| *v == value) {
            Some(at) => {
                picks.remove(at);
            }
            None => picks.push(value),
        }
    } else {
        *picks = vec![value];
        d.custom.remove(&k);
    }
    stop_typing(s, f, tab);
}

fn start_typing<S: DockHost>(s: &mut S, f: &Form, tab: usize) {
    if busy(s, f) {
        return;
    }
    let k = f.key(tab);
    dock(s, f).focus = rows(&f.fields[tab]).len();
    let d = s.dock_mut();
    d.custom.insert(k.clone());
    if !f.multi(tab) {
        d.picks.remove(&k);
    }
    s.composer_edit_mut().focused = false;
    s.dock_mut().typed_mut(&f.id, &f.fields[tab].key).focused = true;
}

fn toggle_custom<S: DockHost>(s: &mut S, f: &Form, tab: usize) {
    if busy(s, f) {
        return;
    }
    let k = f.key(tab);
    if !f.multi(tab) || !s.dock().custom_on(&k) {
        return start_typing(s, f, tab);
    }
    s.dock_mut().custom.remove(&k);
    stop_typing(s, f, tab);
    dock(s, f).focus = rows(&f.fields[tab]).len();
}

fn jump<S: DockHost>(s: &mut S, f: &Form, tab: usize, tx: &mut Tx) {
    let (from, _) = place(s, f);
    if tab == from || tab >= f.fields.len() || busy(s, f) {
        return;
    }
    stop_typing(s, f, from);
    let focus = pick_focus(s, f, tab);
    let d = dock(s, f);
    d.step = if tab > from { 1 } else { -1 };
    d.tab = tab;
    d.focus = focus;
    tx.motion_curve(knobs::morph(s.knobs()));
}

/// Next question, or the reply on the last one.
fn next<S: DockHost>(s: &mut S, f: &Form, tx: &mut Tx) {
    let (tab, _) = place(s, f);
    stop_typing(s, f, tab);
    if !f.last(tab) {
        return jump(s, f, tab + 1, tx);
    }
    // Unanswered questions that may be skipped are left out, as Desktop does; a required one is
    // where the reply goes next instead.
    if s.dock().answer(&f.id, &f.fields).is_none() {
        let gap = (0..f.fields.len()).find(|&t| !answered(s, f, t) && (f.fields[t].required || f.fields.len() == 1));
        if let Some(t) = gap {
            jump(s, f, t, tx);
        }
        return;
    }
    s.dock_submit_form(&f.id);
    closed_if_gone(s, f, tx);
}

fn dismiss<S: DockHost>(s: &mut S, f: &Form, tx: &mut Tx) {
    s.dock_dismiss_form(&f.id);
    closed_if_gone(s, f, tx);
}

fn closed_if_gone<S: DockHost>(s: &mut S, f: &Form, tx: &mut Tx) {
    if !s.dock_waiting(&f.id) {
        morph(s, tx);
    }
}

/// Enter on a single choice picks the highlighted answer and moves on; an empty typed answer
/// opens its field instead.
fn commit<S: DockHost>(s: &mut S, f: &Form, tx: &mut Tx) {
    let (tab, focus) = place(s, f);
    if f.multi(tab) {
        return next(s, f, tx);
    }
    if focus < rows(&f.fields[tab]).len() {
        select(s, f, tab, focus);
        return next(s, f, tx);
    }
    if s.dock().typed.get(&f.key(tab)).is_none_or(|t| t.text.trim().is_empty()) {
        tx.instant();
        return start_typing(s, f, tab);
    }
    s.dock_mut().custom.insert(f.key(tab));
    next(s, f, tx);
}

/// A key while the dock shows; false when it isn't the dock's.
pub fn key<S: DockHost>(s: &mut S, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if classic(s) {
        return classic::key(s, ev, tx);
    }
    let Some(f) = asking(s) else { return false };
    let (tab, focus) = place(s, &f);
    let field = &f.fields[tab];
    let n = rows(field).len();
    let zone = if editing(s, &f, tab) {
        Zone::Input
    } else if focus >= n && has_custom(field) {
        Zone::Custom
    } else {
        Zone::Option
    };
    let intent = intent(ev, zone, count(field));
    // A reply on its way: the dock's keys do nothing (Desktop's `nav` while `sending`).
    if busy(s, &f) {
        return intent != Intent::None;
    }
    match intent {
        Intent::None => return false,
        Intent::Dismiss => dismiss(s, &f, tx),
        // Next and Submit are lit only on an answered question; → skips one.
        Intent::Advance => {
            if answered(s, &f, tab) {
                next(s, &f, tx)
            }
        }
        // ⌘[ on the first question isn't the dock's: it goes back through the history.
        Intent::Question(-1) if tab == 0 && matches!(ev.key, Key::Command(_)) => return false,
        Intent::Question(step) => {
            if step > 0 && !f.last(tab) {
                jump(s, &f, tab + 1, tx);
            } else if step < 0 && tab > 0 {
                jump(s, &f, tab - 1, tx);
            }
        }
        Intent::Commit => commit(s, &f, tx),
        Intent::Type(c) => {
            tx.instant();
            let t = s.dock_mut().typed_mut(&f.id, &field.key);
            *t = TextEdit::new(format!("{}{c}", t.text));
            start_typing(s, &f, tab);
        }
        Intent::Number(i) => {
            tx.instant();
            if i >= n {
                toggle_custom(s, &f, tab);
            } else {
                select(s, &f, tab, i);
                dock(s, &f).focus = i;
            }
        }
        Intent::Activate => {
            tx.instant();
            if focus >= n { toggle_custom(s, &f, tab) } else { select(s, &f, tab, focus) }
        }
        Intent::Move(step) => {
            tx.instant();
            dock(s, &f).focus = (focus as i32 + step).clamp(0, count(field) as i32 - 1) as usize;
        }
        Intent::First | Intent::Last => {
            tx.instant();
            dock(s, &f).focus = if ev.key == Key::Home { 0 } else { count(field) - 1 };
        }
    }
    reveal_focus(s, tx);
    true
}

/// Keys in "Type your own answer": Esc leaves the field, ↑ on its first line goes to the answer
/// above, ↵ moves on, ⌘↵ and ⌘[ as anywhere. Everything else types.
fn typed_key<S: DockHost>(s: &mut S, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Some(f) = asking(s) else { return false };
    let (tab, _) = place(s, &f);
    let n = rows(&f.fields[tab]).len();
    let m = ev.mods;
    match ev.key {
        Key::Escape => {
            tx.instant();
            stop_typing(s, &f, tab);
            dock(s, &f).focus = n;
            true
        }
        Key::Up if n > 0 && !m.shift && !m.alt && !m.cmd && !m.ctrl && s.dock().typed.get(&f.key(tab)).is_none_or(|t| face::on_first_line(t, s.dock().input_w)) => {
            tx.instant();
            stop_typing(s, &f, tab);
            dock(s, &f).focus = n - 1;
            reveal_focus(s, tx);
            true
        }
        Key::Enter if !ev.mods.shift && !ev.mods.cmd && !ev.mods.ctrl => {
            if answered(s, &f, tab) {
                next(s, &f, tx);
            }
            true
        }
        _ => key(s, ev, tx) || s.dock_shortcut(ev, tx),
    }
}

fn hover_answer<S: DockHost>(s: &mut S, i: usize, tx: &mut Tx) {
    let Some(f) = asking(s) else { return };
    let (tab, _) = place(s, &f);
    if !editing(s, &f, tab) {
        tx.instant();
        dock(s, &f).focus = i;
    }
}

/// The tallest the header and answers get, the footer under them taking the rest of the cap;
/// None on a stage with no pane to share (the composer stories).
fn body_max<S: DockHost>(s: &S) -> Option<f32> {
    let pane = s.dock_pane_h()?;
    // Never so tall the timeline goes: in a short pane (1280×640 at 1.6×) the 240 pt floor gives
    // way, so a fifth of the pane under the header still shows the conversation; the answers
    // scroll inside whatever the dock gets.
    let keep = pane * TIMELINE_KEEP + HEADER_H;
    let cap = (pane * s.knobs().get(&knobs::QUESTION_MAX)).max(MIN_H).min(pane - keep).max(FOOTER_H);
    Some((cap - FOOTER_H).max(0.0))
}

/// Brings the highlighted answer, or "Type your own answer", into sight; on the first answer as
/// much of the question above it as fits shows too (reveals run in order, each from where the
/// last left the list).
pub fn reveal_focus<S: DockHost>(s: &S, tx: &mut Tx) {
    let Some(f) = asking(s) else { return };
    let (tab, focus) = place(s, &f);
    let field = &f.fields[tab];
    let scope = format!("q-scroll-{tab}");
    if focus == 0 {
        tx.scroll_into_view_in(scope.clone(), format!("q-details-{tab}"));
    }
    let key = if focus >= rows(field).len() && has_custom(field) { "q-custom".to_owned() } else { format!("q-opt-{focus}") };
    tx.scroll_into_view_in(scope, key);
}
