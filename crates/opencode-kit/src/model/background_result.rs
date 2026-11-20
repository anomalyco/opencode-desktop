//! Unified `<background-result kind="..." ...>` completion notice parser
//! (`packages/core/src/session/background-result.ts`, anomalyco/opencode#64971), with backward
//! compatibility for legacy `<shell>`, `<subagent>`, `<codemode>`, `<image>`, and `<video>`
//! notice blocks emitted by older OpenCode servers (`v2.2.6`, `v2.3.0`, pre-20 Nov `dev`).
//!
//! Unknown `kind` values are preserved in [`BackgroundResultKind::Unknown`] so future server
//! background-work notices are never dropped or crashed on.

use super::Outcome;

/// Kind of background work reported in a completion notice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackgroundResultKind {
    Shell,
    Subagent,
    CodeMode,
    Image,
    Video,
    /// Unrecognized `kind="..."` from a newer server.
    Unknown(String),
}

impl BackgroundResultKind {
    pub fn parse(raw: &str) -> Self {
        match raw.trim() {
            "shell" => Self::Shell,
            "subagent" => Self::Subagent,
            "codemode" => Self::CodeMode,
            "image" => Self::Image,
            "video" => Self::Video,
            other => Self::Unknown(other.to_owned()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Shell => "shell",
            Self::Subagent => "subagent",
            Self::CodeMode => "codemode",
            Self::Image => "image",
            Self::Video => "video",
            Self::Unknown(raw) => raw.as_str(),
        }
    }
}

/// Structured fields parsed from either `<background-result kind="..." ...>...</background-result>`
/// or a legacy `<shell>` / `<subagent>` / `<codemode>` / `<image>` / `<video>` wrapper block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackgroundResult {
    pub kind: BackgroundResultKind,
    /// `id` (or `jobID`) attribute on the opening tag, if present.
    pub id: Option<String>,
    /// `sessionID` attribute (subagent reports), if present.
    pub session_id: Option<String>,
    /// Raw `state` attribute (`"completed"`, `"error"`, `"cancelled"`, `"running"`, ...).
    pub state: Option<String>,
    /// `command` attribute (shell reports), if present.
    pub command: Option<String>,
    /// `description` attribute (subagent reports), if present.
    pub description: Option<String>,
    /// Inner report body with the wrapper's outer newlines removed.
    pub body: String,
}

impl BackgroundResult {
    /// Maps `state` to the timeline's [`Outcome`], matching `timeline-next/row.tsx`.
    pub fn outcome(&self) -> Outcome {
        match self.state.as_deref() {
            Some("error") => Outcome::Error,
            Some("cancelled") => Outcome::Cancelled,
            _ => Outcome::Completed,
        }
    }

    /// Job or child identifier carried by the XML tag.
    pub fn target_id(&self) -> Option<&str> {
        self.id.as_deref().or(self.session_id.as_deref()).filter(|id| !id.is_empty())
    }

    /// Non-empty readable fallback label when a synthetic notice lacks `description`.
    pub fn fallback_description(&self) -> Option<String> {
        if let Some(desc) = self.description.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            return Some(desc.to_owned());
        }
        if let Some(cmd) = self.command.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            return Some(cmd.to_owned());
        }
        let kind = self.kind.as_str();
        match (kind.is_empty(), self.target_id()) {
            (false, Some(id)) => Some(format!("{kind} · {id}")),
            (false, None) => Some(kind.to_owned()),
            (true, Some(id)) => Some(id.to_owned()),
            (true, None) => self.body.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_owned),
        }
    }
}

const UNIFIED_TAG: &str = "background-result";
const LEGACY_TAGS: [&str; 5] = ["shell", "subagent", "codemode", "image", "video"];

/// Parses a `<background-result ...>...</background-result>` or legacy
/// `<shell>`/`<subagent>`/`<codemode>`/`<image>`/`<video>` block from a synthetic notice's `text`.
///
/// Tolerates leading prose lines (such as `ShellResult.userNotification`'s prefix) and unescaped
/// quotes or `>` characters inside `command="..."` / `description="..."` attributes.
pub fn parse_background_result(text: &str) -> Option<BackgroundResult> {
    if let Some(parsed) = parse_tag(text, UNIFIED_TAG, true) {
        return Some(parsed);
    }
    LEGACY_TAGS.iter().find_map(|tag| parse_tag(text, tag, false))
}

fn parse_tag(text: &str, tag: &str, unified: bool) -> Option<BackgroundResult> {
    let open_start = find_opening_tag(text, tag)?;
    let close_marker = format!("</{tag}>");
    let after_name = open_start + 1 + tag.len();
    let close_rel = text[after_name..].rfind(&close_marker)?;
    let close_start = after_name + close_rel;
    let open_and_body = &text[after_name..close_start];
    let multiline = open_and_body.contains('\n');
    let (attrs, body_start) = parse_opening_attrs(open_and_body, multiline)?;
    let raw_body = &open_and_body[body_start..];
    let body = strip_wrapper_newlines(raw_body).to_owned();

    let mut kind_attr: Option<String> = None;
    let mut id: Option<String> = None;
    let mut session_id: Option<String> = None;
    let mut state: Option<String> = None;
    let mut command: Option<String> = None;
    let mut description: Option<String> = None;

    for (key, value) in attrs {
        let non_empty = || (!value.is_empty()).then(|| value.to_owned());
        match key {
            "kind" => kind_attr = Some(value.to_owned()),
            "id" => id = non_empty(),
            "jobID" if id.is_none() => id = non_empty(),
            "sessionID" => session_id = non_empty(),
            "state" => state = non_empty(),
            "command" => command = non_empty(),
            "description" => description = non_empty(),
            _ => {}
        }
    }

    let kind = if unified {
        BackgroundResultKind::parse(kind_attr.as_deref().unwrap_or(""))
    } else {
        BackgroundResultKind::parse(tag)
    };

    Some(BackgroundResult { kind, id, session_id, state, command, description, body })
}

/// Finds `<tag` at the start of `text` or at the start of a line, followed by whitespace or `>`.
fn find_opening_tag(text: &str, tag: &str) -> Option<usize> {
    let needle = format!("<{tag}");
    let mut offset = 0;
    while let Some(rel) = text[offset..].find(&needle) {
        let pos = offset + rel;
        let line_start = pos == 0 || text[..pos].ends_with('\n');
        let boundary = text[pos + needle.len()..].chars().next().is_some_and(|c| c == '>' || c.is_ascii_whitespace());
        if line_start && boundary {
            return Some(pos);
        }
        offset = pos + 1;
    }
    None
}

/// Parses `key="value"` attributes from `s` (immediately after `<tag`) up to the closing `>` of
/// the opening tag. Returns the parsed attributes and the byte offset immediately after `>`.
///
/// Server emissions (`ShellResult.notification`, `SubagentCompletion.deliver`) interpolate raw
/// command/description text inside quotes without XML-escaping `"` or `>`, so a closing quote is
/// recognized only when immediately followed by `>` or by whitespace and another `attr="` pair.
fn parse_opening_attrs(s: &str, multiline: bool) -> Option<(Vec<(&str, &str)>, usize)> {
    let bytes = s.as_bytes();
    let mut pos = 0;
    let mut attrs = Vec::new();
    while pos < bytes.len() {
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if pos >= bytes.len() {
            return None;
        }
        if bytes[pos] == b'>' {
            return Some((attrs, pos + 1));
        }
        let name_start = pos;
        while pos < bytes.len() && is_attr_name_byte(bytes[pos]) {
            pos += 1;
        }
        if pos == name_start || pos >= bytes.len() || bytes[pos] != b'=' {
            return fallback_close(s, attrs);
        }
        let name = &s[name_start..pos];
        pos += 1; // skip '='
        if pos >= bytes.len() || bytes[pos] != b'"' {
            return fallback_close(s, attrs);
        }
        pos += 1; // skip opening '"'
        let val_start = pos;
        let freeform = matches!(name, "command" | "description");
        let Some(val_end) = find_attr_value_end(s, val_start, freeform && multiline) else {
            return fallback_close(s, attrs);
        };
        attrs.push((name, &s[val_start..val_end]));
        pos = val_end + 1; // skip closing '"'
    }
    None
}

fn find_attr_value_end(s: &str, from: usize, require_line_end_on_gt: bool) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = from;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            let after = i + 1;
            if after < bytes.len() && bytes[after] == b'>' {
                if !require_line_end_on_gt || matches!(bytes.get(after + 1), None | Some(b'\n' | b'\r')) {
                    return Some(i);
                }
            } else if after < bytes.len() && looks_like_next_attr(bytes, after, require_line_end_on_gt) {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

fn looks_like_next_attr(bytes: &[u8], mut i: usize, require_line_end_on_gt: bool) -> bool {
    let ws_start = i;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i == ws_start || i >= bytes.len() {
        return false;
    }
    if bytes[i] == b'>' {
        return !require_line_end_on_gt || matches!(bytes.get(i + 1), None | Some(b'\n' | b'\r'));
    }
    // OpenCode XML attributes start with a lowercase letter (avoids matching shell `FOO="1"`).
    if !bytes[i].is_ascii_lowercase() {
        return false;
    }
    let name_start = i;
    while i < bytes.len() && is_attr_name_byte(bytes[i]) {
        i += 1;
    }
    i > name_start && i + 1 < bytes.len() && bytes[i] == b'=' && bytes[i + 1] == b'"'
}

fn fallback_close<'a>(s: &'a str, attrs: Vec<(&'a str, &'a str)>) -> Option<(Vec<(&'a str, &'a str)>, usize)> {
    s.find('>').map(|gt| (attrs, gt + 1))
}

fn is_attr_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')
}

fn strip_wrapper_newlines(raw: &str) -> &str {
    let stripped = raw.strip_prefix("\r\n").or_else(|| raw.strip_prefix('\n')).unwrap_or(raw);
    stripped.strip_suffix("\r\n").or_else(|| stripped.strip_suffix('\n')).unwrap_or(stripped)
}
