//! Desktop host adapter for `opencode-feature-dictation` (`features/dictation`).
//!
//! Hex voice dictation drives a local desktop process via `hex-client` and a filesystem watcher,
//! neither of which exists in a browser; on `wasm32` the feature crate is omitted and the host
//! hooks are no-ops.

#[cfg(not(target_arch = "wasm32"))]
pub use opencode_feature_dictation::*;

#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;

#[cfg(not(target_arch = "wasm32"))]
use opencode_kernel::{Feature, KernelBuilder, Slot};
#[cfg(not(target_arch = "wasm32"))]
use rune::{KeyEvent, TextEdit, Tx, Waker};

use crate::App;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) static DICTATION: Slot<Dictation> = Slot::new();

/// Whether a Hex dictation session is currently active (always `false` on `wasm32`).
pub(crate) fn is_active(s: &App) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        s.kernel.slots.get(&DICTATION).active()
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = s;
        false
    }
}

#[cfg(target_arch = "wasm32")]
pub fn toolbar(_: &App, _: &crate::palette::Pal) -> Option<rune::El> {
    None
}

#[cfg(target_arch = "wasm32")]
pub fn status(_: &App, _: &crate::palette::Pal) -> Option<rune::El> {
    None
}

#[cfg(target_arch = "wasm32")]
pub fn key(_: &mut App, _: &rune::KeyEvent, _: &mut rune::Tx) -> bool {
    false
}

#[cfg(target_arch = "wasm32")]
pub fn key_up(_: &mut App, _: &rune::KeyEvent, _: &mut rune::Tx) -> bool {
    false
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn submit_mode(_: &mut App, _: crate::send::SubmitMode) -> bool {
    false
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn submit_mode(s: &mut App, mode: crate::send::SubmitMode) -> bool {
    let kind = match mode {
        crate::send::SubmitMode::Default => SubmitKind::Default,
        crate::send::SubmitMode::Alternate => SubmitKind::Alternate,
        crate::send::SubmitMode::QueueFirst => SubmitKind::QueueFirst,
    };
    submit(s, kind)
}

#[cfg(not(target_arch = "wasm32"))]
impl DictationHost for App {
    fn dictation(&self) -> &Dictation {
        self.kernel.slots.get(&DICTATION)
    }

    fn dictation_mut(&mut self) -> &mut Dictation {
        self.kernel.slots.get_mut(&DICTATION)
    }

    fn composer_mut(&mut self) -> &mut TextEdit {
        &mut self.composer
    }

    fn is_dictate_bind(&self, ev: &KeyEvent) -> bool {
        crate::keymap::Bind::Dictate.matches(ev)
    }

    fn dictate_keycaps(&self) -> Vec<String> {
        crate::keymap::Bind::Dictate.keycaps()
    }

    fn hex_support_dir(&self) -> Option<PathBuf> {
        crate::dirs::hex_support()
    }

    fn dictation_waker(&self) -> Option<Waker> {
        self.launch.inbox_wake.clone()
    }

    fn submit_prompt(&mut self, kind: SubmitKind, tx: &mut Tx) {
        let mode = match kind {
            SubmitKind::Default => crate::send::SubmitMode::Default,
            SubmitKind::Alternate => crate::send::SubmitMode::Alternate,
            SubmitKind::QueueFirst => crate::send::SubmitMode::QueueFirst,
        };
        App::submit_mode(self, mode, tx);
    }
}

/// Kernel feature registration for Hex voice dictation.
#[cfg(not(target_arch = "wasm32"))]
pub struct DictationFeature;

#[cfg(not(target_arch = "wasm32"))]
impl Feature<App> for DictationFeature {
    fn id(&self) -> &'static str {
        "feature.dictation"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder.register_slot(&DICTATION);
        // Precaution: dictation can call `App::submit`, mutating `route`/`tabs`/`sessions`/`projects`
        // read by `projects.home` and `project_switcher.poll`.
        builder
            .schedule(crate::tick::stage::INGEST_SERVERS, "dictation.poll", |s, now, tx| {
                poll(s, now, tx)
            })
            .after("project_switcher.poll");
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod stories {
    //! The storybook's "Composer › Dictation", on the fake Hex.

    use super::{Dictation, Heard, Problem, cancel, start, stop};
    use crate::App;
    use crate::stories::{Story, Trigger, replay};

    const STORY_KEYS: &[Trigger] = &[
        replay(story_open),
        Trigger {
            key: 's',
            label: "Start",
            run: |s, tx| {
                tx.instant();
                s.kernel.slots.get_mut(&super::DICTATION).set_fake_prepare(0.0);
                start(s);
            },
        },
        Trigger {
            key: 'p',
            label: "Slow start",
            run: |s, tx| {
                tx.instant();
                cancel(s);
                s.kernel.slots.get_mut(&super::DICTATION).slow_fake();
                start(s);
            },
        },
        Trigger {
            key: 'w',
            label: "Words so far",
            run: |s, tx| {
                tx.instant();
                if !s.kernel.slots.get(&super::DICTATION).recording() {
                    s.kernel.slots.get_mut(&super::DICTATION).set_fake_prepare(0.0);
                    start(s);
                }
                s.kernel.slots.get_mut(&super::DICTATION).push_fake_heard([
                    Heard::Ready,
                    Heard::Partial {
                        stable: "keep the retry budget as it is".into(),
                        tentative: " and add a".into(),
                    },
                ]);
            },
        },
        Trigger {
            key: 'f',
            label: "Finish",
            run: |s, tx| {
                tx.instant();
                stop(s);
            },
        },
        Trigger {
            key: 'n',
            label: "Finish, words never come",
            run: |s, tx| {
                tx.instant();
                s.kernel.slots.get_mut(&super::DICTATION).set_fake_stall(true);
                stop(s);
            },
        },
        Trigger {
            key: 'e',
            label: "Next error",
            run: |s, tx| {
                tx.instant();
                let next = match s.kernel.slots.get(&super::DICTATION).error() {
                    Some(hex_client::NOT_RUNNING) => hex_client::BUSY,
                    Some(hex_client::BUSY) => hex_client::OUTDATED,
                    Some(hex_client::OUTDATED) => hex_client::NOT_INSTALLED,
                    Some(hex_client::NOT_INSTALLED) => hex_client::MIC_BLOCKED,
                    _ => hex_client::NOT_RUNNING,
                };
                cancel(s);
                s.kernel.slots.get_mut(&super::DICTATION).fail(Problem::of(next));
            },
        },
    ];

    /// A draft with the caret after it, then the loop iOS shows: the stop while it listens, the
    /// spinner while the words come, the words at the caret.
    fn story_open(s: &mut App, tx: &mut rune::Play<App>) {
        *s.kernel.slots.get_mut(&super::DICTATION) = Dictation::fake();
        *s.composer = rune::TextEdit::new("Looks right. Before you merge,");
        start(s);
        tx.after(1.6, |s, tx| {
            tx.instant();
            stop(s);
        });
    }

    rune_register::register!(crate::stories::STORY_LIST: Story = [
        Story::composer("Composer/Dictation", STORY_KEYS),
    ]);
}
