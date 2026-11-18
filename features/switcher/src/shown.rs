//! What the palette's list shows while its searches run. Desktop drops a search's rows the moment
//! the query changes (`search.ts`), so each key blanks the list to "Loading" until the server
//! answers. This one never blanks between keys, as VS Code's quick open, Raycast and Linear don't:
//! the typed query's local matches (commands, sessions, the files already found) show in the key's
//! frame; when nothing local matches yet, the last rows stay until the searches answer, and their
//! rows replace them in one frame. "Loading" is a quiet line, and only once a search has shown
//! nothing for [`LOADING_AFTER`]; "No results" only once the searches finish empty.

/// How long a search with nothing to show waits before it says "Loading".
pub const LOADING_AFTER: f64 = 0.25;

/// The list's state, between keys and answers.
#[derive(Clone, Debug, PartialEq)]
pub enum Shown {
    /// `query`'s rows. They stay while a newer query's searches run and nothing matches it yet.
    Rows { query: String },
    /// `query`'s searches finished with nothing; a query that extends it can't find more (every
    /// search here matches substrings or subsequences), so it says "No results" at once.
    Nothing { query: String },
    /// `query`'s searches failed with nothing to show.
    Failed { query: String },
    /// Searching for `query` with nothing to show since `since` (poll clock seconds): blank.
    Waiting { query: String, since: f64 },
    /// Still searching for `query` with nothing to show after [`LOADING_AFTER`].
    Loading { query: String },
}

/// What happened to the list.
#[derive(Clone, Copy, Debug)]
pub enum Event<'a> {
    /// The palette opened at `now`.
    Opened { now: f64 },
    /// The list for `query`: whether anything matches it now, whether a search for it is still
    /// out, and whether its session search failed.
    Seen { query: &'a str, rows: bool, searching: bool, failed: bool, now: f64 },
    /// Time passed.
    Tick { now: f64 },
}

/// What the caller does after a step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    None,
    /// Wake at this poll clock time, for "Loading".
    WakeAt(f64),
    /// The list changed with no new rows: draw it.
    Redraw,
}

/// What shows where the rows would be, when there are none.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Empty {
    /// Nothing yet: the searches have just gone out.
    Blank,
    Loading,
    NoResults,
    Failed,
}

impl Default for Shown {
    fn default() -> Self {
        Shown::Rows { query: String::new() }
    }
}

impl Shown {
    /// The query this state is about.
    pub fn query(&self) -> &str {
        match self {
            Shown::Rows { query } | Shown::Nothing { query } | Shown::Failed { query } | Shown::Waiting { query, .. } | Shown::Loading { query } => query,
        }
    }

    pub fn step(&mut self, event: Event<'_>) -> Effect {
        match event {
            Event::Opened { now } => {
                *self = Shown::Waiting { query: String::new(), since: now };
                Effect::WakeAt(now + LOADING_AFTER)
            }
            Event::Seen { query, rows: true, .. } => {
                *self = Shown::Rows { query: query.to_owned() };
                Effect::None
            }
            Event::Seen { query, searching: false, failed, .. } => {
                let query = query.to_owned();
                *self = if failed { Shown::Failed { query } } else { Shown::Nothing { query } };
                Effect::None
            }
            Event::Seen { query, now, .. } => match self {
                // Held while the newer query is searched.
                Shown::Rows { .. } => Effect::None,
                Shown::Nothing { query: was } if query.starts_with(was.as_str()) => Effect::None,
                Shown::Waiting { query: was, .. } | Shown::Loading { query: was } => {
                    query.clone_into(was);
                    Effect::None
                }
                _ => {
                    *self = Shown::Waiting { query: query.to_owned(), since: now };
                    Effect::WakeAt(now + LOADING_AFTER)
                }
            },
            Event::Tick { now } => match self {
                Shown::Waiting { query, since } if now >= *since + LOADING_AFTER => {
                    *self = Shown::Loading { query: std::mem::take(query) };
                    Effect::Redraw
                }
                Shown::Waiting { since, .. } => Effect::WakeAt(*since + LOADING_AFTER),
                _ => Effect::None,
            },
        }
    }

    /// The query whose rows show for `query` while it's searched with nothing of its own.
    pub fn held(&self, query: &str) -> Option<&str> {
        match self {
            Shown::Rows { query: was } if was != query => Some(was),
            _ => None,
        }
    }

    /// What shows for `query` with no rows: while `searching`, a blank, "Loading", or "No
    /// results" for a query that extends one that found nothing.
    pub fn empty(&self, query: &str, searching: bool, failed: bool) -> Empty {
        match self {
            _ if !searching && failed => Empty::Failed,
            _ if !searching => Empty::NoResults,
            Shown::Nothing { query: was } if query.starts_with(was.as_str()) => Empty::NoResults,
            Shown::Loading { .. } => Empty::Loading,
            _ => Empty::Blank,
        }
    }
}
