//! Per-tab bounded console and network diagnostics with extension exclusion and sensitive-header
//! redaction (`packages/gui-plugins/src/browser/{diagnostics,diagnostics-machine}.ts`).

use std::collections::{BTreeMap, HashMap, VecDeque};

use browser_policy::extension_origin;
use serde_json::{Value, json};

pub const MAX_RETAINED_MESSAGES: usize = 500;
pub const MAX_RETAINED_REQUESTS: usize = 500;
const MAX_TEXT: usize = 100_000;
const MAX_HEADER_COUNT: usize = 100;
const MAX_HEADER_VALUE: usize = 2_000;
const MAX_HEADERS_TOTAL: usize = 16_000;

const REDACTED_HEADERS: &[&str] = &["cookie", "set-cookie", "authorization", "proxy-authorization"];

#[derive(Clone, Debug)]
pub struct ConsoleEntry {
    pub id: String,
    pub timestamp_ms: f64,
    pub level: &'static str,
    pub text: String,
    pub text_truncated: bool,
    pub source: Option<(String, u64, u64)>,
}

impl ConsoleEntry {
    pub fn to_json(&self) -> Value {
        let mut obj = json!({
            "id": self.id,
            "timestampMs": self.timestamp_ms,
            "level": self.level,
            "text": self.text,
            "textTruncated": self.text_truncated,
        });
        if let Some((url, line, col)) = &self.source {
            obj["source"] = json!({ "url": url, "line": line, "column": col });
        }
        obj
    }
}

#[derive(Clone, Debug)]
pub enum HopPhase {
    Pending { started: f64 },
    Completed { duration_ms: f64 },
    Failed { duration_ms: f64, failure: String },
}

#[derive(Clone, Debug)]
pub struct NetworkHop {
    pub id: String,
    pub native_id: String,
    pub session_id: Option<String>,
    pub extension: bool,
    pub url: String,
    pub method: String,
    pub resource_type: &'static str,
    pub timestamp_ms: f64,
    pub status_code: Option<u64>,
    pub request_headers: BTreeMap<String, String>,
    pub response_headers: BTreeMap<String, String>,
    pub headers_truncated: bool,
    pub mime_type: String,
    pub post_data: Option<String>,
    pub has_post_data: bool,
    pub post_data_truncated: bool,
    pub phase: HopPhase,
}

impl NetworkHop {
    pub fn summary_json(&self) -> Value {
        let mut obj = json!({
            "id": self.id,
            "url": self.url,
            "method": self.method,
            "resourceType": self.resource_type,
            "timestampMs": self.timestamp_ms,
        });
        if let Some(code) = self.status_code {
            obj["statusCode"] = json!(code);
        }
        match &self.phase {
            HopPhase::Pending { .. } => {
                obj["state"] = json!("pending");
            }
            HopPhase::Completed { duration_ms } => {
                obj["state"] = json!("completed");
                obj["durationMs"] = json!(duration_ms);
            }
            HopPhase::Failed { duration_ms, failure } => {
                obj["state"] = json!("failed");
                obj["durationMs"] = json!(duration_ms);
                obj["failure"] = json!(failure);
            }
        }
        obj
    }

    /// Official `diagnostics-machine.ts:312`:
    /// `^(text\/|application\/(json|.*\+json|javascript|xml|.*\+xml|x-www-form-urlencoded))`, case-insensitive.
    pub fn is_text_response(&self) -> bool {
        let m = self.mime_type.to_ascii_lowercase();
        if m.starts_with("text/") {
            return true;
        }
        let Some(app) = m.strip_prefix("application/") else {
            return false;
        };
        ["json", "javascript", "xml", "x-www-form-urlencoded"].iter().any(|p| app.starts_with(p))
            || app.contains("+json")
            || app.contains("+xml")
    }

    /// A hop with no response, or a WebSocket, has no body to read (`diagnostics-machine.ts:308-309`).
    fn response_captured(&self) -> bool {
        self.status_code.is_some() && self.resource_type != "websocket"
    }

    /// Whether `Network.getResponseBody` may be asked for this hop's body.
    pub fn response_body_fetchable(&self) -> bool {
        !matches!(self.phase, HopPhase::Pending { .. }) && self.response_captured() && self.is_text_response()
    }
}

/// Bounded console and network diagnostics store for one browser tab.
#[derive(Default)]
pub struct TabDiagnostics {
    scope: u64,
    seq: u64,
    messages: VecDeque<ConsoleEntry>,
    dropped_messages: u64,
    requests: VecDeque<NetworkHop>,
    by_native: HashMap<String, String>,
    dropped_requests: u64,
    /// `ExtraInfo` events that came before their `requestWillBeSent`, by `session:requestId`,
    /// replayed when it comes (official `MAX_STASHED_CHAINS` keys, `MAX_WIRE_QUEUE` each).
    stashed: HashMap<String, Vec<(String, Value)>>,
    stash_order: VecDeque<String>,
}

const MAX_STASHED_CHAINS: usize = 128;
const MAX_WIRE_QUEUE: usize = 16;

impl TabDiagnostics {
    pub fn new(scope: u64) -> Self {
        Self { scope, ..Self::default() }
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.requests.clear();
        self.by_native.clear();
        self.stashed.clear();
        self.stash_order.clear();
        self.dropped_messages = 0;
        self.dropped_requests = 0;
    }

    fn next_id(&mut self) -> String {
        self.seq = self.seq.saturating_add(1);
        format!("diag_{}:{}", self.scope, self.seq)
    }

    /// Ingests one CDP event (`Runtime.*`, `Log.entryAdded`, `Network.*`), dropping any entry
    /// attributed to a browser extension and redacting `cookie`/`set-cookie`/`authorization`/
    /// `proxy-authorization` headers.
    pub fn ingest_cdp_event(&mut self, method: &str, params: &Value, session_id: Option<&str>) {
        match method {
            "Runtime.consoleAPICalled" => {
                if stack_is_extension(params.get("stackTrace")) {
                    return;
                }
                let ty = params.get("type").and_then(Value::as_str).unwrap_or("log");
                let level = match ty {
                    "error" | "assert" => "error",
                    "warning" => "warning",
                    "debug" => "debug",
                    _ => "info",
                };
                let text = params
                    .get("args")
                    .and_then(Value::as_array)
                    .map(|args| {
                        args.iter()
                            .map(|a| {
                                if a.get("type").and_then(Value::as_str) == Some("string") {
                                    a.get("value").and_then(Value::as_str).unwrap_or("").to_owned()
                                } else if let Some(v) = a.get("value") {
                                    v.to_string()
                                } else {
                                    a.get("description")
                                        .or_else(|| a.get("unserializableValue"))
                                        .or_else(|| a.get("type"))
                                        .and_then(Value::as_str)
                                        .unwrap_or("")
                                        .to_owned()
                                }
                            })
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .unwrap_or_default();
                let source = first_frame_source(params.get("stackTrace"));
                let ts = params.get("timestamp").and_then(Value::as_f64).unwrap_or(0.0);
                self.push_console(level, &text, ts, source);
            }
            "Log.entryAdded" => {
                let Some(entry) = params.get("entry") else { return };
                let url = entry.get("url").and_then(Value::as_str);
                if url.is_some_and(extension_origin) || stack_is_extension(entry.get("stackTrace")) {
                    return;
                }
                let level = match entry.get("level").and_then(Value::as_str).unwrap_or("info") {
                    "verbose" => "debug",
                    "warning" => "warning",
                    "error" => "error",
                    _ => "info",
                };
                let text = entry.get("text").and_then(Value::as_str).unwrap_or("");
                let ts = entry.get("timestamp").and_then(Value::as_f64).unwrap_or(0.0);
                let source = url.filter(|u| !u.is_empty()).map(|u| {
                    let line = entry.get("lineNumber").and_then(Value::as_u64).unwrap_or(0) + 1;
                    (truncate_chars(u, MAX_TEXT), line, 1)
                });
                self.push_console(level, text, ts, source);
            }
            "Runtime.exceptionThrown" => {
                let Some(details) = params.get("exceptionDetails") else { return };
                let url = details.get("url").and_then(Value::as_str);
                if url.is_some_and(extension_origin) || stack_is_extension(details.get("stackTrace")) {
                    return;
                }
                let text = details
                    .get("exception")
                    .and_then(|e| e.get("description"))
                    .and_then(Value::as_str)
                    .or_else(|| details.get("text").and_then(Value::as_str))
                    .unwrap_or("Uncaught exception");
                let ts = params.get("timestamp").and_then(Value::as_f64).unwrap_or(0.0);
                let source = url.filter(|u| !u.is_empty()).map(|u| {
                    let line = details.get("lineNumber").and_then(Value::as_u64).unwrap_or(0) + 1;
                    let col = details.get("columnNumber").and_then(Value::as_u64).unwrap_or(0) + 1;
                    (truncate_chars(u, MAX_TEXT), line, col)
                });
                self.push_console("error", text, ts, source);
            }
            "Network.requestWillBeSent" => {
                let Some(req_id) = params.get("requestId").and_then(Value::as_str) else { return };
                let Some(req) = params.get("request") else { return };
                let url = req.get("url").and_then(Value::as_str).unwrap_or("");
                let doc_url = params.get("documentURL").and_then(Value::as_str).unwrap_or("");
                let init_url = params.get("initiator").and_then(|i| i.get("url")).and_then(Value::as_str);
                let ext = extension_origin(url)
                    || extension_origin(doc_url)
                    || init_url.is_some_and(extension_origin)
                    || stack_is_extension(params.get("initiator").and_then(|i| i.get("stack")));
                // A redirect reuses the request ID: its earlier hop ends there with the redirect's
                // response (official `transitionChain` `redirected`, reported as completed).
                if let Some(redirect) = params.get("redirectResponse")
                    && let Some(prev) = self.find_native_mut(params, session_id)
                    && let HopPhase::Pending { started } = prev.phase
                {
                    let ts = params.get("timestamp").and_then(Value::as_f64).unwrap_or(started);
                    let (headers, trunc) = redact_headers(redirect.get("headers"));
                    prev.headers_truncated |= merge_headers(&mut prev.response_headers, headers) || trunc;
                    prev.status_code = prev.status_code.or_else(|| redirect.get("status").and_then(Value::as_u64));
                    prev.mime_type = redirect.get("mimeType").and_then(Value::as_str).unwrap_or("").to_owned();
                    prev.phase = HopPhase::Completed { duration_ms: ((ts - started) * 1000.0).max(0.0) };
                }
                let (headers, trunc) = redact_headers(req.get("headers"));
                let post_raw = req.get("postData").and_then(Value::as_str);
                let post_trunc = post_raw.is_some_and(|s| s.len() > 20_000);
                let post_data = post_raw.map(|s| truncate_chars(s, 20_000));
                let id = self.next_id();
                let key = format!("{}:{req_id}", session_id.unwrap_or(""));
                self.by_native.insert(key, id.clone());
                self.requests.push_back(NetworkHop {
                    id,
                    native_id: req_id.to_owned(),
                    session_id: session_id.map(str::to_owned),
                    extension: ext,
                    url: truncate_chars(url, 16_384),
                    method: req.get("method").and_then(Value::as_str).unwrap_or("GET").to_owned(),
                    resource_type: normalize_resource_type(params.get("type").and_then(Value::as_str).unwrap_or("other")),
                    timestamp_ms: params.get("wallTime").and_then(Value::as_f64).unwrap_or(0.0) * 1000.0,
                    status_code: None,
                    request_headers: headers,
                    response_headers: BTreeMap::new(),
                    headers_truncated: trunc,
                    mime_type: String::new(),
                    post_data,
                    has_post_data: req.get("hasPostData").and_then(Value::as_bool).unwrap_or(false),
                    post_data_truncated: post_trunc,
                    phase: HopPhase::Pending {
                        started: params.get("timestamp").and_then(Value::as_f64).unwrap_or(0.0),
                    },
                });
                while self.requests.len() > MAX_RETAINED_REQUESTS {
                    if let Some(old) = self.requests.pop_front() {
                        let k = format!("{}:{}", old.session_id.as_deref().unwrap_or(""), old.native_id);
                        if self.by_native.get(&k).map(String::as_str) == Some(old.id.as_str()) {
                            self.by_native.remove(&k);
                        }
                        self.dropped_requests += 1;
                    }
                }
                let key = format!("{}:{req_id}", session_id.unwrap_or(""));
                if let Some(early) = self.stashed.remove(&key) {
                    self.stash_order.retain(|k| *k != key);
                    for (method, params) in early {
                        self.ingest_cdp_event(&method, &params, session_id);
                    }
                }
            }
            "Network.requestWillBeSentExtraInfo" => {
                if let Some(hop) = self.find_native_mut(params, session_id) {
                    let (extra, trunc) = redact_headers(params.get("headers"));
                    hop.headers_truncated |= merge_headers(&mut hop.request_headers, extra) || trunc;
                } else {
                    self.stash(method, params, session_id);
                }
            }
            "Network.responseReceived" => {
                if let Some(hop) = self.find_native_mut(params, session_id)
                    && let Some(resp) = params.get("response")
                {
                    let (headers, trunc) = redact_headers(resp.get("headers"));
                    hop.headers_truncated |= merge_headers(&mut hop.response_headers, headers) || trunc;
                    if hop.status_code.is_none() {
                        hop.status_code = resp.get("status").and_then(Value::as_u64);
                    }
                    hop.mime_type = resp.get("mimeType").and_then(Value::as_str).unwrap_or("").to_owned();
                }
            }
            "Network.responseReceivedExtraInfo" => {
                if let Some(hop) = self.find_native_mut(params, session_id) {
                    let (extra, trunc) = redact_headers(params.get("headers"));
                    hop.headers_truncated |= merge_headers(&mut hop.response_headers, extra) || trunc;
                    if let Some(code) = params.get("statusCode").and_then(Value::as_u64) {
                        hop.status_code = Some(code);
                    }
                } else {
                    self.stash(method, params, session_id);
                }
            }
            "Network.loadingFinished" => {
                let ts = params.get("timestamp").and_then(Value::as_f64).unwrap_or(0.0);
                if let Some(hop) = self.find_native_mut(params, session_id)
                    && let HopPhase::Pending { started } = hop.phase
                {
                    hop.phase = HopPhase::Completed { duration_ms: ((ts - started) * 1000.0).max(0.0) };
                }
            }
            "Network.loadingFailed" => {
                let ts = params.get("timestamp").and_then(Value::as_f64).unwrap_or(0.0);
                let err = params
                    .get("errorText")
                    .and_then(Value::as_str)
                    .unwrap_or("Request failed")
                    .to_owned();
                if let Some(hop) = self.find_native_mut(params, session_id)
                    && let HopPhase::Pending { started } = hop.phase
                {
                    hop.phase = HopPhase::Failed {
                        duration_ms: ((ts - started) * 1000.0).max(0.0),
                        failure: truncate_chars(&err, 2_048),
                    };
                }
            }
            _ => {}
        }
    }

    /// Holds an `ExtraInfo` that came before its request, bounded per request and in all.
    fn stash(&mut self, method: &str, params: &Value, session_id: Option<&str>) {
        let Some(req_id) = params.get("requestId").and_then(Value::as_str) else { return };
        let key = format!("{}:{req_id}", session_id.unwrap_or(""));
        let queue = self.stashed.entry(key.clone()).or_default();
        if queue.is_empty() {
            self.stash_order.push_back(key);
        }
        if queue.len() < MAX_WIRE_QUEUE {
            queue.push((method.to_owned(), params.clone()));
        }
        while self.stash_order.len() > MAX_STASHED_CHAINS {
            if let Some(oldest) = self.stash_order.pop_front() {
                self.stashed.remove(&oldest);
            }
        }
    }

    fn find_native_mut(&mut self, params: &Value, session_id: Option<&str>) -> Option<&mut NetworkHop> {
        let req_id = params.get("requestId").and_then(Value::as_str)?;
        let key = format!("{}:{req_id}", session_id.unwrap_or(""));
        let pub_id = self.by_native.get(&key)?.clone();
        self.requests.iter_mut().rev().find(|h| h.id == pub_id)
    }

    fn push_console(
        &mut self,
        level: &'static str,
        raw_text: &str,
        timestamp_ms: f64,
        source: Option<(String, u64, u64)>,
    ) {
        let truncated = raw_text.chars().count() > 2_000;
        let id = self.next_id();
        self.messages.push_back(ConsoleEntry {
            id,
            timestamp_ms,
            level,
            text: truncate_chars(raw_text, 2_000),
            text_truncated: truncated,
            source,
        });
        while self.messages.len() > MAX_RETAINED_MESSAGES {
            self.messages.pop_front();
            self.dropped_messages += 1;
        }
    }

    pub fn query_console(&self, min_level: Option<&str>, limit: Option<usize>) -> Value {
        let min_rank = level_rank(min_level.unwrap_or("info"));
        let matching: Vec<Value> = self
            .messages
            .iter()
            .filter(|e| level_rank(e.level) >= min_rank)
            .map(ConsoleEntry::to_json)
            .collect();
        let (items, truncated) = bounded_tail(&matching, limit.unwrap_or(100));
        json!({
            "messages": items,
            "truncated": truncated,
            "dropped": self.dropped_messages,
        })
    }

    pub fn query_network_list(
        &self,
        url_contains: Option<&str>,
        resource_type: Option<&str>,
        limit: Option<usize>,
    ) -> Value {
        let matching: Vec<Value> = self
            .requests
            .iter()
            .filter(|h| {
                !h.extension
                    && url_contains.is_none_or(|sub| h.url.contains(sub))
                    && resource_type.is_none_or(|rt| h.resource_type == rt)
            })
            .map(NetworkHop::summary_json)
            .collect();
        let (items, truncated) = bounded_tail(&matching, limit.unwrap_or(100));
        json!({
            "requests": items,
            "truncated": truncated,
            "dropped": self.dropped_requests,
        })
    }

    pub fn network_hop(&self, id: &str) -> Option<&NetworkHop> {
        self.requests.iter().find(|h| !h.extension && h.id == id)
    }

    pub fn format_network_get(
        hop: &NetworkHop,
        include_body: bool,
        include_response_body: bool,
        max_body_chars: usize,
        fetched_response: Option<Result<String, &'static str>>,
    ) -> Value {
        let req_headers: Vec<Value> = hop
            .request_headers
            .iter()
            .map(|(k, v)| json!({ "name": k, "value": v }))
            .collect();
        let resp_headers: Vec<Value> = hop
            .response_headers
            .iter()
            .map(|(k, v)| json!({ "name": k, "value": v }))
            .collect();
        let req_body = if !include_body {
            json!({ "state": "notRequested" })
        } else if let Some(data) = &hop.post_data {
            let truncated = hop.post_data_truncated || data.chars().count() > max_body_chars;
            json!({ "state": "text", "text": truncate_chars(data, max_body_chars), "truncated": truncated })
        } else if hop.has_post_data {
            json!({ "state": "unavailable", "reason": "notCaptured" })
        } else {
            json!({ "state": "empty" })
        };
        // `include_response_body` is already `includeResponseBody ?? includeBody` (`diagnostics.ts:287`).
        let resp_body = if !include_response_body {
            json!({ "state": "notRequested" })
        } else if matches!(hop.phase, HopPhase::Pending { .. }) {
            json!({ "state": "pending" })
        } else if !hop.response_captured() {
            json!({ "state": "unavailable", "reason": "notCaptured" })
        } else if !hop.is_text_response() {
            json!({ "state": "unavailable", "reason": "binary" })
        } else {
            match fetched_response {
                Some(Ok(text)) if text.is_empty() => json!({ "state": "empty" }),
                Some(Ok(text)) => {
                    let truncated = text.chars().count() > max_body_chars;
                    json!({ "state": "text", "text": truncate_chars(&text, max_body_chars), "truncated": truncated })
                }
                Some(Err(reason)) => json!({ "state": "unavailable", "reason": reason }),
                None => json!({ "state": "unavailable", "reason": "backendUnavailable" }),
            }
        };
        json!({
            "request": hop.summary_json(),
            "requestHeaders": req_headers,
            "responseHeaders": resp_headers,
            "headersTruncated": hop.headers_truncated,
            "requestBody": req_body,
            "responseBody": resp_body,
        })
    }
}

/// Redacts sensitive headers (`cookie`, `set-cookie`, `authorization`, `proxy-authorization`) to
/// `"<redacted>"`, lowercasing names and enforcing size bounds (`diagnostics-machine.ts:496-522`).
pub fn redact_headers(raw: Option<&Value>) -> (BTreeMap<String, String>, bool) {
    let Some(obj) = raw.and_then(Value::as_object) else {
        return (BTreeMap::new(), false);
    };
    let mut out = BTreeMap::new();
    let mut total = 0usize;
    let mut truncated = obj.len() > MAX_HEADER_COUNT;
    for (k, v) in obj.iter().take(MAX_HEADER_COUNT) {
        let lower = k.to_ascii_lowercase();
        let name = truncate_chars(&lower, 2_048);
        let value_raw = if REDACTED_HEADERS.contains(&name.as_str()) {
            "<redacted>".to_owned()
        } else {
            match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            }
        };
        let remaining = MAX_HEADERS_TOTAL.saturating_sub(total + name.len());
        if remaining == 0 {
            truncated = true;
            break;
        }
        let val_bound = truncate_chars(&value_raw, MAX_HEADER_VALUE.min(remaining));
        truncated |= val_bound.len() < value_raw.len() || name.len() < k.len();
        total += name.len() + val_bound.len();
        out.insert(name, val_bound);
    }
    (out, truncated)
}

/// Merges `extra` (already redacted) under the headers a hop holds, which win, and bounds the
/// result as one set: official re-trims the merged headers (`diagnostics-machine.ts:148,165,182`
/// `trimHeaders({ ...extra, ...latest })`). True when anything was cut.
fn merge_headers(dst: &mut BTreeMap<String, String>, extra: BTreeMap<String, String>) -> bool {
    for (k, v) in extra {
        dst.entry(k).or_insert(v);
    }
    let mut truncated = dst.len() > MAX_HEADER_COUNT;
    let (mut kept, mut total) = (0usize, 0usize);
    dst.retain(|name, value| {
        let remaining = MAX_HEADERS_TOTAL.saturating_sub(total + name.len());
        if kept >= MAX_HEADER_COUNT || remaining == 0 {
            truncated = true;
            return false;
        }
        if value.chars().count() > remaining {
            *value = truncate_chars(value, remaining);
            truncated = true;
        }
        kept += 1;
        total += name.len() + value.len();
        true
    });
    truncated
}

fn stack_is_extension(stack: Option<&Value>) -> bool {
    let mut cur = stack;
    while let Some(s) = cur {
        if let Some(frames) = s.get("callFrames").and_then(Value::as_array)
            && frames
                .iter()
                .any(|f| f.get("url").and_then(Value::as_str).is_some_and(extension_origin))
        {
            return true;
        }
        cur = s.get("parent");
    }
    false
}

fn first_frame_source(stack: Option<&Value>) -> Option<(String, u64, u64)> {
    let frame = stack?.get("callFrames")?.as_array()?.first()?;
    let url = frame.get("url").and_then(Value::as_str)?;
    if url.is_empty() {
        return None;
    }
    let line = frame.get("lineNumber").and_then(Value::as_u64).unwrap_or(0) + 1;
    let col = frame.get("columnNumber").and_then(Value::as_u64).unwrap_or(0) + 1;
    Some((truncate_chars(url, MAX_TEXT), line, col))
}

fn level_rank(level: &str) -> u8 {
    match level {
        "debug" => 0,
        "info" => 1,
        "warning" => 2,
        "error" => 3,
        _ => 1,
    }
}

fn normalize_resource_type(raw: &str) -> &'static str {
    match raw.to_ascii_lowercase().as_str() {
        "document" => "document",
        "stylesheet" => "stylesheet",
        "image" => "image",
        "media" => "media",
        "font" => "font",
        "script" => "script",
        "xhr" => "xhr",
        "fetch" => "fetch",
        "eventsource" => "eventsource",
        "websocket" => "websocket",
        "manifest" => "manifest",
        _ => "other",
    }
}

fn bounded_tail(items: &[Value], limit: usize) -> (Vec<Value>, bool) {
    let start = items.len().saturating_sub(limit);
    let mut selected = Vec::new();
    let mut size = 0usize;
    for item in items[start..].iter().rev() {
        let len = item.to_string().len();
        if size + len > MAX_TEXT {
            break;
        }
        size += len;
        selected.push(item.clone());
    }
    selected.reverse();
    let truncated = selected.len() < items.len();
    (selected, truncated)
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    s.chars().take(max).collect()
}
