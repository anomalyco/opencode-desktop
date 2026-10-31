//! The catalog: every part an outline may use, what it takes, and one example. The parser, the
//! checker, `seer parts` and the stories all read it, so a part added here works everywhere.

/// What may sit indented under a part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kids {
    /// A leaf.
    None,
    /// Any part (a container).
    Any,
    /// Only these parts.
    Only(&'static [&'static str]),
}

#[derive(Debug)]
pub struct PartSpec {
    pub name: &'static str,
    /// How a line is written, after the indent.
    pub syntax: &'static str,
    /// What it draws, in ten words or fewer.
    pub about: &'static str,
    pub kids: Kids,
    /// One outline using it, checked by the tests and drawn by the stories.
    pub example: &'static str,
}

impl PartSpec {
    pub fn container(&self) -> bool {
        self.kids == Kids::Any
    }
}

/// Parts that lay others out; they count towards an outline's depth.
const C: Kids = Kids::Any;
/// A leaf.
const L: Kids = Kids::None;

pub static CATALOG: &[PartSpec] = &[
    PartSpec { name: "row", syntax: "row", about: "kids side by side; stacks on a narrow screen", kids: C, example: "row\n  box Before\n    text One grammar per fence\n  arrow\n  box After\n    text One outline" },
    PartSpec { name: "col", syntax: "col", about: "kids stacked, as one column of a row", kids: C, example: "row\n  col\n    title Left\n    text Stacked under it\n  col\n    title Right\n    text Also stacked" },
    PartSpec { name: "box", syntax: "box [Title]", about: "a quiet filled frame, its title a faint label", kids: C, example: "box Status\n  item {done} Parser · landed\n  item {working} Views · on a branch" },
    PartSpec { name: "screen", syntax: "screen [Title]", about: "a desktop window around its kids", kids: C, example: "screen Hex settings\n  title Microphone\n  toggle {on} Skip a closed laptop's mic" },
    PartSpec { name: "phone", syntax: "phone [Title]", about: "an iPhone-shaped frame around its kids", kids: C, example: "phone iPhone\n  from WRAITH\n  say Kit {voice}: filter Seer to Hex\n  field Message" },
    PartSpec { name: "arrow", syntax: "arrow", about: "points from the kid before to the next", kids: L, example: "row\n  box Agent\n  arrow\n  box Seer" },
    PartSpec { name: "title", syntax: "title Words", about: "a short heading inside a frame", kids: L, example: "box\n  title Microphone" },
    PartSpec { name: "text", syntax: "text Words", about: "a sentence; Markdown, PRs and cards draw as chips", kids: L, example: "box\n  text The closed laptop's mic drops out of the list." },
    PartSpec { name: "item", syntax: "item [{tone}] Label [· detail]", about: "a row with a status mark and a quiet detail", kids: L, example: "box\n  item {done} Studio Display mic · in use\n  item {risk} MacBook Pro mic · lid closed" },
    PartSpec { name: "kv", syntax: "kv Label: [{tone}] value [· detail]", about: "a label and its value, ruled like facts", kids: L, example: "box\n  kv Status: {proposal} Nothing built · yet\n  kv Owner: t1108" },
    PartSpec { name: "say", syntax: "say Who [{how}]: words", about: "a chat line: you on the right, others left", kids: Kids::Only(&["tools"]), example: "screen Desktop\n  say Kit {voice}: filter Seer to Hex\n  say build {claude-opus-5-5}: Filtered. t1096 waits on a pick.\n    tools wisp, seer" },
    PartSpec { name: "tools", syntax: "tools a, b", about: "the tools the chat line above used", kids: L, example: "screen\n  say build: Done.\n    tools wisp, seer" },
    PartSpec { name: "from", syntax: "from Name", about: "who a phone thread is from, as its header", kids: L, example: "phone\n  from WRAITH" },
    PartSpec { name: "toggle", syntax: "toggle {on|off} Label", about: "a switch, right-aligned beside its label", kids: L, example: "screen Settings\n  toggle {on} Skip a closed laptop's mic\n  toggle {off} Ask before switching" },
    PartSpec { name: "button", syntax: "button [{primary}] Label", about: "a pill button", kids: L, example: "box\n  button {primary} Ship\n  button Hold" },
    PartSpec { name: "field", syntax: "field [Placeholder]", about: "a text field with its placeholder", kids: L, example: "phone\n  field Message" },
    PartSpec { name: "chips", syntax: "chips a, b, c", about: "small labels in a row", kids: L, example: "box\n  chips voice, spoken, agent" },
    PartSpec { name: "rule", syntax: "rule", about: "a hairline between kids", kids: L, example: "box\n  text Above\n  rule\n  text Below" },
];

pub fn spec(name: &str) -> Option<&'static PartSpec> {
    CATALOG.iter().find(|s| s.name == name)
}

/// The colour a `{word}` asks for. Every surface maps these to its own palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Done,
    Risk,
    Proposal,
    Working,
    Accent,
    Quiet,
}

impl Tone {
    /// Done things green, risks red, proposals amber, work in flight the working colour.
    pub fn of(word: &str) -> Tone {
        let w = word.trim().to_lowercase();
        // Short words match whole (`on`, not `one`); longer ones by prefix (`fixed`, `fixes`).
        let any = |xs: &[&str]| xs.iter().any(|x| if x.len() < 4 { w == *x } else { w.starts_with(x) });
        if any(&["done", "ship", "merged", "after", "works", "pass", "fixed", "live", "landed", "built", "ok", "yes", "on"]) {
            Tone::Done
        } else if any(&["risk", "blocked", "broken", "bug", "bugs", "fail", "error", "warning", "danger", "revert", "no"]) {
            Tone::Risk
        } else if any(&["proposal", "proposed", "hold", "draft", "plan", "next", "pending", "waiting", "todo", "review", "decide"]) {
            Tone::Proposal
        } else if any(&["working", "running", "in progress", "building"]) {
            Tone::Working
        } else if any(&["accent", "primary", "pick", "recommended"]) {
            Tone::Accent
        } else {
            Tone::Quiet
        }
    }
}

/// What an agent reads before writing an outline: the grammar, then each part (or one part).
pub fn docs(part: Option<&str>) -> Result<String, String> {
    let list: Vec<&PartSpec> = match part {
        Some(name) => vec![spec(name).ok_or_else(|| format!("no part `{name}`; parts: {}", names()))?],
        None => CATALOG.iter().collect(),
    };
    let mut out = String::new();
    if part.is_none() {
        out.push_str(
            "A ```ui fence holds an outline: a part name first on every line, kids indented two spaces.\n\
             No sizes or colours: colour comes only from {tone} words (done, risk, proposal, working, accent).\n\
             `seer` checks the outline before posting and names any part it doesn't know.\n\n",
        );
    }
    let w = list.iter().map(|s| s.syntax.len()).max().unwrap_or(0);
    for s in &list {
        out.push_str(&format!("{:w$}  {}\n", s.syntax, s.about));
    }
    if let [one] = list.as_slice() {
        out.push_str(&format!("\n```ui\n{}\n```\n", one.example));
    } else {
        out.push_str(&format!("\n```ui\n{}\n```\n", spec("row").map_or("", |s| s.example)));
    }
    Ok(out)
}

pub(crate) fn names() -> String {
    CATALOG.iter().map(|s| s.name).collect::<Vec<_>>().join(", ")
}
