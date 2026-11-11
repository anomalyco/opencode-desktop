//! `StartupImportState`: the schema check, the one-time import of the files Rune kept before
//! the database, the read-only first-run seed from Desktop's `drafts.sqlite`, and the removal
//! of imported files. Whether any of it is needed is answered from inside the database (the
//! header's `user_version` flags), so a settled profile never touches the old files again.
//!
//! The work that only matters once and isn't needed to draw (removing imported files, the seed)
//! waits for the first frame and runs off the UI thread. A file is retired (moved into a dated
//! folder, purged after 14 days of clean starts) only through
//! [`VerifiedPaths`], which only `VerifyingImport` builds, on a read-back that matched.

use std::path::PathBuf;

use rune::machine::Effects;

/// Files whose every imported row was read back, after a checkpoint, through a fresh
/// connection. Built only by [`step`] on [`ImportEvent::Verified`].
#[derive(Debug, PartialEq, Eq)]
pub struct VerifiedPaths(Vec<PathBuf>);

impl VerifiedPaths {
    pub fn paths(&self) -> &[PathBuf] {
        &self.0
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub enum ImportState {
    #[default]
    MigratingSchema,
    ScanningSources {
        seed_settled: bool,
    },
    ImportingLegacyFiles {
        files: usize,
    },
    VerifyingImport {
        files: usize,
    },
    /// Imported and verified; the files go once the first frame is up.
    RetireDeferred {
        verified: VerifiedPaths,
        /// Files that didn't verify: kept, and the import stays unsettled so the next launch
        /// tries again (existing rows win, so it never overwrites newer state).
        kept: usize,
    },
    RetiringLegacyFiles {
        kept: usize,
    },
    /// A fresh profile: Desktop's database may seed it, once the first frame is up.
    SeedDeferred,
    SeedingFromOfficial,
    Ready,
    /// The database couldn't open or migrate: state lives in memory for this run.
    DegradedInMemory {
        error: String,
    },
}

#[derive(Debug)]
pub enum ImportEvent {
    /// pending: a group the running build cut over isn't settled yet.
    SchemaReady { pending: bool, seed_settled: bool },
    OpenOrMigrateFailed { error: String },
    FoundRuneFiles { files: usize },
    /// None of the pending groups' files exist. resh: no legacy file of any group either, and
    /// the database is empty.
    NoRuneFiles { fresh: bool },
    CommittedAndCheckpointed,
    ImportFailed { error: String },
    Verified { matched: Vec<PathBuf>, mismatched: Vec<PathBuf> },
    FirstFramePresented,
    FilesRetired { failed: usize },
    OfficialSeedSettled,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ImportEffect {
    Scan,
    Import,
    Verify,
    /// Move the files into the dated retired folder.
    Retire(VerifiedPaths),
    Seed,
    /// Sets the pending groups' header flags: their files never need a look again.
    MarkGroupsSettled,
    MarkSeedSettled,
    FallBackToMemory,
    Report(String),
}

pub fn step(state: &mut ImportState, event: ImportEvent) -> Effects<ImportEffect> {
    let (next, effects) = transition(std::mem::take(state), event);
    *state = next;
    effects
}

fn transition(state: ImportState, event: ImportEvent) -> (ImportState, Effects<ImportEffect>) {
    use ImportEffect as E;
    use ImportEvent as Ev;
    use ImportState as S;
    match (state, event) {
        (S::MigratingSchema, Ev::OpenOrMigrateFailed { error }) => (S::DegradedInMemory { error: error.clone() }, Effects::two(E::FallBackToMemory, E::Report(error))),
        (S::MigratingSchema, Ev::SchemaReady { pending: false, .. }) => (S::Ready, Effects::none()),
        (S::MigratingSchema, Ev::SchemaReady { pending: true, seed_settled }) => (S::ScanningSources { seed_settled }, Effects::one(E::Scan)),
        (S::ScanningSources { .. }, Ev::FoundRuneFiles { files }) => (S::ImportingLegacyFiles { files }, Effects::one(E::Import)),
        // Nothing of Rune's to import: a fresh profile may still take Desktop's state.
        (S::ScanningSources { seed_settled: false }, Ev::NoRuneFiles { fresh: true }) => (S::SeedDeferred, Effects::one(E::MarkGroupsSettled)),
        (S::ScanningSources { .. }, Ev::NoRuneFiles { .. }) => (S::Ready, Effects::two(E::MarkGroupsSettled, E::MarkSeedSettled)),
        (S::ImportingLegacyFiles { files }, Ev::CommittedAndCheckpointed) => (S::VerifyingImport { files }, Effects::one(E::Verify)),
        (S::ImportingLegacyFiles { .. }, Ev::ImportFailed { error }) => (S::Ready, Effects::one(E::Report(format!("legacy import failed, files kept: {error}")))),
        (S::VerifyingImport { .. }, Ev::Verified { matched, mismatched }) => {
            let kept = mismatched.len();
            let report = (kept > 0).then(|| E::Report(format!("{kept} legacy file(s) didn't verify and stay on disk: {mismatched:?}")));
            let mut effects = Effects::none();
            effects.extend(report);
            // Rune had files, so the profile isn't fresh: no seed from Desktop.
            effects.push(E::MarkSeedSettled);
            (S::RetireDeferred { verified: VerifiedPaths(matched), kept }, effects)
        }
        (S::RetireDeferred { verified, kept }, Ev::FirstFramePresented) => (S::RetiringLegacyFiles { kept }, Effects::one(E::Retire(verified))),
        (S::RetiringLegacyFiles { kept }, Ev::FilesRetired { failed }) => {
            if kept == 0 && failed == 0 {
                return (S::Ready, Effects::one(E::MarkGroupsSettled));
            }
            (S::Ready, Effects::one(E::Report(format!("legacy import stays unsettled: {kept} unverified, {failed} not retired"))))
        }
        (S::SeedDeferred, Ev::FirstFramePresented) => (S::SeedingFromOfficial, Effects::one(E::Seed)),
        (S::SeedingFromOfficial, Ev::OfficialSeedSettled) => (S::Ready, Effects::one(E::MarkSeedSettled)),
        (state, _) => (state, Effects::none()),
    }
}
