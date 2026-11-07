//! Remote access as Desktop's main process changes it (`main-machine.ts`): the background
//! service's setting, read and written through `opencode service`, one command at a time. A
//! change restarts the service, so its `set` and `start` run as one job, and no read lands
//! between them; a change that already matches is left alone unless asked to restart.

use std::collections::VecDeque;

use opencode_plugin::prelude::*;

use crate::Pairing;

/// Ceiling for `opencode service get remote`.
const READ_TIMEOUT_MS: u64 = 20_000;
/// `opencode service set remote`, which creates the tunnel the first time it turns on.
const SET_TIMEOUT_MS: u64 = 180_000;
/// `opencode service start`, after a change.
const START_TIMEOUT_MS: u64 = 60_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Job {
    Read { attempt: u64 },
    Set { attempt: u64, enabled: bool, restart: bool },
}

pub enum Done {
    Read { attempt: u64, enabled: Option<bool> },
    Set { attempt: u64, ok: bool },
}

#[derive(Default)]
pub struct Queue {
    pending: VecDeque<Job>,
    running: bool,
}

/// `opencode service <args>`, its window hidden, killed past `timeout`.
fn command(args: &[&str], timeout: u64) -> ProcessSpec {
    ProcessSpec::new("opencode").arg("service").args(args.iter().copied()).timeout(timeout)
}

impl Queue {
    pub fn push(&mut self, cx: &mut ActionCx, job: Job) {
        self.pending.push_back(job);
        if !self.running {
            self.next(cx);
        }
    }

    /// Starts the next job, once none runs.
    pub fn next(&mut self, cx: &mut ActionCx) {
        self.running = false;
        let Some(job) = self.pending.pop_front() else { return };
        self.running = true;
        match job {
            Job::Read { attempt } => read(cx, move |s, cx, enabled| s.service_done(cx, Done::Read { attempt, enabled })),
            // The setting is read first, so a switch that already matches restarts nothing.
            Job::Set { attempt, enabled, restart: true } => set(cx, attempt, enabled),
            Job::Set { attempt, enabled, restart: false } => read(cx, move |s, cx, now| {
                if now == Some(enabled) {
                    s.service_done(cx, Done::Set { attempt, ok: true });
                } else {
                    set(cx, attempt, enabled);
                }
            }),
        }
    }
}

fn read(cx: &mut ActionCx, done: impl FnOnce(&mut Pairing, &mut ActionCx, Option<bool>) + 'static) {
    let spec = command(&["get", "remote"], READ_TIMEOUT_MS);
    let started = cx.run_process::<Pairing>(&spec, move |s, cx, out| {
        let enabled = out.ok().then(|| out.stdout.trim() == "true");
        if !out.ok() {
            opencode_plugin::system::log(true, &format!("pairing service command failed: get remote: {}", out.stderr.trim()));
        }
        done(s, cx, enabled);
    });
    if started.is_none() {
        opencode_plugin::system::log(true, "pairing service command failed: get remote: refused");
    }
}

/// Sets the setting, then starts the service again (with the tunnel when it's on).
fn set(cx: &mut ActionCx, attempt: u64, enabled: bool) {
    let spec = command(&["set", "remote", if enabled { "true" } else { "false" }], SET_TIMEOUT_MS);
    cx.run_process::<Pairing>(&spec, move |s, cx, out| {
        if !out.ok() {
            opencode_plugin::system::log(true, &format!("pairing service command failed: set remote: {}", out.stderr.trim()));
            return s.service_done(cx, Done::Set { attempt, ok: false });
        }
        let start = command(&["start"], START_TIMEOUT_MS);
        cx.run_process::<Pairing>(&start, move |s, cx, out| {
            if !out.ok() {
                opencode_plugin::system::log(true, &format!("pairing service command failed: start: {}", out.stderr.trim()));
            }
            s.service_done(cx, Done::Set { attempt, ok: out.ok() });
        });
    });
}
