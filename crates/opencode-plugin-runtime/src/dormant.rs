//! Pure contribution-record serialization and the [`Records`] state machine for dormant
//! built-in plugins.

use std::collections::{BTreeMap, VecDeque};

use opencode_plugin_spec as spec;
use serde::{Deserialize, Serialize};

use crate::contribs::{Contrib, Contribs};
use crate::icons;

/// One recorded contribution: its key (`<plugin>.<id>`), its spec and the names of the functions it
/// handed over (`run`, `enabled`, `when`, ...).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub key: String,
    pub spec: serde_json::Value,
    pub fns: Vec<String>,
}

/// What a plugin contributed when it last started, and what it was keyed by.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// Its files' hash (`sources::files_hash`).
    pub hash: String,
    /// The interface language its strings were made in.
    pub locale: String,
    /// Its window half's contributions by kind (`commands`, `panels`, ...).
    pub window: BTreeMap<String, Vec<Entry>>,
    /// Its app half's: app menu rows and MCP tools.
    pub app: BTreeMap<String, Vec<Entry>>,
    /// The server event prefixes its window half listened for (`Server:OnEvent`).
    pub events: Vec<String>,
    /// The watched host signals its window half connected to (`Composer.PickerChanged`, ...).
    #[serde(default)]
    pub signals: Vec<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    version: u32,
    plugins: BTreeMap<String, Record>,
}

pub const VERSION: u32 = 2;

/// Why a dormant plugin starts: its first use, the next turn after it was asked for, or idle time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    Use,
    Near,
    Idle,
}

/// When a dormant plugin starts if nothing uses it first: at its first use only; in idle time (a
/// background duty, or a surface asked something only its VM answers); or at the next turn (a
/// session it draws is near, or a surface shows its part now). It only moves later in the list.
///
/// ```text
/// FirstUse ─ want ▶ Idle ─ want_now / near ▶ Next
/// FirstUse ─────────── want_now / near ───▶ Next
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Wait {
    #[default]
    FirstUse,
    Idle,
    Next,
}

/// A dormant plugin's side of the host: its record, the server events kept for it, and when it
/// starts unless used first.
#[derive(Default)]
pub struct Asleep {
    pub record: Record,
    pub events: VecDeque<(String, serde_json::Value)>,
    pub wait: Wait,
}

/// The built-ins' recorded contributions, as this window has read and updated them.
#[derive(Default)]
pub enum Records {
    #[default]
    Unread,
    Clean {
        records: BTreeMap<String, Record>,
        /// The database had no records: an older build's file may be in the cache folder.
        old_file: bool,
    },
    Dirty {
        records: BTreeMap<String, Record>,
        /// The database had no records: an older build's file may be in the cache folder.
        old_file: bool,
    },
}

pub enum RecordsEvent {
    Loaded(Option<BTreeMap<String, Record>>),
    Note { id: String, record: Record },
    Save,
}

pub enum RecordsEffect {
    Write { json: String, forget_old_file: bool },
}

impl Records {
    pub fn is_unread(&self) -> bool {
        matches!(self, Self::Unread)
    }

    pub fn is_dirty(&self) -> bool {
        matches!(self, Self::Dirty { .. })
    }

    pub fn get(&self, id: &str) -> Option<&Record> {
        match self {
            Self::Unread => None,
            Self::Clean { records, .. } | Self::Dirty { records, .. } => records.get(id),
        }
    }

    pub fn step(&mut self, event: RecordsEvent) -> Option<RecordsEffect> {
        match (std::mem::take(self), event) {
            (Self::Unread, RecordsEvent::Loaded(found)) => {
                *self = Self::Clean { old_file: found.is_none(), records: found.unwrap_or_default() };
                None
            }
            (Self::Clean { mut records, old_file }, RecordsEvent::Note { id, record }) => {
                if records.get(&id) != Some(&record) {
                    records.insert(id, record);
                    *self = Self::Dirty { records, old_file };
                } else {
                    *self = Self::Clean { records, old_file };
                }
                None
            }
            (Self::Dirty { mut records, old_file }, RecordsEvent::Note { id, record }) => {
                if records.get(&id) != Some(&record) {
                    records.insert(id, record);
                }
                *self = Self::Dirty { records, old_file };
                None
            }
            (Self::Dirty { records, old_file }, RecordsEvent::Save) => match serde_json::to_string(&File { version: VERSION, plugins: records.clone() }) {
                Ok(json) => {
                    *self = Self::Clean { records, old_file: false };
                    Some(RecordsEffect::Write { json, forget_old_file: old_file })
                }
                Err(_) => {
                    *self = Self::Clean { records, old_file };
                    None
                }
            },
            (state, _) => {
                *self = state;
                None
            }
        }
    }
}

/// Parses a serialized contribution records file; returns an empty map on schema mismatch.
pub fn parse_records(text: &str) -> BTreeMap<String, Record> {
    match serde_json::from_str::<File>(text) {
        Ok(f) if f.version == VERSION => f.plugins,
        _ => BTreeMap::new(),
    }
}

macro_rules! kinds {
    ($m:ident) => {
        $m! {
            commands: CommandSpec,
            panels: PanelSpec,
            settings: SettingSpec,
            settings_pages: SettingsPageSpec,
            menu_items: MenuItemSpec,
            menubar_items: MenubarItemSpec,
            link_handlers: LinkHandlerSpec,
            titlebar_items: TitlebarItemSpec,
            composer_pills: ComposerPillSpec,
            code_blocks: CodeBlockSpec,
            code_block_actions: CodeBlockActionSpec,
            slots: SlotSpec,
            themes: ThemeSpec,
            file_viewers: FileViewerSpec,
            server_sources: ServerSourceSpec,
            dialogs: DialogSpec,
            popovers: PopoverSpec,
            mcp_tools: McpToolSpec,
        }
    };
}

/// `contribs`' kinds as record entries (`only`: just those kinds; empty: all).
pub fn record_kinds(contribs: &Contribs, only: &[&str]) -> BTreeMap<String, Vec<Entry>> {
    fn entries<S: Serialize>(list: &[Contrib<S>]) -> Vec<Entry> {
        list.iter()
            .filter_map(|c| {
                Some(Entry {
                    key: c.key.to_owned(),
                    spec: without_nulls(serde_json::to_value(&c.spec).ok()?),
                    fns: c.fns.keys().map(|k| (*k).to_owned()).collect(),
                })
            })
            .collect()
    }
    let mut out = BTreeMap::new();
    macro_rules! put {
        ($($field:ident: $ty:ident,)*) => {
            $(
                if (only.is_empty() || only.contains(&stringify!($field))) && !contribs.$field.is_empty() {
                    out.insert(stringify!($field).to_owned(), entries(&contribs.$field));
                }
            )*
        };
    }
    kinds!(put);
    out
}

/// Contributions as a record lists them: their specs and keys, no functions.
pub fn contribs_of(kinds: &BTreeMap<String, Vec<Entry>>) -> Contribs {
    fn list<S: serde::de::DeserializeOwned>(kinds: &BTreeMap<String, Vec<Entry>>, kind: &str) -> Vec<Contrib<S>> {
        kinds
            .get(kind)
            .into_iter()
            .flatten()
            .filter_map(|e| {
                Some(Contrib {
                    spec: serde_json::from_value(e.spec.clone()).ok()?,
                    fns: Default::default(),
                    key: icons::intern(&e.key),
                })
            })
            .collect()
    }
    let mut c = Contribs::default();
    macro_rules! take {
        ($($field:ident: $ty:ident,)*) => {
            $( c.$field = list::<spec::$ty>(kinds, stringify!($field)); )*
        };
    }
    kinds!(take);
    c
}

/// `value` without its null fields, at any depth: a spec's options that are none.
fn without_nulls(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => map.into_iter().filter(|(_, v)| !v.is_null()).map(|(k, v)| (k, without_nulls(v))).collect::<serde_json::Map<_, _>>().into(),
        serde_json::Value::Array(items) => items.into_iter().map(without_nulls).collect::<Vec<_>>().into(),
        other => other,
    }
}
