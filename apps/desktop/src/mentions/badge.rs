//! Mentions as badges: one rounded chip per `@` mention, in the composer (Rune's `Mark::Pill`
//! atoms) and in a sent prompt's bubble (static pills over its text). Desktop colours agent and
//! file mentions' text (`syntax-type`, `syntax-property`, `syntax-keyword` for references) and
//! gives session mentions a raised chip; here every kind gets a chip, tinted by its colour.

use std::ops::Range;
use std::sync::OnceLock;

use opencode_kit::mention::{self, MentionKind, UserMention};
use rune::{Color, PillStyle};

use crate::App;
use crate::palette::Pal;
use crate::view::title_of;

/// A coloured kind's chip: its colour at this opacity, lighter on dark where tints read louder.
fn tint(dark: bool) -> f32 {
    if dark { 0.035 } else { 0.16 }
}
/// A plain kind's chip (skills, sessions, references): the text colour at this opacity.
fn neutral(dark: bool) -> f32 {
    if dark { 0.035 } else { 0.09 }
}

/// Colours the shared palette doesn't carry: `syntax-type` for agents, `syntax-property` for
/// files, `icon-info-active`.
pub(super) struct Ink {
    pub agent: Color,
    pub file: Color,
    pub info: Color,
}

pub(super) fn ink(dark: bool) -> &'static Ink {
    static DARK: OnceLock<Ink> = OnceLock::new();
    static LIGHT: OnceLock<Ink> = OnceLock::new();
    let build = || {
        let hex = |s: &str| rune::color::hex(s).unwrap_or(Color::BLACK);
        let theme = rune::Theme::from_json(include_str!("../../assets/theme.json"), dark).unwrap_or_else(|e| panic!("{e}"));
        Ink {
            agent: if dark { hex("#ecf58c") } else { hex("#596600") },
            file: if dark { hex("#ff9ae2") } else { hex("#ed6dc8") },
            info: theme.color("state-fg-info"),
        }
    };
    if dark { DARK.get_or_init(build) } else { LIGHT.get_or_init(build) }
}

/// A mention's badge: a chip and label, no icon, as Desktop draws mentions as text alone.
pub fn style(p: &Pal, kind: MentionKind) -> PillStyle {
    let k = ink(p.dark);
    let tinted = |c: Color| PillStyle::new().bg(c.alpha(tint(p.dark))).fg(c);
    let neutral = |fg: Color| PillStyle::new().bg(p.text.alpha(neutral(p.dark))).fg(fg);
    match kind {
        MentionKind::Agent => tinted(k.agent),
        MentionKind::File => tinted(k.file),
        MentionKind::Directory => neutral(p.muted),
        MentionKind::Skill | MentionKind::Session => neutral(p.text),
    }
}

/// The kind of a composer pill by its id (`agent:`, `skill:`, `file:`, `session:`).
pub fn kind_of(id: &str) -> Option<MentionKind> {
    Some(match id.split_once(':')?.0 {
        "agent" => MentionKind::Agent,
        "skill" => MentionKind::Skill,
        "file" => MentionKind::File,
        "session" => MentionKind::Session,
        _ => return None,
    })
}

/// The composer's pill look by id. Pills that aren't mentions keep Rune's default.
pub fn pill_style(p: &Pal) -> impl Fn(&str) -> PillStyle + 'static {
    let p = *p;
    move |id: &str| kind_of(id).map_or_else(PillStyle::new, |k| style(&p, k))
}

/// The mentions of a prompt sent from the composer, over its trimmed text (what its bubble
/// shows until the server's copy replaces it).
pub fn echo(text: &str, pills: &[(Range<usize>, String)]) -> Vec<UserMention> {
    let lead = text.len() - text.trim_start().len();
    let end = lead + text.trim().len();
    pills
        .iter()
        .filter(|(r, _)| r.start >= lead && r.end <= end && r.start < r.end)
        .filter_map(|(r, id)| Some(UserMention { range: r.start - lead..r.end - lead, kind: kind_of(id)? }))
        .collect()
}

/// A bubble's text and badges: the message's recorded mentions (those still on an `@` in the
/// text, as an echo's may not be once the server's copy lands), and its `@session:<id>`
/// tokens shown as `@<title>` when the session is known. `title` names a session by id.
pub fn bubble(body: &str, mentions: &[UserMention], title: impl Fn(&str) -> Option<String>) -> (String, Vec<(Range<usize>, MentionKind)>) {
    let mut marks: Vec<(Range<usize>, MentionKind, Option<String>)> =
        mentions.iter().filter(|m| body.get(m.range.clone()).is_some_and(|t| t.starts_with('@'))).map(|m| (m.range.clone(), m.kind, None)).collect();
    for (r, id) in mention::session_tokens(body) {
        if !marks.iter().any(|(m, _, _)| m.start < r.end && r.start < m.end) {
            marks.push((r, MentionKind::Session, title(id).map(|t| format!("@{t}"))));
        }
    }
    marks.sort_by_key(|(r, _, _)| r.start);
    let (mut out, mut ranges, mut at) = (String::with_capacity(body.len()), Vec::new(), 0);
    for (r, kind, shown) in marks {
        if r.start < at {
            continue;
        }
        out.push_str(&body[at..r.start]);
        let start = out.len();
        out.push_str(shown.as_deref().unwrap_or(&body[r.clone()]));
        ranges.push((start..out.len(), kind));
        at = r.end;
    }
    out.push_str(&body[at..]);
    (out, ranges)
}

impl App {
    /// A session's title for a mention of it: a loaded session's, or one the `@` search found.
    pub fn mention_title(&self, id: &str) -> Option<String> {
        self.session(id).or_else(|| self.mentions.found_session(id)).map(title_of)
    }

    /// Records the composer's mentions for the prompt about to leave as message `id`.
    pub fn echo_mentions(&self, id: &str) {
        let pills: Vec<(Range<usize>, String)> = self.composer.pills().map(|(r, k)| (r, k.to_owned())).collect();
        mention::set_user_mentions(id, echo(&self.composer.text, &pills));
    }

    /// Message `id`'s bubble text and badges.
    pub fn bubble_badges(&self, p: &Pal, id: &str, body: &str) -> (String, Vec<(Range<usize>, PillStyle)>) {
        let (text, marks) = bubble(body, &opencode_kit::mention::user_mentions(id), |sid| self.mention_title(sid));
        (text, marks.into_iter().map(|(r, k)| (r, style(p, k))).collect())
    }
}

#[cfg(test)]
#[path = "badge_tests.rs"]
mod tests;
