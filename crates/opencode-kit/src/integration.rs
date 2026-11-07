//! The server's integrations (`/api/integration`): model providers and services that connect
//! with an API key, a browser sign-in, a command or an external login, the accounts they hold
//! (`/api/credential`), and the MCP servers of a location (`/api/mcp`). Requests here keep the
//! server's own error message (`{ message }`), as Desktop's `formatServerError` shows it.

use std::time::Duration;

use serde_json::{Value, json};

use crate::client::{Server, encode_component};

/// One integration as the server lists it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Integration {
    pub id: String,
    pub name: String,
    pub methods: Vec<Method>,
    pub connections: Vec<Connection>,
    /// `metadata.source`: `"mcp"` for an MCP server's sign-in.
    pub source: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MethodKind {
    Env,
    Key,
    Oauth,
    Command,
    External,
}

/// A way to connect (`IntegrationMethod`).
#[derive(Clone, Debug, PartialEq)]
pub struct Method {
    pub kind: MethodKind,
    /// None for a key method and environment variables.
    pub id: Option<String>,
    pub label: Option<String>,
    pub form: Vec<Field>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FieldKind {
    String,
    Number,
    Integer,
    Boolean,
    Multiselect,
    /// A link to open in the browser, finished by "I finished".
    External { url: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldOption {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
}

/// One condition of a field's `when`: the field applies while every condition holds.
#[derive(Clone, Debug, PartialEq)]
pub struct When {
    pub key: String,
    pub equal: bool,
    pub value: Value,
}

/// One question of a method's form (`FormField`).
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub key: String,
    pub kind: FieldKind,
    pub title: Option<String>,
    pub description: Option<String>,
    pub required: bool,
    pub hidden: bool,
    pub custom: bool,
    pub placeholder: Option<String>,
    pub default: Option<Value>,
    pub options: Vec<FieldOption>,
    pub when: Vec<When>,
    pub min_items: Option<usize>,
    pub max_items: Option<usize>,
}

/// A connection the integration has: a stored credential (an account) or an environment variable.
#[derive(Clone, Debug, PartialEq)]
pub struct Connection {
    pub credential: bool,
    /// The credential's id, or the variable's name.
    pub id: String,
    /// The account's label, or `$NAME` for a variable.
    pub label: String,
    /// `key`, `oauth` or `external` for a credential.
    pub method: String,
    /// Set when the connection needs a fresh sign-in.
    pub status: Option<ConnectionStatus>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConnectionStatus {
    pub message: String,
    pub url: Option<String>,
}

/// A browser sign-in the server opened (`IntegrationAttempt`).
#[derive(Clone, Debug, PartialEq)]
pub struct Authorization {
    pub attempt: String,
    pub url: String,
    pub instructions: String,
    /// The browser hands back a code to paste, rather than finishing on its own.
    pub code: bool,
    /// The sign-in also takes the redirect URL pasted from the browser.
    pub paste: bool,
    pub created: i64,
    pub expires: i64,
}

/// How a browser sign-in or a command stands (`IntegrationAttemptStatus`).
#[derive(Clone, Debug, PartialEq)]
pub enum AttemptStatus {
    /// A command's latest output comes with it.
    Pending(String),
    Complete,
    Failed(String),
    Expired,
}

/// An MCP server of a location (`McpServer`).
#[derive(Clone, Debug, PartialEq)]
pub struct McpServer {
    pub name: String,
    pub status: McpStatus,
    pub integration: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum McpStatus {
    Connected,
    Pending,
    Disabled,
    Failed(String),
    NeedsAuth(String),
}

impl McpStatus {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::Pending => "pending",
            Self::Disabled => "disabled",
            Self::Failed(_) => "failed",
            Self::NeedsAuth(_) => "needs_auth",
        }
    }
}

impl Integration {
    /// The methods the dialog offers, as the TUI orders them: sign-ins first, API keys last
    /// (`connectMethods`). Environment variables connect nothing from here.
    pub fn connect_methods(&self) -> Vec<Method> {
        let mut out: Vec<Method> = self.methods.iter().filter(|m| m.kind != MethodKind::Env).cloned().collect();
        out.sort_by_key(|m| m.kind == MethodKind::Key);
        out
    }

    /// The stored credentials, the active one first, as the server lists them.
    pub fn accounts(&self) -> Vec<&Connection> {
        self.connections.iter().filter(|c| c.credential).collect()
    }
}

impl Method {
    /// The key method's label, or "API key" when the server names it nothing.
    pub fn title(&self) -> String {
        self.label.clone().unwrap_or_else(|| if self.kind == MethodKind::Key { "API key".into() } else { String::new() })
    }
}

fn s(v: &Value, key: &str) -> Option<String> {
    v[key].as_str().map(str::to_owned)
}

fn list(v: &Value) -> Vec<Value> {
    v["data"].as_array().or_else(|| v.as_array()).cloned().unwrap_or_default()
}

pub fn parse_integration(v: &Value) -> Integration {
    Integration {
        id: s(v, "id").unwrap_or_default(),
        name: s(v, "name").unwrap_or_default(),
        methods: v["methods"].as_array().into_iter().flatten().filter_map(parse_method).collect(),
        connections: v["connections"].as_array().into_iter().flatten().map(parse_connection).collect(),
        source: s(&v["metadata"], "source"),
    }
}

pub fn parse_integrations(v: &Value) -> Vec<Integration> {
    list(v).iter().map(parse_integration).collect()
}

fn parse_method(v: &Value) -> Option<Method> {
    let kind = match v["type"].as_str()? {
        "env" => MethodKind::Env,
        "key" => MethodKind::Key,
        "oauth" => MethodKind::Oauth,
        "command" => MethodKind::Command,
        "external" => MethodKind::External,
        _ => return None,
    };
    let form = v["form"].as_array().into_iter().flatten().filter_map(parse_field).collect();
    Some(Method { kind, id: s(v, "id"), label: s(v, "label"), form })
}

fn parse_field(v: &Value) -> Option<Field> {
    let kind = match v["type"].as_str()? {
        "string" => FieldKind::String,
        "number" => FieldKind::Number,
        "integer" => FieldKind::Integer,
        "boolean" => FieldKind::Boolean,
        "multiselect" => FieldKind::Multiselect,
        "external" => FieldKind::External { url: s(v, "url").unwrap_or_default() },
        _ => return None,
    };
    let options = v["options"].as_array().into_iter().flatten()
        .map(|o| FieldOption { value: s(o, "value").unwrap_or_default(), label: s(o, "label").unwrap_or_default(), description: s(o, "description") })
        .collect();
    let when = v["when"].as_array().into_iter().flatten()
        .map(|w| When { key: s(w, "key").unwrap_or_default(), equal: w["op"].as_str() != Some("neq"), value: w["value"].clone() })
        .collect();
    Some(Field {
        key: s(v, "key")?,
        kind,
        title: s(v, "title"),
        description: s(v, "description"),
        required: v["required"].as_bool().unwrap_or(false),
        hidden: v["hidden"].as_bool().unwrap_or(false),
        custom: v["custom"].as_bool().unwrap_or(false),
        placeholder: s(v, "placeholder"),
        default: Some(v["default"].clone()).filter(|d| !d.is_null()),
        options,
        when,
        min_items: v["minItems"].as_u64().map(|n| n as usize),
        max_items: v["maxItems"].as_u64().map(|n| n as usize),
    })
}

fn parse_connection(v: &Value) -> Connection {
    let credential = v["type"].as_str() == Some("credential");
    let status = v["status"].is_object().then(|| ConnectionStatus { message: s(&v["status"], "message").unwrap_or_default(), url: s(&v["status"], "url") });
    if credential {
        return Connection { credential, id: s(v, "id").unwrap_or_default(), label: s(v, "label").unwrap_or_default(), method: s(v, "method").unwrap_or_default(), status };
    }
    let name = s(v, "name").unwrap_or_default();
    Connection { credential, label: format!("${name}"), id: name, method: String::new(), status }
}

pub fn parse_authorization(v: &Value) -> Authorization {
    let d = if v["data"].is_object() { &v["data"] } else { v };
    Authorization {
        attempt: s(d, "attemptID").unwrap_or_default(),
        url: s(d, "url").unwrap_or_default(),
        instructions: s(d, "instructions").unwrap_or_default(),
        code: d["mode"].as_str() == Some("code"),
        paste: d["paste"].as_bool().unwrap_or(false),
        created: d["time"]["created"].as_i64().unwrap_or(0),
        expires: d["time"]["expires"].as_i64().unwrap_or(0),
    }
}

fn parse_status(v: &Value) -> AttemptStatus {
    let d = if v["data"].is_object() { &v["data"] } else { v };
    match d["status"].as_str() {
        Some("complete") => AttemptStatus::Complete,
        Some("failed") => AttemptStatus::Failed(s(d, "message").unwrap_or_default()),
        Some("expired") => AttemptStatus::Expired,
        _ => AttemptStatus::Pending(s(d, "message").unwrap_or_default()),
    }
}

pub fn parse_mcp(v: &Value) -> Vec<McpServer> {
    list(v).iter().map(|m| {
        let st = &m["status"];
        let error = s(st, "error").unwrap_or_default();
        let status = match st["status"].as_str() {
            Some("connected") => McpStatus::Connected,
            Some("disabled") => McpStatus::Disabled,
            Some("failed") => McpStatus::Failed(error),
            Some("needs_auth") => McpStatus::NeedsAuth(error),
            _ => McpStatus::Pending,
        };
        McpServer { name: s(m, "name").unwrap_or_default(), status, integration: s(m, "integrationID") }
    }).collect()
}

/// `?location[directory]=…`, or nothing for the server's default location.
pub fn location_query(dir: Option<&str>) -> String {
    dir.filter(|d| !d.is_empty()).map_or_else(String::new, |d| format!("?location%5Bdirectory%5D={}", encode_component(d)))
}

impl Server {
    /// A request whose failure says what the server said (`{ message }`), else its status.
    pub fn call(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
        self.call_within(method, path, body, Duration::from_secs(30))
    }

    /// [`Self::call`], waiting up to `timeout` for the answer (a model's reply can take minutes).
    pub fn call_within(&self, method: &str, path: &str, body: Option<Value>, timeout: Duration) -> Result<Value, String> {
        let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(timeout)).http_status_as_error(false).build().into();
        let url = format!("{}{path}", self.base);
        let body = body.map(|b| b.to_string());
        let resp = match (method, body) {
            ("GET", _) => self.authed(agent.get(&url)).call(),
            ("DELETE", _) => self.authed(agent.delete(&url)).call(),
            (_, body) => {
                let req = match method {
                    "PATCH" => agent.patch(&url),
                    "PUT" => agent.put(&url),
                    _ => agent.post(&url),
                };
                self.authed(req.header("Content-Type", "application/json")).send(body.unwrap_or_else(|| "{}".into()).as_bytes())
            }
        };
        let mut resp = resp.map_err(|e| format!("{method} {path}: {e}"))?;
        let status = resp.status().as_u16();
        let text = resp.body_mut().read_to_string().unwrap_or_default();
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if status >= 400 {
            return Err(s(&value, "message").or_else(|| s(&value["data"], "message")).unwrap_or_else(|| format!("{method} {path}: HTTP {status}")));
        }
        Ok(value)
    }

    fn authed<B>(&self, req: ureq::RequestBuilder<B>) -> ureq::RequestBuilder<B> {
        match &self.auth {
            Some(a) => req.header("Authorization", a),
            None => req,
        }
    }

    pub fn integrations(&self, dir: Option<&str>) -> Result<Vec<Integration>, String> {
        self.call("GET", &format!("/api/integration{}", location_query(dir)), None).map(|v| parse_integrations(&v))
    }

    pub fn integration(&self, id: &str, dir: Option<&str>) -> Result<Integration, String> {
        self.call("GET", &format!("/api/integration/{}{}", encode_component(id), location_query(dir)), None).map(|v| parse_integration(&v["data"]))
    }

    fn connect_path(id: &str, tail: &str, dir: Option<&str>) -> String {
        format!("/api/integration/{}/connect/{tail}{}", encode_component(id), location_query(dir))
    }

    /// Saves an API key (`connect/key`), with the method form's answer.
    pub fn connect_key(&self, id: &str, dir: Option<&str>, key: &str, answer: Option<&Value>) -> Result<(), String> {
        let mut body = json!({ "key": key });
        if let Some(a) = answer { body["answer"] = a.clone(); }
        self.call("POST", &Self::connect_path(id, "key", dir), Some(body)).map(|_| ())
    }

    /// Records an external login (`connect/external`), which the server checks itself.
    pub fn connect_external(&self, id: &str, dir: Option<&str>, method: &str, answer: Option<&Value>) -> Result<(), String> {
        let mut body = json!({ "methodID": method });
        if let Some(a) = answer { body["answer"] = a.clone(); }
        self.call("POST", &Self::connect_path(id, "external", dir), Some(body)).map(|_| ())
    }

    /// Starts a browser sign-in (`connect/oauth`).
    pub fn oauth_connect(&self, id: &str, dir: Option<&str>, method: &str, answer: Option<&Value>) -> Result<Authorization, String> {
        let mut body = json!({ "methodID": method });
        if let Some(a) = answer { body["answer"] = a.clone(); }
        self.call("POST", &Self::connect_path(id, "oauth", dir), Some(body)).map(|v| parse_authorization(&v))
    }

    pub fn oauth_status(&self, id: &str, attempt: &str, dir: Option<&str>) -> Result<AttemptStatus, String> {
        self.call("GET", &Self::connect_path(id, &format!("oauth/{}", encode_component(attempt)), dir), None).map(|v| parse_status(&v))
    }

    /// Finishes a sign-in with the code the browser gave (`…/complete`).
    pub fn oauth_complete(&self, id: &str, attempt: &str, dir: Option<&str>, code: &str) -> Result<(), String> {
        self.call("POST", &Self::connect_path(id, &format!("oauth/{}/complete", encode_component(attempt)), dir), Some(json!({ "code": code }))).map(|_| ())
    }

    pub fn oauth_cancel(&self, id: &str, attempt: &str, dir: Option<&str>) -> Result<(), String> {
        self.call("DELETE", &Self::connect_path(id, &format!("oauth/{}", encode_component(attempt)), dir), None).map(|_| ())
    }

    /// Runs a sign-in command on the server (`connect/command`); its attempt id comes back.
    pub fn command_connect(&self, id: &str, dir: Option<&str>, method: &str) -> Result<String, String> {
        self.call("POST", &Self::connect_path(id, "command", dir), Some(json!({ "methodID": method })))
            .map(|v| v["data"]["attemptID"].as_str().unwrap_or_default().to_owned())
    }

    pub fn command_status(&self, id: &str, attempt: &str, dir: Option<&str>) -> Result<AttemptStatus, String> {
        self.call("GET", &Self::connect_path(id, &format!("command/{}", encode_component(attempt)), dir), None).map(|v| parse_status(&v))
    }

    pub fn command_cancel(&self, id: &str, attempt: &str, dir: Option<&str>) -> Result<(), String> {
        self.call("DELETE", &Self::connect_path(id, &format!("command/{}", encode_component(attempt)), dir), None).map(|_| ())
    }

    /// Forgets a stored credential (`DELETE /api/credential/:id`).
    pub fn credential_remove(&self, credential: &str) -> Result<(), String> {
        self.call("DELETE", &format!("/api/credential/{}", encode_component(credential)), None).map(|_| ())
    }

    /// Makes a stored credential its integration's active account.
    pub fn credential_activate(&self, credential: &str) -> Result<(), String> {
        self.call("POST", &format!("/api/credential/{}/activate", encode_component(credential)), None).map(|_| ())
    }

    pub fn credential_rename(&self, credential: &str, label: &str) -> Result<(), String> {
        self.call("PATCH", &format!("/api/credential/{}", encode_component(credential)), Some(json!({ "label": label }))).map(|_| ())
    }

    pub fn mcp_servers(&self, dir: Option<&str>) -> Result<Vec<McpServer>, String> {
        self.call("GET", &format!("/api/mcp{}", location_query(dir)), None).map(|v| parse_mcp(&v))
    }

    /// Connects (`enabled`) or disconnects an MCP server of a location.
    pub fn mcp_toggle(&self, name: &str, dir: Option<&str>, enabled: bool) -> Result<(), String> {
        let verb = if enabled { "connect" } else { "disconnect" };
        self.call("POST", &format!("/api/experimental/mcp/{}/{verb}{}", encode_component(name), location_query(dir)), None).map(|_| ())
    }
}
