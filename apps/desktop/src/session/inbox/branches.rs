//! Per-directory Git branch lookups for Inbox, Home, and draft pickers (`InboxBranchEntry`).
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use opencode_kit::servers::ServerKey;
use rune::Tx;

use crate::Source;

const BRANCH_BATCH: usize = 4;
pub(super) const BRANCH_RETRY_DELAY: f64 = 1.0;
pub(super) const BRANCH_RETRY_MAX_DELAY: f64 = 4.0;
/// Retries after the first failed lookup before the folder rests as failed until a reconnect.
pub(super) const BRANCH_RETRY_LIMIT: u32 = 2;
/// `CHECKOUT_PLACEHOLDER_DELAY_MS` (`new-session/workspace/checkout-machine.ts:2`): wait 250 ms
/// before showing the skeleton pill so fast `/api/vcs` loads paint the branch name directly.
const CHECKOUT_PLACEHOLDER_DELAY: f64 = 0.25;

type BranchKey = (ServerKey, String);

/// Keyed by `(ServerKey, same_directory_key(dir))` (`inbox-branch-machine.ts:14-17`) so two servers
/// sharing a path (e.g. `/workspace`) stay isolated and Windows drive/UNC paths compare
/// case-insensitively (`packages/util/src/path.ts:60-72`).
fn branch_key(server: &ServerKey, dir: &str) -> BranchKey {
    (server.clone(), opencode_servers::home::same_directory_key(dir))
}

/// One folder's git branch load (`InboxBranchEntry`).
#[derive(Clone, Debug, PartialEq)]
enum Branch {
    /// A row displays this folder, but nobody has selected it. Listen for server events only.
    Observed,
    Queued { failures: u32, since: Option<f64>, skeleton: bool },
    Loading { failures: u32, since: f64, skeleton: bool },
    Retrying { failures: u32, due: f64, since: f64, skeleton: bool, dir: String },
    Ok(Option<String>),
    Failed,
}

type BranchResult = (ServerKey, String, Result<Option<String>, ()>);

/// The rows' branches by `(server, folder)`: git info isn't on the session, so each shown folder
/// loads it from `/api/vcs`, and `vcs.branch.updated` keeps it current.
#[derive(Default)]
pub struct Branches {
    by_dir: HashMap<BranchKey, Branch>,
    queue: Vec<(BranchKey, String)>,
    landed: Arc<Mutex<Vec<BranchResult>>>,
}

impl Branches {
    pub fn get(&self, server: &ServerKey, dir: &str) -> Option<&str> {
        match self.by_dir.get(&branch_key(server, dir)) {
            Some(Branch::Ok(b)) => b.as_deref(),
            _ => None,
        }
    }

    /// Whether `dir`'s branch lookup on `server` has settled (`Ok` or `Failed`), rather than loading.
    pub fn settled(&self, server: &ServerKey, dir: &str) -> bool {
        matches!(self.by_dir.get(&branch_key(server, dir)), Some(Branch::Ok(_) | Branch::Failed))
    }

    /// Whether an in-flight lookup for `dir` on `server` has waited past `CHECKOUT_PLACEHOLDER_DELAY`.
    pub fn skeleton(&self, server: &ServerKey, dir: &str) -> bool {
        matches!(
            self.by_dir.get(&branch_key(server, dir)),
            Some(Branch::Queued { skeleton: true, .. } | Branch::Loading { skeleton: true, .. } | Branch::Retrying { skeleton: true, .. })
        )
    }

    /// Merely showing a row must not boot its server location: booting advances project recency.
    pub(super) fn observe(&mut self, server: &ServerKey, dirs: impl IntoIterator<Item = String>) {
        for dir in dirs {
            if !dir.is_empty() {
                self.by_dir.entry(branch_key(server, &dir)).or_insert(Branch::Observed);
            }
        }
    }

    /// Queues a branch lookup once for an explicit draft on screen (`Route::Draft(_)`), without
    /// retrying `Failed` on every poll.
    pub(super) fn enqueue_draft(&mut self, server: &ServerKey, dir: Option<String>) {
        let Some(dir) = dir.filter(|d| !d.is_empty()) else { return };
        let k = branch_key(server, &dir);
        if matches!(self.by_dir.get(&k), None | Some(Branch::Observed)) {
            self.by_dir.insert(k.clone(), Branch::Queued { failures: 0, since: None, skeleton: false });
            self.queue.push((k, dir));
        }
    }

    /// A deliberate operation (choosing a draft folder or entering its picker) needs its branch.
    pub(crate) fn enqueue(&mut self, server: &ServerKey, dirs: impl IntoIterator<Item = String>) {
        for dir in dirs {
            if dir.is_empty() {
                continue;
            }
            let k = branch_key(server, &dir);
            if !matches!(self.by_dir.get(&k), None | Some(Branch::Observed | Branch::Failed)) {
                continue;
            }
            self.by_dir.insert(k.clone(), Branch::Queued { failures: 0, since: None, skeleton: false });
            self.queue.push((k, dir));
        }
    }

    /// A folder's branch changed on `server` (`packages/client/src/solid/data.ts:1478-1481`):
    /// store unconditionally scoped by `server` so events received before a draft opens are never dropped.
    pub fn updated(&mut self, server: &ServerKey, dir: &str, branch: Option<String>) {
        if !dir.is_empty() {
            self.by_dir.insert(branch_key(server, dir), Branch::Ok(branch));
        }
    }

    /// A reconnect isn't a visit. Do not re-open locations for failed decoration reads.
    pub fn reconnected(&mut self, server: &ServerKey) {
        for ((srv, _), b) in &mut self.by_dir {
            if srv == server && *b == Branch::Failed {
                *b = Branch::Observed;
            }
        }
    }

    fn loading(&self) -> usize {
        self.by_dir.values().filter(|b| matches!(b, Branch::Loading { .. })).count()
    }

    /// Takes in finished loads, starts due retries and fills the free slots. True on a change.
    pub(super) fn poll(&mut self, server_key: &ServerKey, s: &Source, waker: Option<&rune::Waker>, now: f64, tx: &mut Tx) -> bool {
        let landed = std::mem::take(&mut *self.landed.lock().unwrap_or_else(|e| e.into_inner()));
        let mut changed = !landed.is_empty();
        for (srv, dir, result) in landed {
            let k = branch_key(&srv, &dir);
            let Some(Branch::Loading { failures, since, skeleton }) = self.by_dir.get(&k).cloned() else { continue };
            let next = match result {
                Ok(branch) => Branch::Ok(branch),
                Err(()) if failures >= BRANCH_RETRY_LIMIT => Branch::Failed,
                Err(()) => {
                    let delay = (BRANCH_RETRY_DELAY * f64::from(1u32 << failures)).min(BRANCH_RETRY_MAX_DELAY);
                    Branch::Retrying { failures: failures + 1, due: now + delay, since, skeleton, dir }
                }
            };
            self.by_dir.insert(k, next);
        }
        let mut next_due = None::<f64>;
        let mut note_due = |due: f64| next_due = Some(next_due.map_or(due, |d| d.min(due)));
        for (k, b) in &mut self.by_dir {
            match b {
                Branch::Queued { since, skeleton, .. } => {
                    let t = *since.get_or_insert(now);
                    if !*skeleton {
                        if now - t >= CHECKOUT_PLACEHOLDER_DELAY {
                            *skeleton = true;
                            changed = true;
                        } else {
                            note_due(t + CHECKOUT_PLACEHOLDER_DELAY);
                        }
                    }
                }
                Branch::Loading { since, skeleton, .. } if !*skeleton => {
                    if now - *since >= CHECKOUT_PLACEHOLDER_DELAY {
                        *skeleton = true;
                        changed = true;
                    } else {
                        note_due(*since + CHECKOUT_PLACEHOLDER_DELAY);
                    }
                }
                Branch::Retrying { failures, due, since, skeleton, dir } => {
                    if !*skeleton {
                        if now - *since >= CHECKOUT_PLACEHOLDER_DELAY {
                            *skeleton = true;
                            changed = true;
                        } else {
                            note_due(*since + CHECKOUT_PLACEHOLDER_DELAY);
                        }
                    }
                    if *due <= now + 1e-9 {
                        let raw = dir.clone();
                        *b = Branch::Queued { failures: *failures, since: Some(*since), skeleton: *skeleton };
                        self.queue.push((k.clone(), raw));
                    } else {
                        note_due(*due);
                    }
                }
                _ => {}
            }
        }
        if let Some(due) = next_due {
            tx.wake_after(Duration::from_secs_f64((due - now).max(0.0)));
        }
        let Source::Live { server, .. } = s else {
            // A fixture has no git: its rows show none.
            for (k, _) in self.queue.drain(..) {
                self.by_dir.insert(k, Branch::Ok(None));
                changed = true;
            }
            return changed;
        };
        while self.loading() < BRANCH_BATCH {
            let Some(pos) = self.queue.iter().position(|((srv, _), _)| srv == server_key) else { break };
            let (k, dir) = self.queue.remove(pos);
            let Some(Branch::Queued { failures, since, skeleton }) = self.by_dir.get(&k).cloned() else { continue };
            let since = since.unwrap_or(now);
            let skeleton = skeleton || now - since >= CHECKOUT_PLACEHOLDER_DELAY;
            if !skeleton {
                tx.wake_after(Duration::from_secs_f64((since + CHECKOUT_PLACEHOLDER_DELAY - now).max(0.0)));
            }
            self.by_dir.insert(k.clone(), Branch::Loading { failures, since, skeleton });
            let (server, landed, wake) = (server.clone(), Arc::clone(&self.landed), waker.cloned());
            let (srv_spawn, dir_spawn) = (server_key.clone(), dir.clone());
            let spawned = rune::task::spawn("inbox-branch", move || {
                let path = format!("/api/vcs?location%5Bdirectory%5D={}", crate::switcher::encode(&dir_spawn));
                // Non-git folders answer HTTP 200 with `{ branch: {} }` and no provider: settle
                // immediately as `Ok(None)` instead of retrying 3× (and 3× again on reconnect).
                // If a newly created git worktree's VCS provider loads later, `vcs.branch.updated`
                // SSE populates it via `Branches::updated` (`packages/client/src/solid/data.ts:1475-1478`).
                let result = server.get(&path).map_err(|_| ()).map(|v| {
                    let data = v.get("data").unwrap_or(&v);
                    data["branch"]["current"].as_str().map(str::to_owned)
                });
                landed.lock().unwrap_or_else(|e| e.into_inner()).push((srv_spawn, dir_spawn, result));
                if let Some(w) = wake {
                    w();
                }
            });
            if spawned.is_err() {
                self.by_dir.insert(k, Branch::Failed);
                break;
            }
        }
        changed
    }
}
