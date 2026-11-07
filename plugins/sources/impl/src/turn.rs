//! A turn's web searches and page fetches (`session-ui/src/sources/sources.ts`): the steps its
//! assistant messages ran, in order, and the distinct pages they met.

use serde_json::Value;

/// A page: its URL, its title when the search named one, its site (the host without `www.`) and
/// its host.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Source {
    pub url: String,
    pub title: Option<String>,
    pub site: String,
    pub host: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StepStatus {
    Running,
    Completed,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Step {
    Search { id: String, query: String, status: StepStatus, results: Vec<Source>, error: Option<String> },
    Fetch { id: String, source: Source, path: String, status: StepStatus, error: Option<String> },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct TurnSources {
    pub steps: Vec<Step>,
    /// Distinct sources in the order the turn met them, deduped by page.
    pub sources: Vec<Source>,
}

/// The assistant messages of the turn user message `from` opened: up to the next user or shell
/// message (`turnMessages`).
pub fn turn_messages<'a>(messages: &'a [Value], from: &str) -> Vec<&'a Value> {
    let Some(start) = messages.iter().position(|m| m["id"] == from) else { return Vec::new() };
    messages[start + 1..]
        .iter()
        .take_while(|m| !matches!(m["type"].as_str(), Some("user" | "shell")))
        .filter(|m| m["type"] == "assistant")
        .collect()
}

pub fn turn_sources(messages: &[&Value]) -> TurnSources {
    let steps: Vec<Step> = messages.iter().flat_map(|m| m["content"].as_array().into_iter().flatten().filter_map(step)).collect();
    let mut seen = Vec::new();
    let mut sources = Vec::new();
    for s in &steps {
        let found: Vec<&Source> = match s {
            Step::Search { results, .. } => results.iter().collect(),
            Step::Fetch { status: StepStatus::Error, .. } => Vec::new(),
            Step::Fetch { source, .. } => vec![source],
        };
        for source in found {
            let key = identity(&source.url);
            if !seen.contains(&key) {
                seen.push(key);
                sources.push(source.clone());
            }
        }
    }
    TurnSources { steps, sources }
}

fn status(tool: &Value) -> Option<StepStatus> {
    match tool["state"]["status"].as_str()? {
        "streaming" => None,
        "running" => Some(StepStatus::Running),
        "completed" => Some(StepStatus::Completed),
        _ => Some(StepStatus::Error),
    }
}

/// `currentToolInput`: a running or finished tool's input object.
fn input(tool: &Value) -> Value {
    let i = &tool["state"]["input"];
    match i.as_str() {
        Some(text) => serde_json::from_str(text).unwrap_or(Value::Null),
        None => i.clone(),
    }
}

/// `currentToolOutput`: a running tool's streamed output, or a finished one's text content.
fn output(tool: &Value) -> String {
    let state = &tool["state"];
    if state["status"] == "running" {
        return state["metadata"]["output"].as_str().unwrap_or_default().to_owned();
    }
    state["content"].as_array().into_iter().flatten().filter(|c| c["type"] == "text").filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n")
}

fn error(tool: &Value) -> Option<String> {
    (tool["state"]["status"] == "error").then(|| tool["state"]["error"]["message"].as_str().unwrap_or_default().to_owned())
}

fn step(content: &Value) -> Option<Step> {
    if content["type"] != "tool" {
        return None;
    }
    let status = status(content)?;
    let id = content["id"].as_str().unwrap_or_default().to_owned();
    match content["name"].as_str()? {
        "websearch" => Some(Step::Search {
            id,
            query: input(content)["query"].as_str().unwrap_or_default().to_owned(),
            status,
            results: if status == StepStatus::Completed { results(&output(content)) } else { Vec::new() },
            error: error(content),
        }),
        "webfetch" => {
            let source = source(input(content)["url"].as_str()?)?;
            let path = path(&source.url);
            Some(Step::Fetch { id, source, path, status, error: error(content) })
        }
        _ => None,
    }
}

/// The websearch tool writes each result as a `## [title](url)` heading; a result without a
/// title repeats its URL.
fn results(text: &str) -> Vec<Source> {
    text.split('\n')
        .filter_map(|line| {
            if !line.starts_with("## [") || !line.ends_with(')') {
                return None;
            }
            let split = line.rfind("](")?;
            if split < 4 {
                return None;
            }
            let mut result = source(&line[split + 2..line.len() - 1])?;
            let title = line[4..split].trim();
            result.title = (!title.is_empty() && title != result.url).then(|| title.to_owned());
            Some(result)
        })
        .collect()
}

/// A web page's parts, or `None` for anything but an `http(s)` URL.
pub fn source(url: &str) -> Option<Source> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority.rsplit('@').next().unwrap_or_default();
    let host = if host.starts_with('[') { host.split(']').next().map(|h| format!("{h}]")).unwrap_or_default() } else { host.split(':').next().unwrap_or_default().to_owned() };
    if host.is_empty() {
        return None;
    }
    let host = host.to_lowercase();
    Some(Source { url: url.to_owned(), title: None, site: host.strip_prefix("www.").unwrap_or(&host).to_owned(), host })
}

/// The path and query after the host: `/a/b?c` (a trailing slash dropped, the fragment too).
fn path_and_query(url: &str) -> (String, String) {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let after = rest.find(['/', '?', '#']).map_or("", |i| &rest[i..]);
    let before_fragment = after.split('#').next().unwrap_or_default();
    let (path, query) = before_fragment.split_once('?').map_or((before_fragment, ""), |(p, q)| (p, q));
    let query = if query.is_empty() { String::new() } else { format!("?{query}") };
    (path.trim_end_matches('/').to_owned(), query)
}

/// One source per page: the scheme, `www.`, a trailing slash and the fragment don't make another.
fn identity(url: &str) -> String {
    let site = source(url).map(|s| s.site).unwrap_or_default();
    let (path, query) = path_and_query(url);
    format!("{site}{path}{query}")
}

fn path(url: &str) -> String {
    let (path, query) = path_and_query(url);
    format!("{path}{query}")
}
