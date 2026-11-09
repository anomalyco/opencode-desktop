//! The Add server and Edit server form (Desktop's `servers/connect/machine.ts`, the dialog's
//! surface): a pairing link or an address and password, and an optional name, checked against the
//! server before it's saved. `Submitting` carries the attempt its results must match; `Closed`
//! keeps the draft so the dialog shows its values as it goes, and ignores every later event, so
//! the form closes at most once. Desktop has no camera, so the scanner never shows.

/// A backstop for a check that never answers: above the slowest real path (redeeming a link, then
/// two 30 s health checks).
pub const CONNECT_TIMEOUT_MS: u64 = 75_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Link,
    Password,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Link,
    Url,
    Name,
    Password,
}

/// The saved server an edit started from, as it is now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Original {
    pub url: String,
    pub name: Option<String>,
    pub password: Option<String>,
    pub unauthorized: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Add,
    Edit(Original),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Draft {
    pub link: String,
    pub url: String,
    pub name: String,
    pub password: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    LinkInvalid,
    LinkLegacy,
    AddressInvalid,
    /// Already worded: why the link didn't redeem.
    Redeem(String),
    ConnectionFailed,
}

/// Where a server lives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Http {
    pub url: String,
    pub password: Option<String>,
}

/// A saved server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    pub http: Http,
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Save {
    Add(Saved),
    /// The edit moved the server to another address.
    Replace { original: String, server: Saved },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Editing { error: Option<Error> },
    Submitting { attempt: u64 },
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub attempt: u64,
    pub target: Target,
    pub method: Method,
    pub draft: Draft,
    pub status: Status,
}

pub enum Event {
    MethodChanged(Method),
    FieldChanged(Field, String),
    Submitted,
    SubmitSucceeded { attempt: u64, http: Http },
    SubmitFailed { attempt: u64, error: Error },
    SubmitTimedOut { attempt: u64 },
    /// The edited server as the list has it now; None once it's removed.
    OriginalSynced(Option<Original>),
    Disposed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Redeem the link, then check the server: the edited address first, since the token works on
    /// every address of its server.
    ConnectLink { attempt: u64, link: String, preferred: Option<String> },
    ConnectHttp { attempt: u64, http: Http },
    TimerStart { attempt: u64 },
    TimerStop,
    Save(Save),
    Close { saved: bool },
}

impl State {
    pub fn new(target: Target) -> State {
        // A signed-out server needs a new pairing link; otherwise editing starts from its address.
        let (method, draft) = match &target {
            Target::Add => (Method::Link, Draft::default()),
            Target::Edit(o) => (
                if o.unauthorized { Method::Link } else { Method::Password },
                Draft { link: String::new(), url: o.url.clone(), name: o.name.clone().unwrap_or_default(), password: o.password.clone().unwrap_or_default() },
            ),
        };
        State { attempt: 0, target, method, draft, status: Status::Editing { error: None } }
    }

    fn error(&self) -> Option<&Error> {
        match &self.status {
            Status::Editing { error } => error.as_ref(),
            _ => None,
        }
    }

    pub fn step(&mut self, event: Event) -> Vec<Effect> {
        if self.status == Status::Closed {
            return Vec::new();
        }
        match event {
            Event::Disposed => {
                let submitting = matches!(self.status, Status::Submitting { .. });
                self.status = Status::Closed;
                if submitting { vec![Effect::TimerStop] } else { Vec::new() }
            }
            Event::OriginalSynced(original) => self.synced(original),
            Event::SubmitSucceeded { attempt, http } if self.status == (Status::Submitting { attempt }) => self.saved(http),
            Event::SubmitFailed { attempt, error } if self.status == (Status::Submitting { attempt }) => {
                self.status = Status::Editing { error: Some(error) };
                vec![Effect::TimerStop]
            }
            Event::SubmitTimedOut { attempt } if self.status == (Status::Submitting { attempt }) => {
                self.status = Status::Editing { error: Some(Error::ConnectionFailed) };
                vec![Effect::TimerStop]
            }
            _ if matches!(self.status, Status::Submitting { .. }) => Vec::new(),
            Event::MethodChanged(method) => {
                if self.method != method || self.error().is_some() {
                    self.method = method;
                    self.status = Status::Editing { error: None };
                }
                Vec::new()
            }
            Event::FieldChanged(field, value) => {
                let errored = self.error().is_some();
                let slot = match field {
                    Field::Link => &mut self.draft.link,
                    Field::Url => &mut self.draft.url,
                    Field::Name => &mut self.draft.name,
                    Field::Password => &mut self.draft.password,
                };
                if *slot != value || errored {
                    *slot = value;
                    self.status = Status::Editing { error: None };
                }
                Vec::new()
            }
            Event::Submitted => self.submit(),
            _ => Vec::new(),
        }
    }

    /// Where the form points: a pasted pairing link wins, in either field; else the address and
    /// password.
    fn submit(&mut self) -> Vec<Effect> {
        let value = if self.method == Method::Link { self.draft.link.clone() } else { self.draft.url.clone() };
        let attempt = self.attempt + 1;
        let original = match &self.target {
            Target::Edit(o) => Some(o.clone()),
            Target::Add => None,
        };
        if super::pairing::pairing_link(&value).is_some() {
            self.attempt = attempt;
            self.status = Status::Submitting { attempt };
            return vec![Effect::ConnectLink { attempt, link: value, preferred: original.map(|o| o.url) }, Effect::TimerStart { attempt }];
        }
        if super::pairing::legacy_pairing_link(&value) {
            self.status = Status::Editing { error: Some(Error::LinkLegacy) };
            return Vec::new();
        }
        if self.method == Method::Link {
            let Some(address) = super::pairing::bare_server_address(&value) else {
                self.status = Status::Editing { error: Some(Error::LinkInvalid) };
                return Vec::new();
            };
            // A server address typed here belongs to the password form.
            self.method = Method::Password;
            self.draft.link.clear();
            self.draft.url = address;
            self.status = Status::Editing { error: None };
            return Vec::new();
        }
        let Some(url) = super::pairing::server_address(&value) else {
            self.status = Status::Editing { error: Some(Error::AddressInvalid) };
            return Vec::new();
        };
        let password = Some(self.draft.password.clone()).filter(|p| !p.is_empty());
        let name = Some(self.draft.name.trim().to_owned()).filter(|n| !n.is_empty());
        // Nothing changed: close, unless the server rejects these credentials, so saving checks them.
        if let Some(o) = &original
            && !o.unauthorized
            && url == o.url
            && name == o.name
            && password == o.password
        {
            self.status = Status::Closed;
            return vec![Effect::Close { saved: false }];
        }
        self.attempt = attempt;
        self.status = Status::Submitting { attempt };
        vec![Effect::ConnectHttp { attempt, http: Http { url, password } }, Effect::TimerStart { attempt }]
    }

    fn saved(&mut self, http: Http) -> Vec<Effect> {
        let server = Saved { http, name: Some(self.draft.name.trim().to_owned()).filter(|n| !n.is_empty()) };
        let save = match &self.target {
            Target::Edit(o) if o.url != server.http.url => Save::Replace { original: o.url.clone(), server },
            _ => Save::Add(server),
        };
        self.status = Status::Closed;
        vec![Effect::TimerStop, Effect::Save(save), Effect::Close { saved: true }]
    }

    fn synced(&mut self, original: Option<Original>) -> Vec<Effect> {
        let Target::Edit(current) = &self.target else { return Vec::new() };
        // The edited server was removed elsewhere: there is nothing left to save to.
        let Some(original) = original else {
            let submitting = matches!(self.status, Status::Submitting { .. });
            self.status = Status::Closed;
            let mut out = if submitting { vec![Effect::TimerStop] } else { Vec::new() };
            out.push(Effect::Close { saved: false });
            return out;
        };
        if *current != original {
            self.target = Target::Edit(original);
        }
        Vec::new()
    }
}

/// What the dialog shows (`selectServerConnectView`).
pub struct View {
    pub busy: bool,
    pub disabled: bool,
    pub signed_out: bool,
    pub error: Option<Error>,
}

pub fn view(s: &State) -> View {
    View {
        busy: matches!(s.status, Status::Submitting { .. }),
        disabled: !matches!(s.status, Status::Editing { .. }),
        signed_out: matches!(&s.target, Target::Edit(o) if o.unauthorized),
        error: s.error().cloned(),
    }
}
