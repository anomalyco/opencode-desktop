//! Naming a section in place: a new section opens its name field with a suggested name selected
//! (typing replaces it) and a few other names to pick under it; a rename opens it with the name
//! it has. ⏎ or a click elsewhere keeps what's typed, Esc keeps the name it had, an empty field
//! keeps it too.
//!
//! The names are guessed here, at once, from the tabs' titles and projects: no server asks.

use rune::TextEdit;

/// The name being edited: arming until its field takes focus, then editing.
#[derive(Default)]
pub enum Naming {
    #[default]
    Idle,
    Arming { section: String, field: TextEdit, alternates: Vec<String>, before: String },
    Editing { section: String, field: TextEdit, alternates: Vec<String>, before: String },
}

pub enum Event<'a> {
    Start { section: &'a str, name: &'a str, alternates: Vec<String> },
    /// The field's focus, read each frame while naming.
    Focused(bool),
    /// ⏎.
    Commit,
    /// Esc.
    Cancel,
    /// One of the other names, picked.
    Pick(&'a str),
    /// Section `id` went while it was named.
    Gone(&'a str),
}

#[derive(Debug, PartialEq)]
pub enum Effect {
    None,
    Focus,
    Save { section: String, name: String },
}

impl Naming {
    pub fn step(&mut self, event: Event<'_>) -> Effect {
        match (std::mem::take(self), event) {
            (_, Event::Start { section, name, alternates }) => {
                let mut field = TextEdit::new(name);
                field.set_selection(0, field.text.len());
                *self = Naming::Arming { section: section.to_owned(), field, alternates, before: name.to_owned() };
                Effect::Focus
            }
            (Naming::Arming { section, field, alternates, before } | Naming::Editing { section, field, alternates, before }, Event::Focused(true)) => {
                *self = Naming::Editing { section, field, alternates, before };
                Effect::None
            }
            (Naming::Arming { section, field, alternates, before }, Event::Focused(false)) => {
                *self = Naming::Arming { section, field, alternates, before };
                Effect::Focus
            }
            (Naming::Editing { section, field, before, .. }, Event::Focused(false))
            | (Naming::Arming { section, field, before, .. } | Naming::Editing { section, field, before, .. }, Event::Commit) => {
                let typed = field.text.trim();
                Effect::Save { section, name: if typed.is_empty() { before } else { typed.to_owned() } }
            }
            (Naming::Arming { section, before, .. } | Naming::Editing { section, before, .. }, Event::Cancel) => Effect::Save { section, name: before },
            (Naming::Arming { section, .. } | Naming::Editing { section, .. }, Event::Pick(name)) => Effect::Save { section, name: name.to_owned() },
            (Naming::Arming { section, .. } | Naming::Editing { section, .. }, Event::Gone(id)) if section == id => Effect::None,
            (state, _) => {
                *self = state;
                Effect::None
            }
        }
    }

    /// The section being named, its field and the other names offered.
    pub fn editing(&self) -> Option<(&str, &TextEdit, &[String])> {
        match self {
            Naming::Arming { section, field, alternates, .. } | Naming::Editing { section, field, alternates, .. } => Some((section, field, alternates)),
            Naming::Idle => None,
        }
    }

    pub fn field_mut(&mut self) -> Option<&mut TextEdit> {
        match self {
            Naming::Arming { field, .. } | Naming::Editing { field, .. } => Some(field),
            Naming::Idle => None,
        }
    }

    /// Whether the field has focus, while naming.
    pub fn focused(&self) -> Option<bool> {
        self.editing().map(|(_, field, _)| field.focused)
    }
}

/// Words that name nothing on their own.
const STOP: &[&str] = &["a", "an", "and", "the", "of", "or", "to", "in", "on", "for", "with", "fix", "add", "new", "make", "update", "wip", "session"];

/// A title's words broken at spaces, marks and letter-digit seams, each as typed and as compared
/// (lowercase; a lone roman numeral as its number): "PHASE II: LEAD" and "Phase2 runner" both
/// start "phase", "2".
fn tokens(title: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for word in title.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()) {
        let mut part = String::new();
        let mut digit = None;
        for c in word.chars() {
            let d = c.is_ascii_digit();
            if digit.is_some_and(|was| was != d) && !part.is_empty() {
                out.push(std::mem::take(&mut part));
            }
            digit = Some(d);
            part.push(c);
        }
        if !part.is_empty() {
            out.push(part);
        }
    }
    out.into_iter().map(|raw| (compare(&raw), raw)).collect()
}

fn compare(raw: &str) -> String {
    let lower = raw.to_lowercase();
    let roman = ["ii", "iii", "iv", "v", "vi", "vii", "viii", "ix", "x"];
    match roman.iter().position(|r| *r == lower) {
        // "v" and "x" alone are as often letters: only the longer numerals count.
        Some(i) if lower.len() > 1 => (i + 2).to_string(),
        _ => lower,
    }
}

/// A word as a section's name shows it: "INFERENCE" reads "Inference", "lumen" reads "Lumen".
fn show(raw: &str) -> String {
    let shouting = raw.chars().filter(|c| c.is_alphabetic()).count() > 1 && !raw.chars().any(char::is_lowercase);
    let word = if shouting { raw.to_lowercase() } else { raw.to_owned() };
    let mut c = word.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

fn stop(t: &str) -> bool {
    STOP.contains(&t)
}

/// Names for a section of tabs titled `titles` in `projects`: the best first, then up to three
/// others, or `fallback` alone. The leading words every title shares, the project they share,
/// a word they all have, the project with that word.
pub fn suggest(titles: &[&str], projects: &[Option<&str>], fallback: &str) -> Vec<String> {
    let toks: Vec<Vec<(String, String)>> = titles.iter().map(|t| tokens(t)).collect();
    let mut names: Vec<String> = Vec::new();
    let push = |n: String, names: &mut Vec<String>| {
        if !n.trim().is_empty() && !names.iter().any(|x| x.eq_ignore_ascii_case(&n)) {
            names.push(n);
        }
    };
    // The leading words all titles share, up to three ("Inference", "Phase 2").
    let first = toks.first().cloned().unwrap_or_default();
    let shared = first.iter().take(3).enumerate().take_while(|(i, (t, _))| toks.iter().all(|w| w.get(*i).is_some_and(|(x, _)| x == t))).count();
    let lead: Vec<&(String, String)> = first.iter().take(shared).collect();
    let lead_name = (!lead.is_empty() && !lead.iter().all(|(t, _)| stop(t)) && toks.len() > 1).then(|| lead.iter().map(|(_, r)| show(r)).collect::<Vec<_>>().join(" "));
    if let Some(n) = &lead_name {
        push(n.clone(), &mut names);
        // As one title writes it ("Phase2"), where a title runs the shared words together.
        for t in titles {
            if let Some(word) = t.split(|c: char| !c.is_alphanumeric()).find(|w| !w.is_empty())
                && tokens(word).iter().map(|(c, _)| c.as_str()).eq(lead.iter().map(|(c, _)| c.as_str()))
            {
                push(show(word), &mut names);
            }
        }
        // Its first word alone ("Inference" for "Inference stats").
        if lead.len() > 1 {
            push(show(&lead[0].1), &mut names);
        }
    }
    // The project they share.
    let project = projects.first().copied().flatten().filter(|p| !p.is_empty() && projects.iter().all(|x| x.is_some_and(|x| x.eq_ignore_ascii_case(p))));
    if let Some(p) = project {
        push(show(p), &mut names);
    }
    // A word every title has, and then one most of them have.
    let word = |min: usize| {
        first.iter().chain(toks.iter().flatten()).find(|(t, _)| {
            !stop(t) && t.chars().any(char::is_alphabetic) && toks.iter().filter(|w| w.iter().any(|(x, _)| x == t)).count() >= min && !lead.iter().any(|(l, _)| l == t)
        })
    };
    let common = if toks.len() > 1 { word(toks.len()).or_else(|| word(2.max(toks.len() / 2 + 1))) } else { None };
    if let Some((_, raw)) = common {
        push(show(raw), &mut names);
    }
    // The project with that word ("Lumen routing").
    if let (Some(p), Some(key)) = (project, lead_name.clone().or_else(|| common.map(|(_, r)| show(r))))
        && !key.eq_ignore_ascii_case(p)
    {
        push(format!("{} {}", show(p), key.to_lowercase()), &mut names);
    }
    // A section of one: its title's first words.
    if toks.len() == 1 {
        let words: Vec<String> = first.iter().filter(|(t, _)| !stop(t)).map(|(_, r)| show(r)).collect();
        if let Some(w) = words.first() {
            push(w.clone(), &mut names);
        }
        if words.len() > 1 {
            push(words[..2].join(" "), &mut names);
        }
    }
    names.truncate(4);
    if names.is_empty() {
        names.push(fallback.to_owned());
    }
    names
}
