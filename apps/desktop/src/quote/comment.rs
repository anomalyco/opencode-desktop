//! The composer's quote-comment editor as Desktop runs it: which quote's editor is open and its
//! send (`transitionQuoteComment`, `packages/app/src/composer/quote-comment-machine.ts`), and the
//! compact comment field's own keys, held modifier and `@` file list
//! (`transitionLineCommentEditor`, `packages/ui/src/data-display/line-comment/machine.ts`).

use rune::TextEdit;
use rune::machine::{Attempt, Effects, Seq};

/// Where the editor stands when it replaced the Quote toolbar: the quoted text's line boxes,
/// relative to the first text they cover (`ComposerQuoteAnchor`), so it follows the timeline.
#[derive(Clone, Debug, PartialEq)]
pub struct Anchor {
    pub session: String,
    /// The first selected text's key path.
    pub path: String,
    /// The quoted lines' boxes, each `[x, y, w, h]` from `path`'s top left.
    pub rects: Vec<[f32; 4]>,
    pub short: bool,
    pub backward: bool,
}

/// What an open editor holds: its quote, the draft (the saved comment until edited), where it
/// came from, and the field's own state.
#[derive(Clone, Debug)]
pub struct Editing {
    pub key: u64,
    pub draft: TextEdit,
    pub anchor: Option<Anchor>,
    pub field: Field,
}

/// `QuoteCommentState`.
#[derive(Clone, Debug, Default)]
pub enum Comment {
    #[default]
    Closed,
    Open { key: u64, draft: TextEdit, anchor: Option<Anchor>, field: Field },
    /// A direct send of the draft is in flight: the field is read-only and chip toggles do
    /// nothing until the composer's submit settles.
    Sending { key: u64, attempt: Attempt, draft: TextEdit, anchor: Option<Anchor>, field: Field },
    /// The send didn't go: the draft stays, with an alert until the next edit.
    Failed { key: u64, draft: TextEdit, anchor: Option<Anchor>, field: Field },
}

pub enum Event {
    /// A quote's editor opens: from its chip (no anchor), or in place of the toolbar.
    Opened { key: u64, saved: String, anchor: Option<Anchor> },
    Closed,
    /// The draft changed (`draft.changed`).
    Edited,
    /// The draft goes out with the prompt (`send.requested`).
    SendRequested,
    SendSettled { attempt: Attempt, sent: bool },
    /// The quotes still in the composer (`retained`).
    Retained(Vec<u64>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Send the prompt with this quote's comment (`send.run`).
    Send { attempt: Attempt, key: u64, comment: String },
}

impl Comment {
    pub fn key(&self) -> Option<u64> {
        match self {
            Comment::Closed => None,
            Comment::Open { key, .. } | Comment::Sending { key, .. } | Comment::Failed { key, .. } => Some(*key),
        }
    }

    pub fn draft(&self) -> Option<&TextEdit> {
        match self {
            Comment::Closed => None,
            Comment::Open { draft, .. } | Comment::Sending { draft, .. } | Comment::Failed { draft, .. } => Some(draft),
        }
    }

    pub fn draft_mut(&mut self) -> Option<&mut TextEdit> {
        match self {
            Comment::Closed => None,
            Comment::Open { draft, .. } | Comment::Sending { draft, .. } | Comment::Failed { draft, .. } => Some(draft),
        }
    }

    pub fn anchor(&self) -> Option<&Anchor> {
        match self {
            Comment::Closed => None,
            Comment::Open { anchor, .. } | Comment::Sending { anchor, .. } | Comment::Failed { anchor, .. } => anchor.as_ref(),
        }
    }

    pub fn field(&self) -> Option<&Field> {
        match self {
            Comment::Closed => None,
            Comment::Open { field, .. } | Comment::Sending { field, .. } | Comment::Failed { field, .. } => Some(field),
        }
    }

    pub fn field_mut(&mut self) -> Option<&mut Field> {
        match self {
            Comment::Closed => None,
            Comment::Open { field, .. } | Comment::Sending { field, .. } | Comment::Failed { field, .. } => Some(field),
        }
    }

    pub fn sending(&self) -> bool {
        matches!(self, Comment::Sending { .. })
    }

    pub fn failed(&self) -> bool {
        matches!(self, Comment::Failed { .. })
    }

    fn take(&mut self) -> Option<Editing> {
        match std::mem::take(self) {
            Comment::Closed => None,
            Comment::Open { key, draft, anchor, field } | Comment::Sending { key, draft, anchor, field, .. } | Comment::Failed { key, draft, anchor, field } => {
                Some(Editing { key, draft, anchor, field })
            }
        }
    }
}

pub fn step(seq: &mut Seq, state: &mut Comment, event: Event) -> Effects<Effect> {
    if let Event::Retained(keys) = &event {
        if state.key().is_some_and(|k| !keys.contains(&k)) {
            *state = Comment::Closed;
        }
        return Effects::none();
    }
    if let Comment::Sending { attempt, .. } = state {
        let Event::SendSettled { attempt: a, sent } = event else { return Effects::none() };
        if a != *attempt {
            return Effects::none();
        }
        let e = state.take().expect("sending");
        *state = if sent { Comment::Closed } else { Comment::Failed { key: e.key, draft: e.draft, anchor: e.anchor, field: e.field } };
        return Effects::none();
    }
    match event {
        Event::Opened { key, saved, anchor } => {
            if state.key() == Some(key) {
                if let Comment::Open { anchor: a, .. } | Comment::Failed { anchor: a, .. } = state {
                    *a = anchor;
                }
                return Effects::none();
            }
            let mut draft = TextEdit::new(&saved);
            draft.focused = true;
            *state = Comment::Open { key, draft, anchor, field: Field::default() };
            Effects::none()
        }
        Event::Closed => {
            *state = Comment::Closed;
            Effects::none()
        }
        Event::Edited => {
            if let Some(e) = state.failed().then(|| state.take()).flatten() {
                *state = Comment::Open { key: e.key, draft: e.draft, anchor: e.anchor, field: e.field };
            }
            Effects::none()
        }
        Event::SendRequested => {
            let Some(e) = state.take() else { return Effects::none() };
            let attempt = seq.next();
            let comment = e.draft.text.trim().to_owned();
            *state = Comment::Sending { key: e.key, attempt, draft: e.draft, anchor: e.anchor, field: e.field };
            Effects::one(Effect::Send { attempt, key: state.key().unwrap_or_default(), comment })
        }
        Event::SendSettled { .. } | Event::Retained(_) => Effects::none(),
    }
}

/// `MENTION_LIMIT`.
pub const MENTION_LIMIT: usize = 10;
/// `MENTION_TIMEOUT_MS`.
pub const MENTION_TIMEOUT: f64 = 5.0;

/// The `@query` the caret sits at the end of (`MentionToken`), in bytes of the draft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub query: String,
    pub start: usize,
    pub end: usize,
}

/// `parseMentionToken`: the run after the last `@` before a collapsed caret, when it isn't blank.
pub fn token(draft: &TextEdit) -> Option<Token> {
    let (a, b) = draft.range();
    if a != b {
        return None;
    }
    let caret = b.min(draft.text.len());
    let before = &draft.text[..caret];
    let at = before.rfind('@')?;
    let query = &before[at + 1..];
    if query.chars().any(char::is_whitespace) || query.trim().is_empty() {
        return None;
    }
    Some(Token { query: query.to_owned(), start: at, end: caret })
}

/// `LineCommentEditorState`, less `submitting`, which [`Comment::Sending`] is.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Field {
    /// Mod is held over an editor with an alternate send.
    pub alternate: bool,
    pub mention: Mention,
    /// Counts the `@` searches; only the one a `Loading` started may settle it.
    pub attempt: Attempt,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum Mention {
    #[default]
    Idle,
    /// A token Escape closed, or that found nothing, stays closed while the caret stays on it.
    Dismissed { start: usize, query: String },
    /// A search runs, until `due`; the last results stay listed while the query refines.
    Loading { token: Token, items: Vec<String>, active: usize, due: f64 },
    /// Never empty, at most [`MENTION_LIMIT`].
    Open { token: Token, items: Vec<String>, active: usize },
}

impl Mention {
    pub fn items(&self) -> &[String] {
        match self {
            Mention::Loading { items, .. } | Mention::Open { items, .. } => items,
            _ => &[],
        }
    }

    pub fn active(&self) -> Option<&str> {
        match self {
            Mention::Loading { items, active, .. } | Mention::Open { items, active, .. } => items.get(*active).map(String::as_str),
            _ => None,
        }
    }
}

/// What the field's props say when an event happens (`LineCommentEditorConfig`).
#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// An alternate send is offered, and whether it's off.
    pub alternate: Option<bool>,
    pub submitting: bool,
}

impl Config {
    /// `canSubmit` (the editor allows empty comments).
    fn can_submit(self) -> bool {
        true
    }
}

pub enum FieldEvent {
    /// The draft or caret changed (`mention.synced`), with the token now at the caret.
    Synced { token: Option<Token>, now: f64 },
    Loaded { attempt: Attempt, items: Vec<String> },
    /// The search failed, or ran out of time.
    Failed { attempt: Attempt },
    Hovered(usize),
    Picked(usize),
    /// Mod went down or up (`modifier.changed`), or the window lost focus.
    Modifier { held: bool, config: Config },
    Key { key: FieldKey, config: Config },
    /// The action button's click, with Mod held or not.
    Clicked { modified: bool, config: Config },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKey {
    Escape,
    /// Return, with Mod (`(meta || ctrl) && !alt && !shift`), and held down.
    Enter { modified: bool, repeat: bool },
    Tab,
    Step(i32),
    Other,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FieldEffect {
    Search { attempt: Attempt, query: String, due: f64 },
    Insert { path: String, token: Token },
    Submit { alternate: bool },
    Cancel,
}

/// `transitionLineCommentEditor`; whether the field took the key.
pub fn field_step(seq: &mut Seq, f: &mut Field, event: FieldEvent) -> (Effects<FieldEffect>, bool) {
    match event {
        FieldEvent::Modifier { held, config } => {
            f.alternate = held && config.alternate.is_some();
            (Effects::none(), false)
        }
        FieldEvent::Synced { token, now } => (sync(seq, f, token, now), false),
        FieldEvent::Loaded { attempt, items } => {
            let Mention::Loading { token, .. } = &f.mention else { return (Effects::none(), false) };
            if attempt != f.attempt {
                return (Effects::none(), false);
            }
            let items: Vec<String> = items.into_iter().take(MENTION_LIMIT).collect();
            f.mention = if items.is_empty() { Mention::Dismissed { start: token.start, query: token.query.clone() } } else { Mention::Open { token: token.clone(), items, active: 0 } };
            (Effects::none(), false)
        }
        FieldEvent::Failed { attempt } => {
            if matches!(f.mention, Mention::Loading { .. }) && attempt == f.attempt {
                f.mention = Mention::Idle;
            }
            (Effects::none(), false)
        }
        FieldEvent::Hovered(i) => {
            if let Mention::Loading { items, active, .. } | Mention::Open { items, active, .. } = &mut f.mention
                && i < items.len()
            {
                *active = i;
            }
            (Effects::none(), false)
        }
        FieldEvent::Picked(i) => (insert(f, i), true),
        FieldEvent::Clicked { modified, config } => {
            if config.submitting {
                return (Effects::none(), false);
            }
            let alternate = modified && config.alternate.is_some();
            if !alternate && !config.can_submit() {
                return (cancel(f), true);
            }
            (submit(f, config, alternate), true)
        }
        FieldEvent::Key { key, config } => {
            if config.submitting {
                return (Effects::none(), matches!(key, FieldKey::Escape | FieldKey::Enter { .. }));
            }
            key_step(f, key, config)
        }
    }
}

fn key_step(f: &mut Field, key: FieldKey, config: Config) -> (Effects<FieldEffect>, bool) {
    let listed = !f.mention.items().is_empty();
    let open = matches!(f.mention, Mention::Open { .. });
    let active = match &f.mention {
        Mention::Loading { active, .. } | Mention::Open { active, .. } => *active,
        _ => 0,
    };
    match key {
        FieldKey::Escape if listed => {
            dismiss(f);
            (Effects::none(), true)
        }
        FieldKey::Escape => (cancel(f), true),
        FieldKey::Enter { modified: true, repeat } if config.alternate.is_some() => {
            if repeat { (Effects::none(), true) } else { (submit(f, config, true), true) }
        }
        FieldKey::Enter { .. } if open => (insert(f, active), true),
        FieldKey::Enter { .. } if listed => (Effects::none(), true),
        FieldKey::Enter { repeat: true, .. } => (Effects::none(), true),
        FieldKey::Enter { .. } => (submit(f, config, false), true),
        FieldKey::Tab if open => (insert(f, active), true),
        FieldKey::Tab if listed => (Effects::none(), true),
        FieldKey::Step(d) if listed => {
            if let Mention::Loading { items, active, .. } | Mention::Open { items, active, .. } = &mut f.mention {
                let n = items.len() as i32;
                *active = ((*active as i32 + d + n) % n) as usize;
            }
            (Effects::none(), true)
        }
        _ => (Effects::none(), false),
    }
}

fn sync(seq: &mut Seq, f: &mut Field, token: Option<Token>, now: f64) -> Effects<FieldEffect> {
    let Some(token) = token else {
        f.mention = Mention::Idle;
        return Effects::none();
    };
    match &f.mention {
        Mention::Dismissed { start, query } if *start == token.start && *query == token.query => return Effects::none(),
        Mention::Loading { token: t, .. } | Mention::Open { token: t, .. } if *t == token => return Effects::none(),
        _ => {}
    }
    let items = match &f.mention {
        Mention::Loading { token: t, items, .. } | Mention::Open { token: t, items, .. } if t.start == token.start => items.clone(),
        _ => Vec::new(),
    };
    f.attempt = seq.next();
    let due = now + MENTION_TIMEOUT;
    let query = token.query.clone();
    f.mention = Mention::Loading { token, items, active: 0, due };
    Effects::one(FieldEffect::Search { attempt: f.attempt, query, due })
}

fn dismiss(f: &mut Field) {
    if let Mention::Loading { token, .. } | Mention::Open { token, .. } = &f.mention {
        f.mention = Mention::Dismissed { start: token.start, query: token.query.clone() };
    }
}

fn submit(f: &mut Field, config: Config, alternate: bool) -> Effects<FieldEffect> {
    if !config.can_submit() || (alternate && config.alternate == Some(true)) {
        return Effects::none();
    }
    f.mention = Mention::Idle;
    Effects::one(FieldEffect::Submit { alternate })
}

fn cancel(f: &mut Field) -> Effects<FieldEffect> {
    f.mention = Mention::Idle;
    Effects::one(FieldEffect::Cancel)
}

fn insert(f: &mut Field, i: usize) -> Effects<FieldEffect> {
    let (Mention::Loading { token, items, .. } | Mention::Open { token, items, .. }) = &f.mention else { return Effects::none() };
    let Some(path) = items.get(i).cloned() else { return Effects::none() };
    let token = token.clone();
    f.mention = Mention::Idle;
    Effects::one(FieldEffect::Insert { path, token })
}
