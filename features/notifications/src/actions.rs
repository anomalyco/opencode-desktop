//! Toasts you answer where you are: a permission's Allow once, Allow always and Deny, and a
//! one-question form's choices or a text box, on the request's notification (Windows; macOS
//! takes the same `Ask` once `rune_notify` builds its categories). A button answers through the
//! window that posted the toast, as its dock would, without bringing the app forward; the
//! answer takes the toast out everywhere ([`crate::answered`]). Anything the toast can't
//! answer whole (several questions, a request the window no longer holds) opens the session
//! instead, as a click on the toast does.

use opencode_kit::model::{FieldKind, Pending};
use serde_json::{Value, json};

use crate::host::{NotificationReply, NotificationsHost};

/// What a request's toast can answer.
#[derive(Clone, Debug, PartialEq)]
pub enum Ask {
    /// A tool wants to run: `what` is the action and what it acts on.
    Permission { request: String, what: String },
    /// One question: its field and how it's answered.
    Question { request: String, title: String, field: String, choice: Choice },
    /// A form the toast doesn't answer: it opens.
    Form { request: String },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Choice {
    /// Free words, in a text box.
    Text,
    /// One of these (value, label), a button each; `own`: or the person's own words, in a text box.
    Options { options: Vec<(String, String)>, own: bool },
    Boolean,
}

/// Windows shows at most five buttons: four choices, or three and the text box's send.
const OPTIONS: usize = 4;
/// What the toast's second line shows of a command, at most.
const WHAT: usize = 160;

impl Ask {
    pub fn request(&self) -> &str {
        match self {
            Ask::Permission { request, .. } | Ask::Question { request, .. } | Ask::Form { request } => request,
        }
    }
}

/// The ask a `permission.asked` or `form.created` event carries.
pub fn ask(ty: &str, data: &Value) -> Option<Ask> {
    match ty {
        "permission.asked" => match opencode_kit::model::parse_permission(data)? {
            Pending::Permission { id, action, resources, .. } => {
                let what = if resources.is_empty() { action } else { format!("{action}: {}", resources.join(", ")) };
                Some(Ask::Permission { request: id, what: clip(&what) })
            }
            Pending::Form { .. } => None,
        },
        "form.created" => match opencode_kit::model::parse_form(data)? {
            Pending::Form { id, title, fields, .. } => {
                let [f] = fields.as_slice() else {
                    return Some(Ask::Form { request: id });
                };
                let choice = match f.kind {
                    FieldKind::Boolean => Choice::Boolean,
                    FieldKind::Text if f.options.is_empty() => Choice::Text,
                    FieldKind::Text if f.options.len() + usize::from(f.custom) <= OPTIONS => Choice::Options {
                        options: f.options.iter().map(|o| (o.value.clone(), o.label.clone())).collect(),
                        own: f.custom,
                    },
                    _ => return Some(Ask::Form { request: id }),
                };
                // The question itself, else its field's name, else the form's.
                let title = [&f.description, &f.title, &title].into_iter().find(|t| !t.is_empty()).cloned().unwrap_or_default();
                Some(Ask::Question { request: id, title: clip(&title), field: f.key.clone(), choice })
            }
            Pending::Permission { .. } => None,
        },
        _ => None,
    }
}

/// The toast's text under its title: the session, then what it asks.
pub fn body(session: &str, ask: &Ask) -> String {
    match ask {
        Ask::Permission { what, .. } if !what.is_empty() => format!("{session}\n{what}"),
        Ask::Question { title, .. } if !title.is_empty() => format!("{session}\n{title}"),
        _ => session.to_owned(),
    }
}

/// `n` with the ask's buttons and what they answer.
pub fn buttons(n: rune_notify::Notification, ask: &Ask) -> rune_notify::Notification {
    let n = n.info("request", ask.request());
    match ask {
        Ask::Permission { .. } => n
            .info("kind", "permission")
            .action("once", opencode_i18n::t!("ui.permission.allowOnce"))
            .action("always", opencode_i18n::t!("ui.permission.allowAlways"))
            .action("reject", opencode_i18n::t!("ui.permission.deny")),
        Ask::Question { field, choice, .. } => {
            let n = n.info("kind", "question").info("field", field);
            match choice {
                Choice::Text => n.reply("answer", opencode_i18n::t!("ui.question.custom.placeholder"), opencode_i18n::t!("common.submit")),
                Choice::Boolean => n.action("true", "Yes").action("false", "No"),
                Choice::Options { options, own } => {
                    let n = options.iter().fold(n, |n, (value, label)| n.action(format!("pick:{value}"), label));
                    if *own {
                        n.reply("answer", opencode_i18n::t!("ui.question.custom.placeholder"), opencode_i18n::t!("common.submit"))
                    } else {
                        n
                    }
                }
            }
        }
        Ask::Form { .. } => n,
    }
}

/// The clicks for this window: those it posted, and those whose window is gone.
pub fn take(host: &dyn NotificationsHost) -> Vec<rune_notify::Click> {
    let mut all = crate::CLICKS.lock().unwrap_or_else(|e| e.into_inner());
    let window_id = host.window_id();
    let mine = |c: &rune_notify::Click| {
        let orphan = |w: &str| w.is_empty() || !host.window_exists(w);
        // An extension toast's click goes to the window that posted it, or else to one whose
        // engine runs extensions: a window without one would drop it.
        if c.get("kind") == Some("extension") {
            return c.get("window").is_some_and(|w| w == window_id) || (c.get("window").is_none_or(orphan) && host.takes_extension_clicks());
        }
        c.get("window").is_none_or(|w| w == window_id || orphan(w))
    };
    let (taken, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut *all).into_iter().partition(mine);
    *all = kept;
    taken
}

/// What a click on a toast comes to in this window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Its button answered the request.
    Answered,
    /// The request isn't here yet (a launch the click started is still connecting or reading
    /// the session's requests): the click waits for a later poll.
    Wait,
    /// A click on the toast itself, or an answer the window can't give: the session opens.
    Open,
}

/// Polls a waiting click waits for its request, at most: the launch's connection and the read.
const TRIES: u32 = 200;
const TRIED: &str = "rune.tries";

/// Answers `click`'s button, when it's one this window can answer.
pub fn act(host: &mut dyn NotificationsHost, click: &mut rune_notify::Click) -> Outcome {
    let (Some(action), Some(request)) = (click.action().map(str::to_owned), click.get("request").map(str::to_owned)) else {
        return Outcome::Open;
    };
    if !host.has_pending_request(&request) {
        let tries = click.get(TRIED).and_then(|t| t.parse::<u32>().ok()).unwrap_or(0);
        let session = click.get("session").unwrap_or_default().to_owned();
        let has_tab = !session.is_empty() && {
            let root = host.root_session_id(&session).to_owned();
            host.has_tab_for_root(&root)
        };
        if tries >= TRIES || !has_tab {
            return Outcome::Open;
        }
        host.read_requests(&session);
        click.info.retain(|(k, _)| k != TRIED);
        click.info.push((TRIED.to_owned(), (tries + 1).to_string()));
        return Outcome::Wait;
    }
    let request = request.as_str();
    match click.get("kind") {
        Some("permission") if matches!(action.as_str(), "once" | "always" | "reject") => {
            host.reply_permission_or_question(request, NotificationReply::Permission(&action));
        }
        Some("question") => {
            let field = click.get("field").unwrap_or_default().to_owned();
            let value = match action.as_str() {
                "answer" => match click.input("answer").map(str::trim).filter(|t| !t.is_empty()) {
                    Some(text) => Value::from(text),
                    None => return Outcome::Open,
                },
                "true" | "false" => Value::Bool(action == "true"),
                pick => match pick.strip_prefix("pick:") {
                    Some(v) => Value::from(v),
                    None => return Outcome::Open,
                },
            };
            host.reply_permission_or_question(request, NotificationReply::Question(json!({ field: value })));
        }
        _ => return Outcome::Open,
    }
    crate::answered(host, request);
    Outcome::Answered
}

fn clip(s: &str) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() <= WHAT {
        return s;
    }
    format!("{}…", s.chars().take(WHAT - 1).collect::<String>())
}
