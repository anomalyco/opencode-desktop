//! The outbox's write-ahead journal on disk: one file per server, shared by its windows. A
//! prompt's row keeps the shape the journal always had, so prompts saved by an older build come
//! back.
use std::path::{Path, PathBuf};

use opencode_kit::client::{PathAttachment, Prompt};
use opencode_kit::model::Attachment;

use super::outbox::Operation;
use super::{AfterCheck, Phase, Sent, State};
use crate::prompt_history::Entry;

/// Where the outbox's journal is.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) enum Journal {
    /// In memory only: a fixture, a story, a test or a run that doesn't save.
    #[default]
    Memory,
    File(PathBuf),
    /// The file couldn't be read: never overwrite words it may still hold.
    Blocked,
}

/// A prompt as the journal stores it.
#[derive(serde::Serialize, serde::Deserialize)]
struct Row {
    session: String,
    id: String,
    draft: Entry,
    files: Vec<Attachment>,
    paths: Vec<PathAttachment>,
    inbox: bool,
    why: String,
    state: Stored,
    prompt: Option<Prompt>,
    #[serde(default)]
    durable: bool,
    #[serde(default)]
    operation: Operation,
    #[serde(default)]
    after_check: Option<AfterCheck>,
    #[serde(default)]
    restored_to_composer: bool,
}

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
enum Stored {
    Sending,
    Unconfirmed,
    Failed,
    Accepted,
    Waiting,
}

impl Row {
    fn of(s: &Sent) -> Row {
        let state = match s.state() {
            State::Sending => Stored::Sending,
            State::Unconfirmed => Stored::Unconfirmed,
            State::Failed => Stored::Failed,
            State::Accepted => Stored::Accepted,
            State::Waiting => Stored::Waiting,
        };
        Row {
            session: s.session.clone(),
            id: s.id.clone(),
            draft: s.draft.clone(),
            files: s.files.clone(),
            paths: s.paths.clone(),
            inbox: s.inbox,
            why: s.why().to_owned(),
            state,
            prompt: s.prompt.clone(),
            durable: s.journaled,
            operation: s.operation.clone(),
            after_check: s.then(),
            restored_to_composer: matches!(s.phase, Phase::Failed { restored: true, .. }),
        }
    }

    fn sent(self) -> Sent {
        let (why, then) = (self.why, self.after_check);
        let phase = match self.state {
            Stored::Waiting => Phase::Waiting { why },
            Stored::Sending => Phase::Sending { by: 0, then },
            Stored::Unconfirmed => Phase::Unconfirmed { why, then },
            Stored::Failed => Phase::Failed { why, restored: self.restored_to_composer },
            Stored::Accepted => Phase::Accepted,
        };
        Sent { session: self.session, id: self.id, draft: self.draft, files: self.files, paths: self.paths, inbox: self.inbox, prompt: self.prompt, phase, journaled: self.durable, operation: self.operation }
    }
}

/// The prompts saved at `path`; none when there's no file yet.
pub(super) fn read(path: &Path) -> std::io::Result<Vec<Sent>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice::<Vec<Row>>(&bytes)?.into_iter().map(Row::sent).collect()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// Every journaled prompt the server hasn't confirmed, written and synced before it replaces
/// the file.
pub(super) fn write(path: &Path, sent: &[Sent]) -> std::io::Result<()> {
    use std::{
        fs::{self, OpenOptions},
        io::Write,
    };
    let rows: Vec<Row> = sent.iter().filter(|s| s.journaled && s.phase != Phase::Accepted).map(Row::of).collect();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = std::io::BufWriter::new(options.open(&tmp)?);
    serde_json::to_writer(&mut file, &rows)?;
    file.flush()?;
    file.get_ref().sync_data()?;
    drop(file);
    fs::rename(tmp, path)
}
