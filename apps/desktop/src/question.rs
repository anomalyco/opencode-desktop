//! The question dock: Desktop's `session-question-dock.tsx` and `question-keys.ts`. When the
//! open session waits on a question, the composer's surface morphs into it (`ComposerMorph`,
//! `spring-height.tsx`): one card whose height springs on [`MORPH`] while the composer's face
//! fades out and the question's fades up. Answering or dismissing it, here or anywhere else,
//! morphs it back with the draft as it was.
//!
//! Keys: ↑/↓ (Home/End) move the highlight, 1–9 pick, Space toggles, ↵ picks the highlighted
//! answer and moves on (submitting on the last question), ←/→ and ⌘[ change question, ⌘↵ goes
//! on or submits, Esc dismisses. A printable key on "Type your own answer" starts typing it.

use std::collections::HashSet;

use opencode_kit::model::{Field, FieldKind, Pending};
use rune::{Anim, Color, El, Key, KeyEvent, Spring, SpringPair, TextEdit, TextStyle, Transition, Tx, col, el, row, spacer, text, text_input};

use crate::palette::Pal;
use crate::{App, Route};

/// The composer ⇄ question morph: Desktop's `docks.morph` ("smooth", 0.4 s, no bounce), shared
/// by opening and closing so the dock leaves the way it came.
pub const MORPH: Spring = Spring::new(0.4, 0.0);
/// The face that leaves fades out quickly; the one that arrives fades up on the morph.
const FADE_OUT: Spring = Spring::new(0.15, 0.0);
/// A new question's text and answers slide in from the side they come from (`arrive`) over
/// the old ones fading out where they stood.
const ARRIVE: Spring = Spring::new(0.3, 0.0);

const TITLE: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0);
const STRONG: TextStyle = TextStyle::new(13.0, 530.0).lh(20.0);
const HINT: TextStyle = TextStyle::new(11.0, 440.0).lh(12.0);
const MARK: TextStyle = TextStyle::new(11.0, 530.0).lh(12.0).features("\"tnum\"");
const BUTTON: TextStyle = TextStyle::new(13.0, 530.0).lh(20.0).tracking(-0.04);
const CUSTOM: &str = "Type your own answer…";

/// Where the dock is for the form it shows: the question, the highlighted answer, and which
/// questions have their own typed answer switched on.
#[derive(Default)]
pub struct Dock {
    form: String,
    tab: usize,
    focus: usize,
    /// -1 or 1: the way the last question change went, for the arrival slide.
    step: i32,
    /// `form/key`s whose typed answer counts.
    custom: HashSet<String>,
}

impl Dock {
    /// Whether the typed answer at `form/key` counts towards the reply.
    pub fn custom_on(&self, key: &str) -> bool {
        self.custom.contains(key)
    }

    pub fn forget(&mut self, form: &str) {
        let prefix = format!("{form}/");
        self.custom.retain(|k| !k.starts_with(&prefix));
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

/// `questionKey` for a Mac: `count` answers including the typed one.
pub fn intent(ev: &KeyEvent, zone: Zone, count: usize) -> Intent {
    let m = ev.mods;
    match &ev.key {
        Key::Escape => return Intent::Dismiss,
        Key::Command('[') if !m.ctrl && !m.alt => return Intent::Question(-1),
        Key::Enter if (m.cmd || m.ctrl) && !m.alt => return Intent::Advance,
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
fn rows(f: &Field) -> Vec<(String, String, String)> {
    if f.kind == FieldKind::Boolean {
        return vec![("true".into(), "Yes".into(), String::new()), ("false".into(), "No".into(), String::new())];
    }
    f.options.iter().map(|o| (o.value.clone(), if o.label.is_empty() { o.value.clone() } else { o.label.clone() }, o.description.clone())).collect()
}

fn has_custom(f: &Field) -> bool {
    f.kind != FieldKind::Boolean && (f.custom || f.options.is_empty())
}

fn count(f: &Field) -> usize {
    rows(f).len() + has_custom(f) as usize
}

/// The question form the open session waits on.
#[derive(Clone)]
struct Form {
    id: String,
    fields: Vec<Field>,
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

impl App {
    fn asking(&self) -> Option<Form> {
        let Route::Session(id) = &self.route else { return None };
        // A subagent's page has no composer to stand in for.
        if self.session(id).is_some_and(|i| i.parent.is_some()) {
            return None;
        }
        self.pending_for(id).into_iter().find_map(|p| match p {
            Pending::Form { id, fields, .. } if !fields.is_empty() => Some(Form { id: id.clone(), fields: fields.clone() }),
            _ => None,
        })
    }

    /// Whether the composer shows as the question dock.
    pub fn question_shown(&self) -> bool {
        self.asking().is_some()
    }

    /// The question and highlighted answer for `form`, as the dock left them or fresh.
    fn place(&self, f: &Form) -> (usize, usize) {
        if self.question.form == f.id {
            (self.question.tab.min(f.fields.len() - 1), self.question.focus)
        } else {
            (0, self.pick_focus(f, 0))
        }
    }

    fn dock(&mut self, f: &Form) -> &mut Dock {
        if self.question.form != f.id {
            let focus = self.pick_focus(f, 0);
            let custom = std::mem::take(&mut self.question.custom);
            self.question = Dock { form: f.id.clone(), tab: 0, focus, step: 0, custom };
        }
        &mut self.question
    }

    /// The answer a question opens on: its typed one when that's on, else its first pick.
    fn pick_focus(&self, f: &Form, tab: usize) -> usize {
        let k = f.key(tab);
        let list = rows(&f.fields[tab]);
        if self.question.custom_on(&k) && has_custom(&f.fields[tab]) {
            return list.len();
        }
        let picks = self.picks.get(&k);
        list.iter().position(|(v, ..)| picks.is_some_and(|p| p.contains(v))).unwrap_or(0)
    }

    fn editing(&self, f: &Form, tab: usize) -> bool {
        self.typed.get(&f.key(tab)).is_some_and(|t| t.focused)
    }

    fn answered(&self, f: &Form, tab: usize) -> bool {
        let k = f.key(tab);
        self.picks.get(&k).is_some_and(|p| !p.is_empty())
            || (self.question.custom_on(&k) && self.typed.get(&k).is_some_and(|t| !t.text.trim().is_empty()))
    }

    fn stop_typing(&mut self, f: &Form, tab: usize) {
        if let Some(t) = self.typed.get_mut(&f.key(tab)) {
            t.focused = false;
        }
    }

    /// Picks answer `i`: the one answer of a single choice, or one more (or one fewer) of many.
    fn select(&mut self, f: &Form, tab: usize, i: usize) {
        let Some((value, ..)) = rows(&f.fields[tab]).into_iter().nth(i) else { return };
        let k = f.key(tab);
        let picks = self.picks.entry(k.clone()).or_default();
        if f.multi(tab) {
            match picks.iter().position(|v| *v == value) {
                Some(at) => {
                    picks.remove(at);
                }
                None => picks.push(value),
            }
        } else {
            *picks = vec![value];
            self.question.custom.remove(&k);
        }
        self.stop_typing(f, tab);
    }

    fn start_typing(&mut self, f: &Form, tab: usize) {
        let k = f.key(tab);
        self.dock(f).focus = rows(&f.fields[tab]).len();
        self.question.custom.insert(k.clone());
        if !f.multi(tab) {
            self.picks.remove(&k);
        }
        self.composer.focused = false;
        self.typed_mut(&f.id, &f.fields[tab].key).focused = true;
    }

    fn toggle_custom(&mut self, f: &Form, tab: usize) {
        let k = f.key(tab);
        if !f.multi(tab) || !self.question.custom_on(&k) {
            return self.start_typing(f, tab);
        }
        self.question.custom.remove(&k);
        self.stop_typing(f, tab);
        self.dock(f).focus = rows(&f.fields[tab]).len();
    }

    fn jump(&mut self, f: &Form, tab: usize, tx: &mut Tx) {
        let (from, _) = self.place(f);
        if tab == from || tab >= f.fields.len() {
            return;
        }
        self.stop_typing(f, from);
        let focus = self.pick_focus(f, tab);
        let d = self.dock(f);
        d.step = if tab > from { 1 } else { -1 };
        d.tab = tab;
        d.focus = focus;
        tx.motion(MORPH);
    }

    /// Next question, or the reply on the last one.
    fn next(&mut self, f: &Form, tx: &mut Tx) {
        let (tab, _) = self.place(f);
        self.stop_typing(f, tab);
        if !f.last(tab) {
            return self.jump(f, tab + 1, tx);
        }
        self.submit_form(&f.id);
        self.closed_if_gone(f, tx);
    }

    fn dismiss(&mut self, f: &Form, tx: &mut Tx) {
        self.dismiss_form(&f.id);
        self.closed_if_gone(f, tx);
    }

    fn closed_if_gone(&mut self, f: &Form, tx: &mut Tx) {
        if !self.pending.iter().any(|p| p.id() == f.id) {
            self.morph(tx);
        }
    }

    /// The dock opened or closed: the surface springs to its new face, and the composer
    /// gets the keyboard back.
    pub fn morph(&mut self, tx: &mut Tx) {
        tx.motion(MORPH);
        if !self.question_shown() {
            self.composer.focused = true;
        }
    }

    /// Enter on a single choice picks the highlighted answer and moves on; an empty typed
    /// answer opens its field instead.
    fn commit(&mut self, f: &Form, tx: &mut Tx) {
        let (tab, focus) = self.place(f);
        if f.multi(tab) {
            return self.next(f, tx);
        }
        if focus < rows(&f.fields[tab]).len() {
            self.select(f, tab, focus);
            return self.next(f, tx);
        }
        if self.typed.get(&f.key(tab)).is_none_or(|t| t.text.trim().is_empty()) {
            tx.instant();
            return self.start_typing(f, tab);
        }
        self.question.custom.insert(f.key(tab));
        self.next(f, tx);
    }

    /// A key while the dock shows; false when it isn't the dock's.
    pub fn question_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let Some(f) = self.asking() else { return false };
        let (tab, focus) = self.place(&f);
        let field = &f.fields[tab];
        let n = rows(field).len();
        let zone = if self.editing(&f, tab) {
            Zone::Input
        } else if focus >= n && has_custom(field) {
            Zone::Custom
        } else {
            Zone::Option
        };
        match intent(ev, zone, count(field)) {
            Intent::None => return false,
            Intent::Dismiss => self.dismiss(&f, tx),
            Intent::Advance => self.next(&f, tx),
            Intent::Question(step) => {
                if step > 0 && !f.last(tab) {
                    self.jump(&f, tab + 1, tx);
                } else if step < 0 && tab > 0 {
                    self.jump(&f, tab - 1, tx);
                }
            }
            Intent::Commit => self.commit(&f, tx),
            Intent::Type(c) => {
                tx.instant();
                let t = self.typed_mut(&f.id, &field.key);
                *t = TextEdit::new(format!("{}{c}", t.text));
                self.start_typing(&f, tab);
            }
            Intent::Number(i) => {
                tx.instant();
                if i >= n {
                    self.toggle_custom(&f, tab);
                } else {
                    self.select(&f, tab, i);
                    self.dock(&f).focus = i;
                }
            }
            Intent::Activate => {
                tx.instant();
                if focus >= n { self.toggle_custom(&f, tab) } else { self.select(&f, tab, focus) }
            }
            Intent::Move(step) => {
                tx.instant();
                self.dock(&f).focus = (focus as i32 + step).clamp(0, count(field) as i32 - 1) as usize;
            }
            Intent::First | Intent::Last => {
                tx.instant();
                self.dock(&f).focus = if ev.key == Key::Home { 0 } else { count(field) - 1 };
            }
        }
        true
    }

    /// Keys in "Type your own answer": Esc leaves the field, ↵ moves on, ⌘↵ and ⌘[ as anywhere.
    fn typed_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let Some(f) = self.asking() else { return false };
        let (tab, _) = self.place(&f);
        match ev.key {
            Key::Escape => {
                tx.instant();
                self.stop_typing(&f, tab);
                self.dock(&f).focus = rows(&f.fields[tab]).len();
                true
            }
            Key::Enter if !ev.mods.shift && !ev.mods.cmd && !ev.mods.ctrl => {
                self.next(&f, tx);
                true
            }
            _ => self.question_key(ev, tx) || self.shortcut(ev, tx),
        }
    }

    fn hover_answer(&mut self, i: usize, tx: &mut Tx) {
        let Some(f) = self.asking() else { return };
        let (tab, _) = self.place(&f);
        if !self.editing(&f, tab) {
            tx.instant();
            self.dock(&f).focus = i;
        }
    }
}

/// How the composer's face and the question's swap inside the one surface.
pub fn face_transition() -> Transition {
    Transition::asymmetric(Transition::offset(0.0, 6.0).combined(Transition::opacity()), Transition::opacity())
        .springs(SpringPair { enter: MORPH, exit: FADE_OUT })
}

fn arrival(step: i32) -> Transition {
    Transition::asymmetric(Transition::offset(12.0 * step as f32, 0.0).combined(Transition::opacity()), Transition::opacity())
        .springs(SpringPair { enter: ARRIVE, exit: FADE_OUT })
}

#[derive(Clone, Copy, PartialEq)]
enum Variant {
    Ghost,
    Neutral,
    Submit,
}

/// `button-v2` at size large.
fn button(p: &Pal, key: &str, label: &str, variant: Variant, hint: Option<&str>) -> El {
    let (fill, fg) = match variant {
        Variant::Ghost => (Color::TRANSPARENT, p.muted),
        Variant::Neutral => (p.q.neutral, p.text),
        Variant::Submit => (p.q.submit, p.q.submit_text),
    };
    let mut b = row()
        .key(key)
        .role("button")
        .label(label.to_owned())
        .h(32.0)
        .px(15.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .fill(fill)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(Anim::INSTANT, Anim::INSTANT)
        .child(text(label.to_owned()).font(BUTTON).color(fg))
        .children(hint.map(|h| text(h.to_owned()).key("hint").font(BUTTON).color(fg).opacity(0.6)));
    match variant {
        Variant::Ghost => {}
        Variant::Neutral => b = b.border(0.5, p.q.neutral_ring).shadow(1.0, 1.5, p.q.neutral_shadow),
        Variant::Submit => b = b.border(0.5, p.q.submit_ring).shadow(1.0, 1.5, p.q.neutral_shadow),
    }
    b
}

/// An answer's key and mark: round for one answer, square for many, filled once picked.
fn mark(p: &Pal, multi: bool, picked: bool, index: usize) -> El {
    let key = el().size(18.0, 18.0).center().radius(if multi { 4.0 } else { 9.0 });
    let key = match (picked, multi) {
        (true, _) => key.fill(p.bg_accent),
        (false, false) => key.fill(p.layer3).border(0.5, p.border),
        (false, true) => key.border(1.0, p.q.border_strong),
    };
    let ink = if picked { p.q.contrast_text } else { p.muted };
    let key = key.children((index < 9).then(|| text((index + 1).to_string()).font(MARK).color(ink)));
    el().key("mark").size(18.0, 20.0).center().child(key)
}

/// The question's face of the composer surface: header, answers and footer.
pub fn face(s: &App, p: &Pal) -> Option<Vec<El>> {
    let f = s.asking()?;
    let (tab, focus) = s.place(&f);
    let field = &f.fields[tab];
    let multi = f.multi(tab);
    let k = f.key(tab);
    let editing = s.editing(&f, tab);
    let list = rows(field);
    let focus = if editing { list.len() } else { focus };
    let picks = s.picks.get(&k).cloned().unwrap_or_default();
    let total = f.fields.len();

    let title = if total == 1 { "Question".to_owned() } else { format!("{} of {total} questions", tab + 1) };
    let progress = (total > 1).then(|| {
        row().key("q-progress").gap(2.0).children((0..total).map(|i| {
            let color = if i == tab { p.icon_muted } else if s.answered(&f, i) { p.bg_accent } else { p.q.layer4 };
            let form = f.clone();
            el().key(format!("q-seg-{i}"))
                .role("button")
                .label(format!("Question {}", i + 1))
                .size(20.0, 20.0)
                .radius(4.0)
                .center()
                .hover(p.hover)
                .hover_motion(Anim::INSTANT, Anim::INSTANT)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| s.jump(&form, i, tx))
                .child(el().size(14.0, 2.0).fill(color))
        }))
    });
    let text_of = if field.description.is_empty() { &field.title } else { &field.description };
    let header = col()
        .key("q-header")
        .w_full()
        .px(10.0)
        .gap(4.0)
        .child(row().key("q-top").w_full().min_h(28.0).gap(12.0).center_y().child(text(title).key("q-title").font(TITLE).color(p.muted)).child(spacer()).children(progress))
        .child(
            col()
                .key(format!("q-details-{tab}"))
                .w_full()
                .gap(4.0)
                .transition(arrival(s.question.step))
                .child(text(text_of.clone()).key("q-text").font(STRONG).color(p.text).wrap())
                .children(multi.then(|| text("Pick any").key("q-hint").font(HINT).color(p.faint))),
        );

    let mut answers = col().key(format!("q-options-{tab}")).w_full().gap(2.0).role(if multi { "group" } else { "radiogroup" }).transition(arrival(s.question.step));
    for (i, (value, label, desc)) in list.iter().enumerate() {
        let picked = picks.contains(value);
        let form = f.clone();
        answers = answers.child(
            row()
                .key(format!("q-opt-{i}"))
                .role(if multi { "checkbox" } else { "radio" })
                .label(label.clone())
                .w_full()
                .padding(10.0)
                .gap(8.0)
                .items_start()
                .radius(6.0)
                .fill(if i == focus { p.q.selected } else { Color::TRANSPARENT })
                .on_hover_change(move |s: &mut App, inside, tx: &mut Tx| if inside { s.hover_answer(i, tx) })
                .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    let (tab, _) = s.place(&form);
                    s.select(&form, tab, i);
                    s.dock(&form).focus = i;
                })
                .child(mark(p, multi, picked, i))
                .child(
                    col()
                        .key("main")
                        .grow(1.0)
                        .gap(4.0)
                        .child(text(label.clone()).key("label").font(STRONG).color(p.text).wrap())
                        .children((!desc.is_empty()).then(|| text(desc.clone()).key("description").font(TITLE).color(p.muted).wrap())),
                ),
        );
    }
    if has_custom(field) {
        let i = list.len();
        let on = s.question.custom_on(&k);
        let edit = s.typed.get(&k).cloned().unwrap_or_else(|| TextEdit::new(""));
        let (a, b, c, d) = (f.clone(), f.clone(), f.clone(), field.key.clone());
        let id = f.id.clone();
        let mut input = text_input(&edit, if editing { TITLE } else { STRONG })
            .key("q-input")
            .label(CUSTOM)
            .w_full()
            .h(28.0)
            .center_y()
            .radius(6.0)
            .placeholder(CUSTOM, if editing { p.faint } else { p.text })
            .caret(p.text, p.selection)
            .on_edit(move |s: &mut App| s.typed_mut(&id, &d))
            .on_input_change(move |s: &mut App, _| {
                let (tab, _) = s.place(&c);
                s.question.custom.insert(c.key(tab));
                if !c.multi(tab) {
                    s.picks.remove(&c.key(tab));
                }
            })
            .on_input_key(|s: &mut App, ev, tx| s.typed_key(ev, tx));
        if editing {
            input = input.px(8.0).fill(p.bg_base).border(1.0, p.q.focus);
        }
        answers = answers.child(
            row()
                .key("q-custom")
                .w_full()
                .h(40.0)
                .px(10.0)
                .gap(8.0)
                .center_y()
                .radius(6.0)
                .fill(if i == focus { p.q.selected } else { Color::TRANSPARENT })
                .on_hover_change(move |s: &mut App, inside, tx: &mut Tx| if inside { s.hover_answer(i, tx) })
                .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    let (tab, _) = s.place(&a);
                    s.start_typing(&a, tab);
                })
                .child(mark(p, multi, on, i).role(if multi { "checkbox" } else { "radio" }).label(CUSTOM).on_press_tx(move |s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    let (tab, _) = s.place(&b);
                    s.toggle_custom(&b, tab);
                }))
                .child(input),
        );
    }

    let body = col().key("question").w_full().padding(8.0).gap(16.0).transition(face_transition()).child(header).child(answers);

    let last = f.last(tab);
    let (back, next, dismiss) = (f.clone(), f.clone(), f.clone());
    let footer = row()
        .key("question-footer")
        .w_full()
        .px(12.0)
        .pt(7.0)
        .pb(8.0)
        .center_y()
        .transition(face_transition())
        .child(button(p, "q-dismiss", "Dismiss", Variant::Ghost, None).on_press_tx(move |s: &mut App, tx: &mut Tx| s.dismiss(&dismiss, tx)))
        .child(spacer())
        .child(
            row()
                .key("q-actions")
                .gap(8.0)
                .children((tab > 0).then(|| {
                    button(p, "q-back", "Back", Variant::Neutral, None).on_press_tx(move |s: &mut App, tx: &mut Tx| {
                        let (tab, _) = s.place(&back);
                        s.jump(&back, tab.saturating_sub(1), tx);
                    })
                }))
                .child(
                    button(p, "q-next", if last { "Submit" } else { "Next" }, if last { Variant::Submit } else { Variant::Neutral }, Some("⌘⏎"))
                        .on_press_tx(move |s: &mut App, tx: &mut Tx| s.next(&next, tx)),
                ),
        );
    Some(vec![body, footer])
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{Receiver, channel};

    use opencode_kit::client::{Server, Write};
    use rune::{Env, Key, KeyEvent, Mods, Runtime, TextEdit};
    use serde_json::json;

    use super::{Intent, Zone, intent};
    use crate::{App, Source};

    const FORM: &str = "frm_fx0001";
    const SESSION: &str = "ses_fx0001aaaaaaaaaaaaaaaaaa";

    fn ev(key: Key) -> KeyEvent {
        KeyEvent { key, mods: Mods::default() }
    }

    fn cmd(key: Key) -> KeyEvent {
        KeyEvent { key, mods: Mods { cmd: true, ..Mods::default() } }
    }

    fn txt(s: &str) -> KeyEvent {
        ev(Key::Text(s.into()))
    }

    #[test]
    fn keys_map_as_desktops() {
        let o = Zone::Option;
        assert_eq!(intent(&ev(Key::Escape), Zone::Input, 4), Intent::Dismiss);
        assert_eq!(intent(&ev(Key::Down), o, 4), Intent::Move(1));
        assert_eq!(intent(&ev(Key::Up), o, 4), Intent::Move(-1));
        assert_eq!(intent(&ev(Key::Home), o, 4), Intent::First);
        assert_eq!(intent(&ev(Key::End), o, 4), Intent::Last);
        assert_eq!(intent(&ev(Key::Left), o, 4), Intent::Question(-1));
        assert_eq!(intent(&ev(Key::Right), o, 4), Intent::Question(1));
        assert_eq!(intent(&cmd(Key::Command('[')), o, 4), Intent::Question(-1));
        assert_eq!(intent(&cmd(Key::Enter), Zone::Input, 4), Intent::Advance);
        assert_eq!(intent(&txt("2"), o, 4), Intent::Number(1));
        assert_eq!(intent(&txt("4"), o, 4), Intent::Number(3));
        assert_eq!(intent(&txt("5"), o, 4), Intent::None, "past the answers");
        assert_eq!(intent(&txt("0"), o, 4), Intent::None);
        assert_eq!(intent(&txt(" "), o, 4), Intent::Activate);
        assert_eq!(intent(&ev(Key::Enter), o, 4), Intent::Commit);
        assert_eq!(intent(&KeyEvent { key: Key::Enter, mods: Mods { shift: true, ..Mods::default() } }, o, 4), Intent::None);
        assert_eq!(intent(&txt("a"), o, 4), Intent::None);
        assert_eq!(intent(&txt("a"), Zone::Custom, 4), Intent::Type("a".into()));
        assert_eq!(intent(&txt("1"), Zone::Custom, 4), Intent::Number(0), "digits still pick on the typed answer");
        assert_eq!(intent(&txt("2"), Zone::Input, 4), Intent::None, "typing in the field");
        assert_eq!(intent(&ev(Key::Down), Zone::Input, 4), Intent::None);
        assert_eq!(intent(&KeyEvent { key: Key::Down, mods: Mods { alt: true, ..Mods::default() } }, o, 4), Intent::None, "⌥↓ switches tabs");
        assert_eq!(intent(&KeyEvent { key: Key::Down, mods: Mods { shift: true, ..Mods::default() } }, o, 4), Intent::None);
        assert_eq!(intent(&cmd(Key::Command('t')), o, 4), Intent::None, "app shortcuts pass");
    }

    /// The fixture session with its question asked and a draft in the composer. Live sends go
    /// to a channel, never a server; a fixture run polls its own playback instead.
    fn asked(live: bool) -> (Runtime<App>, Option<Receiver<Write>>) {
        let mut app = App::empty();
        app.load_fixture(crate::FIXTURE);
        crate::open_scene(&mut app);
        app.finish_stream();
        app.composer = TextEdit::new("half a thought");
        let q = app.fixture["question"].clone();
        app.apply_event("form.created", &q);
        let writes = live.then(|| {
            let (w, r) = channel();
            app.source = Source::Live { server: Server::new("http://127.0.0.1:9"), requests: None, writes: Some(w), allow_send: true };
            r
        });
        let mut rt = Runtime::new(app, crate::view::app, (1280.0, 820.0), Env::new(true, 1.0));
        if !live {
            rt.poll = Some(crate::poll);
        }
        rt.on_key = Some(crate::on_key);
        settle(&mut rt);
        (rt, writes)
    }

    fn settle(rt: &mut Runtime<App>) {
        for _ in 0..90 {
            rt.frame(1.0 / 120.0);
        }
    }

    fn press(rt: &mut Runtime<App>, e: KeyEvent) {
        rt.key(e);
        rt.frame(1.0 / 120.0);
    }

    fn picks(rt: &Runtime<App>, key: &str) -> Vec<String> {
        rt.state.picks.get(&format!("{FORM}/{key}")).cloned().unwrap_or_default()
    }

    fn shows_composer(rt: &Runtime<App>) -> bool {
        rt.find("prompt").is_some() && rt.find("question").is_none()
    }

    #[test]
    fn the_composer_becomes_the_question() {
        let (rt, _) = asked(true);
        assert!(rt.find("question").is_some() && rt.find("q-opt-0").is_some());
        assert!(rt.find("prompt").is_none() && rt.find("toolbar").is_none(), "one surface, not a card above the composer");
        assert!(rt.find(&format!("request-{FORM}")).is_none());
        let title = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("q-title")).and_then(|e| e.text);
        assert_eq!(title.as_deref(), Some("1 of 2 questions"));
    }

    #[test]
    fn arrows_move_digits_pick_and_enter_submits() {
        let (mut rt, writes) = asked(true);
        let focus = |rt: &Runtime<App>| rt.state.question.focus;
        press(&mut rt, ev(Key::Down));
        assert_eq!(focus(&rt), 1);
        for _ in 0..5 {
            press(&mut rt, ev(Key::Down));
        }
        assert_eq!(focus(&rt), 3, "stops on the typed answer");
        press(&mut rt, ev(Key::Home));
        assert_eq!(focus(&rt), 0);
        press(&mut rt, ev(Key::End));
        press(&mut rt, ev(Key::Up));
        assert_eq!(focus(&rt), 2);
        assert!(picks(&rt, "q0").is_empty(), "moving only highlights");

        press(&mut rt, txt("2"));
        assert_eq!(picks(&rt, "q0"), ["Real timers"]);
        assert_eq!(focus(&rt), 1);
        press(&mut rt, ev(Key::Down));
        // Enter picks the highlighted answer and moves on.
        press(&mut rt, ev(Key::Enter));
        assert_eq!(picks(&rt, "q0"), ["Only the math"]);
        assert_eq!(rt.state.question.tab, 1);
        settle(&mut rt);
        let hint = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("q-hint") && !e.exiting).and_then(|e| e.text);
        assert_eq!(hint.as_deref(), Some("Pick any"));

        press(&mut rt, txt("1"));
        press(&mut rt, txt("3"));
        press(&mut rt, txt(" "));
        press(&mut rt, txt(" "));
        assert_eq!(picks(&rt, "q1"), ["Reset after success", "Jitter stays in range"]);
        press(&mut rt, ev(Key::Left));
        assert_eq!(rt.state.question.tab, 0);
        assert_eq!(focus(&rt), 2, "back on its pick");
        press(&mut rt, ev(Key::Right));
        assert_eq!(rt.state.question.tab, 1);
        assert!(writes.as_ref().unwrap().try_recv().is_err(), "nothing sent yet");

        press(&mut rt, ev(Key::Enter));
        let Ok(Write::Form { session, form, answer }) = writes.unwrap().try_recv() else { panic!("no reply") };
        assert_eq!((session.as_str(), form.as_str()), (SESSION, FORM));
        assert_eq!(answer, json!({ "q0": "Only the math", "q1": ["Reset after success", "Jitter stays in range"] }));
        settle(&mut rt);
        assert!(shows_composer(&rt));
        assert_eq!(rt.state.composer.text, "half a thought");
        assert!(rt.state.composer.focused, "the composer has the keyboard back");
    }

    #[test]
    fn escape_dismisses() {
        let (mut rt, writes) = asked(true);
        press(&mut rt, ev(Key::Escape));
        let Ok(Write::CancelForm { session, form }) = writes.unwrap().try_recv() else { panic!("no cancel") };
        assert_eq!((session.as_str(), form.as_str()), (SESSION, FORM));
        settle(&mut rt);
        assert!(shows_composer(&rt));
        assert_eq!(rt.state.composer.text, "half a thought");
    }

    #[test]
    fn a_typed_answer_goes_in_the_reply() {
        let (mut rt, writes) = asked(true);
        press(&mut rt, txt("4"));
        assert!(rt.state.typed.get(&format!("{FORM}/q0")).is_some_and(|t| t.focused), "4 opens the field");
        for c in ["B", "o", "t", "h"] {
            press(&mut rt, txt(c));
        }
        // Esc leaves the field without dismissing; a printable key goes back to typing.
        press(&mut rt, ev(Key::Escape));
        assert!(writes.as_ref().unwrap().try_recv().is_err());
        press(&mut rt, txt("!"));
        press(&mut rt, ev(Key::Enter));
        assert_eq!(rt.state.question.tab, 1);
        press(&mut rt, cmd(Key::Enter));
        let Ok(Write::Form { answer, .. }) = writes.unwrap().try_recv() else { panic!("no reply") };
        assert_eq!(answer, json!({ "q0": "Both!" }));
    }

    #[test]
    fn a_click_picks_and_submit_replies() {
        let (mut rt, writes) = asked(true);
        let click = |rt: &mut Runtime<App>, key: &str| {
            let (x, y) = rt.find(key).unwrap_or_else(|| panic!("{key}"));
            rt.pointer_move(x, y);
            rt.pointer_down(x, y);
            rt.pointer_up(x, y);
            settle(rt);
        };
        click(&mut rt, "q-opt-0");
        assert_eq!(picks(&rt, "q0"), ["Fake clock"]);
        click(&mut rt, "q-next");
        click(&mut rt, "q-opt-1");
        click(&mut rt, "q-next");
        let Ok(Write::Form { answer, .. }) = writes.unwrap().try_recv() else { panic!("no reply") };
        assert_eq!(answer, json!({ "q0": "Fake clock", "q1": ["Cap at 30 s"] }));
    }

    /// The surface's height each frame through a change made by `event` from another client.
    fn morph(rt: &mut Runtime<App>, ty: &str, data: serde_json::Value) -> Vec<f32> {
        let at = rt.state.clock + 0.01;
        rt.state.playback.push_back((at, ty.into(), data));
        let height = |rt: &Runtime<App>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("composer")).unwrap().rect[3];
        let mut hs = vec![height(rt)];
        for _ in 0..120 {
            rt.frame(1.0 / 120.0);
            hs.push(height(rt));
        }
        hs
    }

    /// Springs from one height to the other: monotonic, no overshoot, no frame covering more
    /// than a fifth of the way, settled at the end.
    fn springs(hs: &[f32], what: &str) {
        let (a, b) = (hs[0], *hs.last().unwrap());
        assert!((a - b).abs() > 40.0, "{what}: {a} → {b}");
        let dir = (b - a).signum();
        for w in hs.windows(2) {
            assert!((w[1] - w[0]) * dir >= -0.01, "{what}: turns back at {w:?}");
            assert!((w[1] - w[0]).abs() < (b - a).abs() / 5.0, "{what}: jumps {w:?}");
        }
        assert!(hs.iter().all(|h| (h - a) * dir >= -0.01 && (h - b) * dir <= 0.01), "{what}: overshoots");
        // Within 2 % of the way in about the morph's 0.4 s.
        let done = hs.iter().position(|h| (h - b).abs() < (b - a).abs() * 0.02).unwrap();
        assert!((30..=60).contains(&done), "{what}: lands after {done} frames at 120 Hz");
    }

    #[test]
    fn an_answer_from_elsewhere_morphs_back_and_keeps_the_draft() {
        let (mut rt, _) = asked(false);
        assert!(!shows_composer(&rt));
        let hs = morph(&mut rt, "form.replied", json!({ "id": FORM, "sessionID": SESSION }));
        springs(&hs, "closing");
        assert!(shows_composer(&rt));
        assert_eq!(rt.state.composer.text, "half a thought");
        let q = rt.state.fixture["question"].clone();
        let hs = morph(&mut rt, "form.created", q);
        springs(&hs, "opening");
        assert!(rt.find("q-opt-0").is_some());
    }
}
