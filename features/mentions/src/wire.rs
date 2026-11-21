//! The pure half of sending pills: the prompt text, its `agents`, `skills` and `files`, and the
//! path encoding file mentions use.

use std::ops::Range;

use opencode_kit::client::{AgentInput, FileInput, Mention, SkillInput};
use opencode_kit::mention::UserMention;
pub use opencode_kit::path::{absolute, encode_path};

/// `parseSessionMention`: an id the server turns back into a session.
pub fn session_id_ok(id: &str) -> bool {
    id.strip_prefix("ses_").is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// `getFilename`: the last segment, ignoring trailing separators.
pub fn filename(path: &str) -> &str {
    opencode_kit::path::basename(path)
}

/// A file's lines sent as context without a mention (`request.ts:187-196`):
/// `file://<path>?start=<a>&end=<b>`.
pub fn context_input(directory: &str, path: &str, start: u32, end: u32) -> FileInput {
    FileInput {
        uri: format!("file://{}?start={start}&end={end}", encode_path(&absolute(directory, path))),
        name: filename(path).to_owned(),
        mime: Some("text/plain".into()),
        ..Default::default()
    }
}

/// What a prompt sends for its pills.
#[derive(Debug, Default, PartialEq)]
pub struct Wire {
    pub text: String,
    /// The text as typed (session pills show titles).
    pub display: String,
    pub agents: Vec<AgentInput>,
    pub skills: Vec<SkillInput>,
    /// File mentions as `file://` references the server reads.
    pub files: Vec<FileInput>,
    /// The badges a local echo of `text` shows until the server's copy lands, as bytes of
    /// `text`. Session mentions aren't here: the bubble finds their `@session:` tokens itself.
    pub marks: Vec<UserMention>,
}

/// `text`'s trimmed span as byte offsets, and the pills inside it (empty ones and ones spanning
/// an edge dropped).
pub fn inside_trimmed<'a, S>(text: &str, pills: &'a [(Range<usize>, S)]) -> (usize, usize, impl Iterator<Item = &'a (Range<usize>, S)>) {
    let lead = text.len() - text.trim_start().len();
    let end = lead + text.trim().len();
    (lead, end, pills.iter().filter(move |(r, _)| r.start >= lead && r.end <= end && r.start < r.end))
}

/// The prompt for `text` (trimmed) and its pills (byte range, id). Session pills become
/// `@session:<id>`, moving the mentions after them; offsets count UTF-16 units. File paths
/// resolve against `directory`.
pub fn wire<S: AsRef<str>>(text: &str, pills: &[(Range<usize>, S)], directory: &str, skill_name: impl Fn(&str) -> String) -> Wire {
    let trimmed = text.trim();
    let (lead, end, pills) = inside_trimmed(text, pills);
    let mut pills: Vec<_> = pills.collect();
    pills.sort_by_key(|(r, _)| r.start);
    let mut out = Wire { display: trimmed.to_owned(), ..Default::default() };
    let utf16 = |s: &str| s.encode_utf16().count();
    let mut at = lead;
    for (range, id) in pills {
        let id = id.as_ref();
        if range.start < at {
            continue;
        }
        out.text.push_str(&text[at..range.start]);
        let shown = &text[range.clone()];
        let (kind, value) = id.split_once(':').unwrap_or(("", id));
        let content = match kind {
            "session" if session_id_ok(value) => format!("@session:{value}"),
            _ => shown.to_owned(),
        };
        let start = utf16(&out.text);
        let at_byte = out.text.len();
        out.text.push_str(&content);
        if kind != "session"
            && let Some(k) = super::badge::kind_of(id)
        {
            out.marks.push(UserMention { range: at_byte..out.text.len(), kind: k });
        }
        let mention = Some(Mention { start, end: start + utf16(&content), text: content.clone() });
        match kind {
            "agent" => out.agents.push(AgentInput { name: value.to_owned(), mention }),
            "skill" => out.skills.push(SkillInput { id: value.to_owned(), name: skill_name(value), mention }),
            "file" => out.files.push(FileInput {
                uri: format!("file://{}", encode_path(&absolute(directory, value))),
                name: filename(value).to_owned(),
                mime: Some("text/plain".into()),
                mention,
                ..Default::default()
            }),
            // A reference is its folder, named as it was picked (`composer/model.ts:190-198`).
            "reference" => out.files.push(FileInput {
                uri: format!("file://{}", encode_path(&absolute(directory, value))),
                name: shown.trim_start_matches('@').to_owned(),
                mime: Some("application/x-directory".into()),
                mention,
                ..Default::default()
            }),
            // Session mentions are already encoded inline as `@session:<id>` in `out.text` above.
            "session" => {}
            // Official decodes a closed `ContentPart` schema union and leaves non-matching parts
            // as plain text in `text` (`packages/app/src/composer/request.ts:121-150`); log any
            // unrecognized pill kind here so a malformed/unprefixed pill never vanishes silently.
            other => {
                eprintln!("opencode-mentions: unrecognized mention pill kind {other:?} (id={id:?}, text={shown:?}); sent as plain text");
            }
        }
        at = range.end;
    }
    out.text.push_str(&text[at..end]);
    out
}
