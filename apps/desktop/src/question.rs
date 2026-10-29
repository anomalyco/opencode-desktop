//! The question dock: Desktop's `session-question-dock.tsx` and `question-keys.ts`. When the
//! open session waits on a question, the composer's surface morphs into it (`ComposerMorph`,
//! `spring-height.tsx`): one card whose height springs on Desktop's `docks.morph` (0.4 s, no
//! bounce) while the composer's face fades out and the question's fades up. The curves and
//! timings are knobs (⌘⇧K → Question dock, `knobs.rs`) whose defaults are Desktop's. Answering or dismissing it, here or anywhere else,
//! morphs it back with the draft as it was.
//!
//! Keys: ↑/↓ (Home/End) move the highlight, 1–9 pick, Space toggles, ↵ picks the highlighted
//! answer and moves on (submitting on the last question), ←/→ and ⌘[ change question, ⌘↵ goes
//! on or submits, Esc dismisses. A printable key on "Type your own answer" starts typing it.
//! In the field, keys type (digits too), Esc leaves it, and ↑ on its first line goes up to the
//! answer above. Submitting with a required question unanswered goes to that question.

use std::collections::{HashMap, HashSet};

use opencode_kit::model::{Field, FieldKind, Pending};
use rune::{Anim, Color, El, Key, KeyEvent, SpringPair, TextEdit, TextStyle, Transition, Tx, col, el, row, spacer, text, text_input};

use crate::odometer::odometer;
use crate::palette::Pal;
use crate::{App, Route, knobs};

const TITLE: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0);
const STRONG: TextStyle = TextStyle::new(13.0, 530.0).lh(20.0);
const HINT: TextStyle = TextStyle::new(11.0, 440.0).lh(12.0);
const MARK: TextStyle = TextStyle::new(11.0, 530.0).lh(12.0).features("\"tnum\"");
const BUTTON: TextStyle = TextStyle::new(13.0, 530.0).lh(20.0).tracking(-0.04);
const CUSTOM: &str = "Type your own answer…";
/// The line height of an answer's title, and of the typed answer.
const LINE: f32 = 20.0;

/// Where the dock is for the form it shows: the question, the highlighted answer, and the
/// picked and typed answers in progress by `form/key`.
#[derive(Default)]
pub struct Dock {
    form: String,
    tab: usize,
    focus: usize,
    /// -1 or 1: the way the last question change went, for the arrival slide.
    step: i32,
    pub picks: HashMap<String, Vec<String>>,
    pub typed: HashMap<String, TextEdit>,
    /// `form/key`s whose typed answer counts.
    custom: HashSet<String>,
    /// The typed answer's laid-out width, to tell which wrapped line its caret is on.
    input_w: f32,
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
            let d = &mut self.question;
            (d.form, d.tab, d.focus, d.step) = (f.id.clone(), 0, focus, 0);
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
        let picks = self.question.picks.get(&k);
        list.iter().position(|(v, ..)| picks.is_some_and(|p| p.contains(v))).unwrap_or(0)
    }

    fn editing(&self, f: &Form, tab: usize) -> bool {
        self.question.typed.get(&f.key(tab)).is_some_and(|t| t.focused)
    }

    /// Whether the dock takes letters now: its typed answer is open or highlighted.
    pub fn question_takes_text(&self) -> bool {
        let Some(f) = self.asking() else { return false };
        let (tab, focus) = self.place(&f);
        self.editing(&f, tab) || (focus >= rows(&f.fields[tab]).len() && has_custom(&f.fields[tab]))
    }

    /// Opens the current question's typed answer with `text` in it (the storybook's typing
    /// stories).
    pub fn question_type(&mut self, text: &str) {
        let Some(f) = self.asking() else { return };
        let (tab, _) = self.place(&f);
        self.start_typing(&f, tab);
        let t = self.typed_mut(&f.id, &f.fields[tab].key);
        *t = TextEdit::new(text);
        t.focused = true;
    }

    /// Goes to the last question (the storybook's Custom answer story).
    pub fn question_last(&mut self) {
        let Some(f) = self.asking() else { return };
        let last = f.fields.len() - 1;
        let focus = self.pick_focus(&f, last);
        let d = self.dock(&f);
        d.tab = last;
        d.focus = focus;
    }

    /// Leaves the typed answer's field as Esc does (the storybook's Custom answer story).
    pub fn question_leave_field(&mut self) {
        let Some(f) = self.asking() else { return };
        let (tab, _) = self.place(&f);
        self.stop_typing(&f, tab);
        self.dock(&f).focus = rows(&f.fields[tab]).len();
    }

    fn answered(&self, f: &Form, tab: usize) -> bool {
        let k = f.key(tab);
        self.question.picks.get(&k).is_some_and(|p| !p.is_empty())
            || (self.question.custom_on(&k) && self.question.typed.get(&k).is_some_and(|t| !t.text.trim().is_empty()))
    }

    fn stop_typing(&mut self, f: &Form, tab: usize) {
        if let Some(t) = self.question.typed.get_mut(&f.key(tab)) {
            t.focused = false;
        }
    }

    /// Picks answer `i`: the one answer of a single choice, or one more (or one fewer) of many.
    fn select(&mut self, f: &Form, tab: usize, i: usize) {
        let Some((value, ..)) = rows(&f.fields[tab]).into_iter().nth(i) else { return };
        let k = f.key(tab);
        let picks = self.question.picks.entry(k.clone()).or_default();
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
            self.question.picks.remove(&k);
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
        tx.motion_curve(knobs::morph(self));
    }

    /// Next question, or the reply on the last one.
    fn next(&mut self, f: &Form, tx: &mut Tx) {
        let (tab, _) = self.place(f);
        self.stop_typing(f, tab);
        if !f.last(tab) {
            return self.jump(f, tab + 1, tx);
        }
        // Unanswered questions that may be skipped are left out, as Desktop does; a required
        // one is where the reply goes next instead.
        if self.form_answer(&f.id).is_none() {
            let gap = (0..f.fields.len()).find(|&t| !self.answered(f, t) && (f.fields[t].required || f.fields.len() == 1));
            if let Some(t) = gap {
                self.jump(f, t, tx);
            }
            return;
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

    /// The dock opened or closed: the surface springs to its new face, the timeline pinned
    /// above it rides its top edge, and the composer gets the keyboard back.
    pub fn morph(&mut self, tx: &mut Tx) {
        tx.motion_curve(knobs::morph(self));
        tx.follow_edge();
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
        if self.question.typed.get(&f.key(tab)).is_none_or(|t| t.text.trim().is_empty()) {
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
            // ⌘[ on the first question isn't the dock's: it goes back through the history.
            Intent::Question(-1) if tab == 0 && matches!(ev.key, Key::Command(_)) => return false,
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

    /// Keys in "Type your own answer": Esc leaves the field, ↑ on its first line goes to the
    /// answer above, ↵ moves on, ⌘↵ and ⌘[ as anywhere. Everything else types.
    fn typed_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let Some(f) = self.asking() else { return false };
        let (tab, _) = self.place(&f);
        let n = rows(&f.fields[tab]).len();
        let m = ev.mods;
        match ev.key {
            Key::Escape => {
                tx.instant();
                self.stop_typing(&f, tab);
                self.dock(&f).focus = n;
                true
            }
            Key::Up if n > 0 && !m.shift && !m.alt && !m.cmd && !m.ctrl && self.question.typed.get(&f.key(tab)).is_none_or(|t| on_first_line(t, self.question.input_w)) => {
                tx.instant();
                self.stop_typing(&f, tab);
                self.dock(&f).focus = n - 1;
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

/// Whether the caret is on the typed answer's first line as it wraps at `width`: the text up
/// to the end of the caret's word fits on one line.
fn on_first_line(t: &TextEdit, width: f32) -> bool {
    let caret = t.focus.min(t.text.len());
    let word_end = t.text[caret..].find(char::is_whitespace).map_or(t.text.len(), |i| caret + i);
    let head = &t.text[..word_end];
    if head.contains('\n') {
        return false;
    }
    width <= 0.0 || rune::text::measure(head, &[], TITLE, Some(width)).1 <= LINE * 1.5
}

/// How the composer's face and the question's swap inside the one surface: the face that
/// leaves fades out quickly, the one that arrives rises and fades up with the morph. The
/// morph is shared by opening and closing, so the dock leaves the way it came.
pub fn face_transition(s: &App) -> Transition {
    Transition::asymmetric(Transition::offset(0.0, s.knobs.get(&knobs::FACE_RISE)).combined(Transition::opacity()), Transition::opacity())
        .springs(SpringPair { enter: knobs::spring(s, &knobs::FACE_IN), exit: knobs::spring(s, &knobs::FADE_OUT) })
}

/// A new question's text and answers slide in from the side they come from (`arrive`) over
/// the old ones fading out where they stood.
fn arrival(s: &App, step: i32) -> Transition {
    let slide = s.knobs.get(&knobs::ARRIVE_SLIDE) * step as f32;
    Transition::asymmetric(Transition::offset(slide, 0.0).combined(Transition::opacity()), Transition::opacity())
        .springs(SpringPair { enter: knobs::spring(s, &knobs::ARRIVE), exit: knobs::spring(s, &knobs::FADE_OUT) })
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
    let picks = s.question.picks.get(&k).cloned().unwrap_or_default();
    let total = f.fields.len();

    let title = if total == 1 { "Question".to_owned() } else { format!("{} of {total} questions", tab + 1) };
    let progress = (total > 1).then(|| {
        row().key("q-progress").gap(2.0).children((0..total).map(|i| {
            let color = if i == tab { p.icon_muted } else if s.answered(&f, i) { p.bg_accent } else { p.q.layer4 };
            el().key(format!("q-seg-{i}"))
                .role("button")
                .label(format!("Question {}", i + 1))
                .size(20.0, 20.0)
                .radius(4.0)
                .center()
                .hover(p.hover)
                .hover_motion(Anim::INSTANT, Anim::INSTANT)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| if let Some(f) = s.asking() { s.jump(&f, i, tx) })
                .child(el().key("bar").size(14.0, 2.0).fill(color))
        }))
    });
    let text_of = if field.description.is_empty() { &field.title } else { &field.description };
    let header = col()
        .key("q-header")
        .w_full()
        .px(10.0)
        .gap(4.0)
        .child(row().key("q-top").w_full().min_h(28.0).gap(12.0).center_y().child(odometer(format!("q-title-{}", f.id), title, (tab + 1) as f64, TITLE, p.muted)).child(spacer()).children(progress))
        .child(
            col()
                .key(format!("q-details-{tab}"))
                .w_full()
                .gap(4.0)
                .transition(arrival(s, s.question.step))
                .child(crate::view::readable(p, text(text_of.clone()).key("q-text").font(STRONG).color(p.text).wrap()))
                .children(multi.then(|| text("Pick any").key("q-hint").font(HINT).color(p.faint))),
        );

    let mut answers = col().key(format!("q-options-{tab}")).w_full().gap(2.0).role(if multi { "group" } else { "radiogroup" }).transition(arrival(s, s.question.step));
    for (i, (value, label, desc)) in list.iter().enumerate() {
        let picked = picks.contains(value);
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
                    let Some(f) = s.asking() else { return };
                    tx.instant();
                    let (tab, _) = s.place(&f);
                    s.select(&f, tab, i);
                    s.dock(&f).focus = i;
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
        let edit = s.question.typed.get(&k).cloned().unwrap_or_else(|| TextEdit::new(""));
        let (id, field_key) = (f.id.clone(), field.key.clone());
        // The typed answer is written where an answer's title is, in the same row: no box of
        // its own, wrapping inside the row as it grows.
        let input = text_input(&edit, if editing { TITLE } else { STRONG })
            .key("q-input")
            .label(CUSTOM)
            .grow(1.0)
            .min_w(0.0)
            .color(p.text)
            .placeholder(CUSTOM, if editing { p.faint } else { p.text })
            .caret(p.text, p.selection)
            .on_layout(|s: &mut App, r| {
                if (s.question.input_w - r[2]).abs() > 0.5 {
                    s.question.input_w = r[2];
                }
            })
            .on_edit(move |s: &mut App| s.typed_mut(&id, &field_key))
            .on_input_change(|s: &mut App, _| {
                let Some(f) = s.asking() else { return };
                let (tab, _) = s.place(&f);
                let k = f.key(tab);
                s.question.custom.insert(k.clone());
                if !f.multi(tab) {
                    s.question.picks.remove(&k);
                }
            })
            .on_input_key(|s: &mut App, ev, tx| s.typed_key(ev, tx));
        answers = answers.child(
            row()
                .key("q-custom")
                .w_full()
                .padding(10.0)
                .gap(8.0)
                .items_start()
                .radius(6.0)
                .fill(if i == focus { p.q.selected } else { Color::TRANSPARENT })
                .on_hover_change(move |s: &mut App, inside, tx: &mut Tx| if inside { s.hover_answer(i, tx) })
                .on_press_tx(|s: &mut App, tx: &mut Tx| {
                    let Some(f) = s.asking() else { return };
                    tx.instant();
                    let (tab, _) = s.place(&f);
                    s.start_typing(&f, tab);
                })
                .child(mark(p, multi, on, i).role(if multi { "checkbox" } else { "radio" }).label(CUSTOM).on_press_tx(|s: &mut App, tx: &mut Tx| {
                    let Some(f) = s.asking() else { return };
                    tx.instant();
                    let (tab, _) = s.place(&f);
                    s.toggle_custom(&f, tab);
                }))
                .child(input),
        );
    }

    let body = col().key("question").w_full().padding(8.0).gap(16.0).transition(face_transition(s)).child(header).child(answers);

    let last = f.last(tab);
    let footer = row()
        .key("question-footer")
        .w_full()
        .px(12.0)
        .pt(7.0)
        .pb(8.0)
        .center_y()
        .transition(face_transition(s))
        .child(button(p, "q-dismiss", "Dismiss", Variant::Ghost, None).on_press_tx(|s: &mut App, tx: &mut Tx| {
            if let Some(f) = s.asking() { s.dismiss(&f, tx) }
        }))
        .child(spacer())
        .child(
            row()
                .key("q-actions")
                .gap(8.0)
                .children((tab > 0).then(|| {
                    button(p, "q-back", "Back", Variant::Neutral, None).on_press_tx(|s: &mut App, tx: &mut Tx| {
                        let Some(f) = s.asking() else { return };
                        let (tab, _) = s.place(&f);
                        s.jump(&f, tab.saturating_sub(1), tx);
                    })
                }))
                .child(
                    button(p, "q-next", if last { "Submit" } else { "Next" }, if last { Variant::Submit } else { Variant::Neutral }, Some("⌘⏎"))
                        .on_press_tx(|s: &mut App, tx: &mut Tx| if let Some(f) = s.asking() { s.next(&f, tx) }),
                ),
        );
    Some(vec![body, footer])
}

rune_register::register!(knobs::GROUP_LIST: rune_knobs::Group = [knobs::QUESTION_DOCK]);

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("question", |s| {
        crate::open_scene(s);
        s.finish_stream();
        let q = s.fixture["question"].clone();
        s.apply_event("form.created", &q);
    }),
    // Answered elsewhere at 1.5 s, asked again at 2.5 s: the morph both ways.
    ("question-morph", |s| {
        crate::open_scene(s);
        s.finish_stream();
        let q = s.fixture["question"].clone();
        s.apply_event("form.created", &q);
        let replied = serde_json::json!({ "id": q["id"], "sessionID": q["sessionID"] });
        s.playback = [(1.5, "form.replied".into(), replied), (2.5, "form.created".into(), q)].into();
    }),
]);

#[cfg(test)]
mod custom_tests;
#[cfg(test)]
mod tests;
