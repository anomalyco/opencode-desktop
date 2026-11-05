//! What must hold after every frame, and once the session has settled.
//!
//! Every frame: no prompt shows twice; prompts never swap places, and one above Working never
//! drops below it; a prompt that vanished doesn't come back (a flicker); no prompt the server
//! has is rolled back as failed; no toast blames the server while the link, not the server, is
//! down. After 30 s of a good link: no "Not confirmed" for a prompt the server has, and none forever
//! for one it lacks; no "Pending" for one it delivered; nothing shown that it lacks, nothing it
//! has missing; Stop shows exactly while the server runs. Settled (a good link for 60 s, the
//! server idle): the screen's prompts are the server's, in its order, then its waiting steers;
//! the queues match; no Stop, Working, Sending or stop in progress.

use std::collections::{HashMap, HashSet};

use super::trace::{Screen, label};

/// What the server has: the sim's state, or a real server's as last read.
pub trait Truth {
    /// Delivered prompts' ids, in transcript order.
    fn users(&self) -> Vec<String>;
    /// Undelivered prompts in admission order: (id, text, queued).
    fn inbox(&self) -> Vec<(String, String, bool)>;
    /// The session's run is going.
    fn busy(&self) -> bool;
    fn up(&self) -> bool;
    /// Background helpers running under the session.
    fn helpers(&self) -> usize;
    /// Every prompt's words by id.
    fn words(&self) -> HashMap<String, String>;

    fn delivered(&self, id: &str) -> bool {
        self.users().iter().any(|u| u == id)
    }

    fn has(&self, id: &str) -> bool {
        self.delivered(id) || self.inbox().iter().any(|i| i.0 == id)
    }

    fn waiting(&self, queue: bool) -> Vec<String> {
        self.inbox().into_iter().filter(|i| i.2 == queue).map(|i| i.0).collect()
    }
}

/// The link as the checks need it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Health {
    /// Since when it has been good: fine, the server up, the event stream open and whole.
    pub good_since: Option<f64>,
    /// The last time the network was dead with the server up.
    pub last_dead: Option<f64>,
    /// The last time the server was down: a toast that blames it then is fair.
    pub last_down: Option<f64>,
}

/// How long the link must have been good before a stuck state counts.
pub const GRACE: f64 = 30.0;
/// How long after a dead link a toast blaming the server counts.
const BLAME_WINDOW: f64 = 15.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A prompt shows twice.
    Doubled,
    /// Two prompts swap places, or one above Working drops below it.
    Jump,
    /// A prompt vanishes and comes back.
    Flicker,
    /// "Failed to send prompt" took back a prompt the server has.
    RolledBack,
    /// "Not confirmed" stays on a prompt the server has (or forever on one it lacks).
    StuckSending,
    /// "Pending" stays on a prompt the server delivered.
    StuckPending,
    /// A prompt shows that the server doesn't have, not marked Sending or failed.
    Phantom,
    /// A prompt the server has never shows.
    Missing,
    /// Stop shows while the server is idle, or Send while it runs.
    BusyMismatch,
    /// "Still trying" stays up after the server answers.
    StuckToast,
    /// A toast blames the server while the link is what's down.
    BlamesServer,
    /// ↵ in an empty composer with Stop showing didn't stop.
    EnterDidntStop,
    /// A held ↵ stopped the run.
    HeldEnterStopped,
    /// Settled, the screen and the server disagree.
    SettledWrong,
    /// The session never settled.
    NeverSettled,
    /// The app panicked.
    Panicked,
}

#[derive(Clone, Debug)]
pub struct Failure {
    pub kind: Kind,
    pub t: f64,
    pub detail: String,
}

#[derive(Default)]
pub struct Oracle {
    pub failures: Vec<Failure>,
    seen: HashSet<Kind>,
    last: Option<Screen>,
    /// Prompts that have shown, and those shown above the Working row.
    shown: HashSet<String>,
    above: HashSet<String>,
    /// Prompts that vanished, and whether a failed-send toast came with it.
    vanished: HashMap<String, bool>,
    /// When each stuck condition began: (kind, id).
    since: HashMap<(Kind, String), f64>,
    last_toast: Option<u64>,
}

impl Oracle {
    fn fail(&mut self, kind: Kind, t: f64, detail: String) {
        if self.seen.insert(kind) {
            self.failures.push(Failure { kind, t, detail });
        }
    }

    pub fn failed(&self) -> bool {
        !self.failures.is_empty()
    }

    pub fn check(&mut self, t: f64, s: &Screen, sim: &dyn Truth, link: Health) {
        let words = names(s, sim);
        let name = |id: &str| label(&words, id);

        let mut once = HashSet::new();
        for id in &s.prompts {
            if !once.insert(id) {
                self.fail(Kind::Doubled, t, format!("{} shows twice", name(id)));
            }
        }
        let mut by_words: HashMap<&str, usize> = HashMap::new();
        for id in &s.prompts {
            *by_words.entry(words.get(id).map_or("", String::as_str)).or_default() += 1;
        }
        if let Some((w, _)) = by_words.iter().find(|(w, n)| !w.is_empty() && **n > 1) {
            self.fail(Kind::Doubled, t, format!("{w} shows twice under different ids"));
        }

        if let Some(last) = self.last.take() {
            self.order(t, &last, s, sim, &words);
            let toast_now = s.toast.as_ref().map(|x| x.0);
            let failed_toast = s.toast.as_ref().is_some_and(|x| x.1 == crate::send_failed::TITLE) && toast_now != self.last_toast;
            for id in &last.prompts {
                if !s.prompts.contains(id) && !s.queued.contains(id) {
                    self.vanished.insert(id.clone(), failed_toast);
                }
            }
        }
        for id in &s.prompts {
            if self.vanished.remove(id).is_some() {
                self.fail(Kind::Flicker, t, format!("{} vanished, then came back", name(id)));
            }
        }
        let rolled: Vec<String> = self.vanished.iter().filter(|(id, failed)| **failed && sim.has(id)).map(|(id, _)| id.clone()).collect();
        for id in rolled {
            self.fail(Kind::RolledBack, t, format!("{} was taken back as failed, but the server has it", name(&id)));
            self.vanished.remove(&id);
        }
        for id in &s.prompts {
            self.shown.insert(id.clone());
            if s.above_actions(id) == Some(true) {
                self.above.insert(id.clone());
            }
        }

        self.blame(t, s, sim, link);
        self.stuck(t, s, sim, link, &words);
        self.last_toast = s.toast.as_ref().map(|x| x.0);
        self.last = Some(s.clone());
    }

    /// Prompts keep their order, and none above Working drops below it. One the server never
    /// got can't hold its place: that it says Not confirmed or Failed is checked elsewhere.
    fn order(&mut self, t: f64, last: &Screen, s: &Screen, sim: &dyn Truth, words: &HashMap<String, String>) {
        let name = |id: &str| label(words, id);
        let common: Vec<&String> = last.prompts.iter().filter(|id| s.prompts.contains(id) && sim.has(id)).collect();
        let now: Vec<usize> = common.iter().map(|id| s.prompts.iter().position(|p| p == *id).unwrap_or(0)).collect();
        if let Some(k) = now.windows(2).position(|w| w[0] > w[1]) {
            self.fail(Kind::Jump, t, format!("{} moved above {}", name(common[k + 1]), name(common[k])));
        }
        for id in &s.prompts {
            if self.above.contains(id) && s.above_actions(id) == Some(false) {
                self.fail(Kind::Jump, t, format!("{} was above Working, now below it", name(id)));
            }
        }
    }

    fn blame(&mut self, t: f64, s: &Screen, sim: &dyn Truth, link: Health) {
        let Some((id, title, desc)) = &s.toast else { return };
        if Some(*id) == self.last_toast {
            return;
        }
        let dead = link.last_dead.is_some_and(|d| t - d <= BLAME_WINDOW) && !link.last_down.is_some_and(|d| t - d <= BLAME_WINDOW);
        let text = format!("{title} {desc}");
        // "Still trying" only says nothing has answered, which is true of a dead link too.
        let calm = text.contains("Still trying");
        if dead && sim.up() && !calm && (text.contains("server") || text.contains("OpenCode")) {
            self.fail(Kind::BlamesServer, t, format!("toast \"{title}\" / \"{desc}\" while the link was dead and the server up"));
        }
    }

    /// Conditions that may hold for a moment but not once the link has been good a while.
    fn stuck(&mut self, t: f64, s: &Screen, sim: &dyn Truth, link: Health, words: &HashMap<String, String>) {
        let mut now: Vec<(Kind, String, String)> = vec![];
        let name = |id: &str| label(words, id);
        for id in &s.sending {
            let has = if sim.has(id) { "the server has it" } else { "the server never got it, so it should say it failed" };
            now.push((Kind::StuckSending, id.clone(), format!("{} still says Not confirmed; {has}", name(id))));
        }
        for id in &s.pending {
            if sim.delivered(id) && !s.sending.contains(id) {
                now.push((Kind::StuckPending, id.clone(), format!("{} still says Pending; the server delivered it", name(id))));
            }
        }
        for id in &s.failed {
            if sim.has(id) {
                now.push((Kind::RolledBack, id.clone(), format!("{} says Failed to send, but the server has it", name(id))));
            }
        }
        for id in s.prompts.iter().chain(&s.queued) {
            if !sim.has(id) && !s.sending.contains(id) && !s.failed.contains(id) {
                now.push((Kind::Phantom, id.clone(), format!("{} shows, but the server doesn't have it", name(id))));
            }
        }
        let inbox = sim.inbox();
        for id in sim.users().iter().chain(inbox.iter().map(|i| &i.0)) {
            if !s.prompts.contains(id) && !s.queued.contains(id) {
                now.push((Kind::Missing, id.clone(), format!("the server has {} but it doesn't show", name(id))));
            }
        }
        if let Some((_, title, _)) = &s.toast
            && title.contains("Still trying")
        {
            now.push((Kind::StuckToast, String::new(), format!("the toast \"{title}\" stays up")));
        }
        let shows_stop = if s.composer_empty && !s.button.is_empty() { s.button == "Stop" } else { s.busy };
        if shows_stop != sim.busy() {
            let what = if shows_stop { "Stop shows, but the server is idle" } else { "Send shows, but the server is running" };
            now.push((Kind::BusyMismatch, String::new(), what.to_owned()));
        }
        let keys: HashSet<(Kind, String)> = now.iter().map(|(k, id, _)| (*k, id.clone())).collect();
        self.since.retain(|k, _| keys.contains(k));
        let good = link.good_since;
        for (kind, id, detail) in now {
            let since = *self.since.entry((kind, id)).or_insert(t);
            if let Some(g) = good
                && t - since.max(g) >= GRACE
            {
                self.fail(kind, t, format!("{detail} ({GRACE:.0} s on a good link)"));
            }
        }
    }

    /// The end state, once the link has been good and the server idle a while.
    pub fn settled(&mut self, t: f64, s: &Screen, sim: &dyn Truth) {
        let words = names(s, sim);
        let show = |ids: &[String]| ids.iter().map(|id| label(&words, id)).collect::<Vec<_>>().join(" ");
        let mut want = sim.users();
        want.extend(sim.waiting(false));
        // A prompt marked Failed that the server never got is right to show; it isn't the server's.
        let prompts: Vec<String> = s.prompts.iter().filter(|id| !(s.failed.contains(*id) && !sim.has(id))).cloned().collect();
        if prompts != want {
            self.fail(Kind::SettledWrong, t, format!("screen shows [{}], server has [{}]", show(&prompts), show(&want)));
        }
        let queued = sim.waiting(true);
        if s.queued != queued {
            self.fail(Kind::SettledWrong, t, format!("queue shows [{}], server has [{}]", show(&s.queued), show(&queued)));
        }
        let mut wrong = vec![];
        if s.busy || s.button == "Stop" {
            wrong.push("Stop shows");
        }
        if s.working {
            wrong.push("Working shows");
        }
        if !s.sending.is_empty() {
            wrong.push("a prompt says Not confirmed");
        }
        if s.stopping {
            wrong.push("a stop is still in progress");
        }
        if s.toast.as_ref().is_some_and(|x| x.1.contains("Still trying")) {
            wrong.push("\"Still trying\" is up");
        }
        if !wrong.is_empty() {
            self.fail(Kind::SettledWrong, t, format!("settled and idle, but {}", wrong.join(", ")));
        }
    }

    pub fn never_settled(&mut self, t: f64, why: String) {
        self.fail(Kind::NeverSettled, t, why);
    }

    pub fn intent(&mut self, kind: Kind, t: f64, detail: String) {
        self.fail(kind, t, detail);
    }
}

fn names(s: &Screen, sim: &dyn Truth) -> HashMap<String, String> {
    let mut words = sim.words();
    for (id, w) in &s.words {
        words.entry(id.clone()).or_insert_with(|| w.clone());
    }
    words
}
