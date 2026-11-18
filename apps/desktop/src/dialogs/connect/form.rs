//! A sign-in method's form, asked one visible field at a time like the TUI, so a field's `when`
//! sees every earlier answer (Desktop's `providers/connect/form-machine.ts`). Browser opens take
//! attempt numbers from `issued`, so a result for an earlier open or step is stale.

use opencode_kit::integration::{Field, FieldKind};
use serde_json::{Map, Value};

pub type Answer = Map<String, Value>;

#[derive(Clone, Debug, PartialEq)]
pub enum Browser {
    Unopened,
    /// `unlocked` keeps "I finished" enabled while the link opens again after an earlier open.
    Opening { attempt: u32, unlocked: bool },
    Opened,
    /// The browser didn't open, so the link shows for the user to open by hand.
    Failed,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Choice {
    None { draft: String },
    Preset { value: String, draft: String },
    Custom { text: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldError {
    Required,
    Number,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Issue {
    SelectOne,
    MinItems,
    MaxItems,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    External { url: String, browser: Browser },
    Multiselect { picked: Vec<String>, custom: String },
    Choice { choice: Choice, error: Option<FieldError> },
    Text { text: String, error: Option<FieldError> },
}

/// The one field the form asks now.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub index: usize,
    pub field: Field,
    pub kind: Kind,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Form {
    pub fields: Vec<Field>,
    pub answer: Answer,
    pub issued: u32,
    pub step: Step,
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Active(Form),
    Completed(Answer),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    TextChanged(String),
    ChoiceSelected(String),
    CustomChanged(String),
    CustomAdded,
    OptionToggled(String, bool),
    ExternalOpen,
    Submitted,
    Skipped,
    /// `opened` is false when the browser failed to open or didn't answer in time.
    ExternalOpened { attempt: u32, opened: bool },
}

/// Opening the external step's link.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenBrowser {
    pub attempt: u32,
    pub url: String,
}

/// The form's first visible field, or its answer when every field is hidden or doesn't apply.
pub fn create(fields: &[Field]) -> State {
    advance(fields, 0, Answer::new(), 0)
}

/// The next state (none when the input changed nothing) and the browser open it asks for.
pub fn transition(form: &Form, input: Input) -> (Option<State>, Option<OpenBrowser>) {
    let step = &form.step;
    let set = |kind: Kind| Some(State::Active(Form { step: Step { kind, ..step.clone() }, ..form.clone() }));
    match (&step.kind, input) {
        (Kind::External { url, browser }, Input::ExternalOpen) => {
            if matches!(browser, Browser::Opening { .. }) {
                return (None, None);
            }
            let attempt = form.issued + 1;
            let unlocked = matches!(browser, Browser::Opened | Browser::Failed);
            let kind = Kind::External { url: url.clone(), browser: Browser::Opening { attempt, unlocked } };
            let next = Form { issued: attempt, step: Step { kind, ..step.clone() }, ..form.clone() };
            (Some(State::Active(next)), Some(OpenBrowser { attempt, url: url.clone() }))
        }
        (Kind::External { url, browser }, Input::ExternalOpened { attempt, opened }) => match browser {
            Browser::Opening { attempt: a, .. } if *a == attempt => {
                (set(Kind::External { url: url.clone(), browser: if opened { Browser::Opened } else { Browser::Failed } }), None)
            }
            _ => (None, None),
        },
        (Kind::External { browser, .. }, Input::Submitted) if can_finish(browser) => (Some(next(form, Some(Value::Bool(true)))), None),
        (Kind::External { .. }, _) => (None, None),
        (Kind::Multiselect { picked, custom }, Input::OptionToggled(value, checked)) => {
            if checked == picked.contains(&value) {
                return (None, None);
            }
            let picked = if checked { [picked.clone(), vec![value]].concat() } else { picked.iter().filter(|p| **p != value).cloned().collect() };
            (set(Kind::Multiselect { picked, custom: custom.clone() }), None)
        }
        (Kind::Multiselect { picked, custom }, Input::CustomChanged(value)) if step.field.custom && value != *custom => {
            (set(Kind::Multiselect { picked: picked.clone(), custom: value }), None)
        }
        (Kind::Multiselect { picked, custom }, Input::CustomAdded) => {
            let value = custom.trim().to_owned();
            if !step.field.custom || value.is_empty() || picked.contains(&value) {
                return (None, None);
            }
            (set(Kind::Multiselect { picked: [picked.clone(), vec![value]].concat(), custom: String::new() }), None)
        }
        (Kind::Multiselect { picked, .. }, Input::Submitted) if multiselect_issue(&step.field, picked).is_none() => {
            let value = (!picked.is_empty()).then(|| Value::Array(picked.iter().cloned().map(Value::String).collect()));
            (Some(next(form, value)), None)
        }
        (Kind::Multiselect { .. }, _) => (None, None),
        (Kind::Choice { choice, .. }, Input::ChoiceSelected(value)) => match select(&step.field, choice, &value) {
            Some(choice) => (set(Kind::Choice { choice, error: None }), None),
            None => (None, None),
        },
        (Kind::Choice { choice: Choice::Custom { text }, .. }, Input::CustomChanged(value)) if value != *text => {
            (set(Kind::Choice { choice: Choice::Custom { text: value }, error: None }), None)
        }
        (Kind::Choice { .. }, Input::Skipped) if !step.field.required => (Some(next(form, None)), None),
        (Kind::Choice { choice, .. }, Input::Submitted) => {
            let value = choice_value(&step.field, choice);
            if value.is_none() && step.field.required {
                return (set(Kind::Choice { choice: choice.clone(), error: Some(FieldError::Required) }), None);
            }
            (Some(next(form, value)), None)
        }
        (Kind::Choice { .. }, _) => (None, None),
        (Kind::Text { text, error }, Input::TextChanged(value)) => {
            if value == *text && error.is_none() {
                return (None, None);
            }
            (set(Kind::Text { text: value, error: None }), None)
        }
        (Kind::Text { text, .. }, Input::Submitted) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                if step.field.required {
                    return (set(Kind::Text { text: text.clone(), error: Some(FieldError::Required) }), None);
                }
                return (Some(next(form, None)), None);
            }
            match step.field.kind {
                FieldKind::String => (Some(next(form, Some(Value::String(trimmed.to_owned())))), None),
                _ => match trimmed.parse::<f64>().ok().filter(|n| n.is_finite() && (step.field.kind != FieldKind::Integer || n.fract() == 0.0)) {
                    Some(n) => (Some(next(form, serde_json::Number::from_f64(n).map(Value::Number))), None),
                    None => (set(Kind::Text { text: text.clone(), error: Some(FieldError::Number) }), None),
                },
            }
        }
        (Kind::Text { .. }, _) => (None, None),
    }
}

impl Form {
    /// Each step's own key, so a new step mounts fresh inputs.
    pub fn key(&self) -> String {
        format!("{}:{}", self.step.index, self.step.field.key)
    }

    pub fn title(&self) -> String {
        let f = &self.step.field;
        f.title.clone().unwrap_or_else(|| match &f.kind {
            FieldKind::External { url } => url.clone(),
            _ => f.key.clone(),
        })
    }

    /// The external step can be finished: its link opened, or failed to open.
    pub fn can_finish(&self) -> bool {
        matches!(&self.step.kind, Kind::External { browser, .. } if can_finish(browser))
    }

    pub fn can_continue(&self) -> bool {
        match &self.step.kind {
            Kind::Choice { choice, .. } => choice_value(&self.step.field, choice).is_some(),
            Kind::Multiselect { picked, .. } => multiselect_issue(&self.step.field, picked).is_none(),
            _ => true,
        }
    }

    pub fn issue(&self) -> Option<Issue> {
        match &self.step.kind {
            Kind::Multiselect { picked, .. } => multiselect_issue(&self.step.field, picked),
            _ => None,
        }
    }

    /// The radio value: a preset, "custom" while typing an own answer, or none.
    pub fn selected(&self) -> Option<String> {
        match &self.step.kind {
            Kind::Choice { choice: Choice::Preset { value, .. }, .. } => Some(value.clone()),
            Kind::Choice { choice: Choice::Custom { .. }, .. } => Some("custom".into()),
            _ => None,
        }
    }

    pub fn error(&self) -> Option<FieldError> {
        match &self.step.kind {
            Kind::Choice { error, .. } | Kind::Text { error, .. } => *error,
            _ => None,
        }
    }

    /// The options a choice or multiselect step lists: a boolean's Yes and No (No first when it
    /// defaults to no), else the field's own.
    pub fn options(&self) -> Vec<(String, String, Option<String>)> {
        let f = &self.step.field;
        if f.kind == FieldKind::Boolean {
            let rows = vec![("true".to_owned(), opencode_i18n::t!("provider.connect.form.yes").to_owned(), None), ("false".to_owned(), opencode_i18n::t!("provider.connect.form.no").to_owned(), None)];
            return if f.default == Some(Value::Bool(false)) { rows.into_iter().rev().collect() } else { rows };
        }
        f.options.iter().map(|o| (o.value.clone(), o.label.clone(), o.description.clone())).collect()
    }
}

fn can_finish(browser: &Browser) -> bool {
    match browser {
        Browser::Unopened => false,
        Browser::Opening { unlocked, .. } => *unlocked,
        _ => true,
    }
}

/// Whether a field applies given the answers so far; the TUI's rule.
fn active(field: &Field, answer: &Answer) -> bool {
    if matches!(field.kind, FieldKind::External { .. }) {
        return true;
    }
    field.when.iter().all(|w| {
        let Some(value) = answer.get(&w.key) else { return false };
        let hit = match value {
            Value::Array(items) => items.iter().any(|i| i.as_str() == Some(&string_of(&w.value))),
            v => *v == w.value,
        };
        if w.equal { hit } else { !hit }
    })
}

fn string_of(v: &Value) -> String {
    v.as_str().map_or_else(|| v.to_string(), str::to_owned)
}

/// Records the step's answer and moves to the next visible field that applies.
fn next(form: &Form, value: Option<Value>) -> State {
    let mut answer = form.answer.clone();
    if let Some(v) = value {
        answer.insert(form.step.field.key.clone(), v);
    }
    advance(&form.fields, form.step.index + 1, answer, form.issued)
}

/// Hidden fields that apply take their default in order, so a later `when` sees them.
fn advance(fields: &[Field], index: usize, mut answer: Answer, issued: u32) -> State {
    let Some(field) = fields.get(index) else { return State::Completed(answer) };
    if !active(field, &answer) {
        return advance(fields, index + 1, answer, issued);
    }
    if !matches!(field.kind, FieldKind::External { .. }) && field.hidden {
        if let Some(d) = &field.default {
            answer.insert(field.key.clone(), d.clone());
        }
        return advance(fields, index + 1, answer, issued);
    }
    State::Active(Form { fields: fields.to_vec(), answer, issued, step: Step { index, field: field.clone(), kind: initial(field) } })
}

/// Prefills a step from its field's default; the value enters the answer only once confirmed.
fn initial(field: &Field) -> Kind {
    let none = Choice::None { draft: String::new() };
    match &field.kind {
        FieldKind::External { url } => Kind::External { url: url.clone(), browser: Browser::Unopened },
        FieldKind::Multiselect => Kind::Multiselect {
            picked: field.default.as_ref().and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect(),
            custom: String::new(),
        },
        FieldKind::Boolean => Kind::Choice {
            choice: field.default.as_ref().map_or(none, |d| Choice::Preset { value: string_of(d), draft: String::new() }),
            error: None,
        },
        FieldKind::String if !field.options.is_empty() => {
            let choice = match field.default.as_ref().and_then(Value::as_str) {
                None => none,
                Some(v) if field.options.iter().any(|o| o.value == v) => Choice::Preset { value: v.to_owned(), draft: String::new() },
                Some(v) if field.custom => Choice::Custom { text: v.to_owned() },
                Some(_) => none,
            };
            Kind::Choice { choice, error: None }
        }
        _ => Kind::Text { text: field.default.as_ref().map(string_of).unwrap_or_default(), error: None },
    }
}

fn select(field: &Field, choice: &Choice, value: &str) -> Option<Choice> {
    let draft = match choice {
        Choice::Custom { text } => text.clone(),
        Choice::None { draft } | Choice::Preset { draft, .. } => draft.clone(),
    };
    if value == "custom" {
        if field.kind != FieldKind::String || !field.custom || matches!(choice, Choice::Custom { .. }) {
            return None;
        }
        return Some(Choice::Custom { text: draft });
    }
    let known = if field.kind == FieldKind::Boolean { value == "true" || value == "false" } else { field.options.iter().any(|o| o.value == value) };
    if !known || matches!(choice, Choice::Preset { value: v, .. } if v == value) {
        return None;
    }
    Some(Choice::Preset { value: value.to_owned(), draft })
}

fn choice_value(field: &Field, choice: &Choice) -> Option<Value> {
    match choice {
        Choice::None { .. } => None,
        Choice::Custom { text } => Some(text.trim()).filter(|t| !t.is_empty()).map(|t| Value::String(t.to_owned())),
        Choice::Preset { value, .. } if field.kind == FieldKind::Boolean => Some(Value::Bool(value == "true")),
        Choice::Preset { value, .. } => Some(Value::String(value.clone())),
    }
}

fn multiselect_issue(field: &Field, picked: &[String]) -> Option<Issue> {
    let n = picked.len();
    if field.required && n == 0 {
        return Some(Issue::SelectOne);
    }
    if field.min_items.is_some_and(|m| n < m) {
        return Some(Issue::MinItems);
    }
    if field.max_items.is_some_and(|m| n > m) {
        return Some(Issue::MaxItems);
    }
    None
}
