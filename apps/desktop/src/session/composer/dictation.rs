//! Desktop host adapter for `opencode-feature-dictation` (`features/dictation`).

pub use opencode_feature_dictation::*;

use std::path::PathBuf;

use opencode_kernel::{Feature, KernelBuilder, Phase};
use rune::{KeyEvent, TextEdit, Tx, Waker};

use crate::App;

impl DictationHost for App {
    fn dictation(&self) -> &Dictation {
        &self.dictation
    }

    fn dictation_mut(&mut self) -> &mut Dictation {
        &mut self.dictation
    }

    fn composer_mut(&mut self) -> &mut TextEdit {
        &mut self.composer
    }

    fn clock(&self) -> f64 {
        *self.clock
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

    fn is_headless(&self) -> bool {
        crate::headless()
    }

    fn waker(&self) -> Option<Waker> {
        self.launch.inbox_wake.clone()
    }

    fn submit_prompt(&mut self, alternate: bool, tx: &mut Tx) {
        App::submit(self, alternate, tx);
    }
}

/// Kernel feature registration for Hex voice dictation.
pub struct DictationFeature;

impl Feature<App> for DictationFeature {
    fn id(&self) -> &'static str {
        "feature.dictation"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder.add_system(Phase::Ingest, "dictation.poll", |s, now, tx| poll(s, now, tx));
    }
}

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
                s.dictation.set_fake_prepare(0.0);
                start(s);
            },
        },
        Trigger {
            key: 'p',
            label: "Slow start",
            run: |s, tx| {
                tx.instant();
                cancel(s);
                s.dictation.slow_fake();
                start(s);
            },
        },
        Trigger {
            key: 'w',
            label: "Words so far",
            run: |s, tx| {
                tx.instant();
                if !s.dictation.recording() {
                    s.dictation.set_fake_prepare(0.0);
                    start(s);
                }
                s.dictation.push_fake_heard([
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
                s.dictation.set_fake_stall(true);
                stop(s);
            },
        },
        Trigger {
            key: 'e',
            label: "Next error",
            run: |s, tx| {
                tx.instant();
                let next = match s.dictation.error() {
                    Some(hex_client::NOT_RUNNING) => hex_client::BUSY,
                    Some(hex_client::BUSY) => hex_client::OUTDATED,
                    Some(hex_client::OUTDATED) => hex_client::NOT_INSTALLED,
                    Some(hex_client::NOT_INSTALLED) => hex_client::MIC_BLOCKED,
                    _ => hex_client::NOT_RUNNING,
                };
                cancel(s);
                s.dictation.fail(Problem::of(next));
            },
        },
    ];

    /// A draft with the caret after it, then the loop iOS shows: the stop while it listens, the
    /// spinner while the words come, the words at the caret.
    fn story_open(s: &mut App, tx: &mut rune::Play<App>) {
        *s.dictation = Dictation::fake();
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
