//! The storybook's Quote cards story (Composer › Quote cards): the same quotes as chips in the
//! composer and as cards in the prompt that sent them. Variants: one quote, three (one with a
//! comment over a quote of two paragraphs), a long comment on a long quote, and quotes cut
//! mid-word. P lands the prompt again, R replays.
//! `--playground 'Composer/Quote cards' --variant Three --snap out.png`.

use opencode_kit::model::{PromptComment, Quote};
use rune::{Play, TextEdit, Tx};
use serde_json::json;

use crate::App;
use crate::stories::{Story, Trigger, emit, replay, sid};

const KEYS: &[Trigger] = &[replay(open), Trigger { key: 'p', label: "The prompt lands", run: |s, tx| prompt(s, tx) }];

const VARIANTS: &[(&str, usize)] = &[("One", 0), ("Three", 1), ("Long", 2), ("Cut mid-word", 3)];

const ASK: &str = "Is this true? Does this really matter?";

fn quote(text: &str, comment: Option<&str>) -> PromptComment {
    Quote { text: text.into(), comment: comment.map(str::to_owned) }.into()
}

fn quotes(variant: usize) -> Vec<PromptComment> {
    match variant {
        1 => vec![
            quote("Believe escapability shouldn't be surfaced as a first-class property of types", None),
            quote(
                "Niko Matsakis, Rust's borrow-checker lead (2024):\n\n\"'a no longer represents a lifetime but rather an origin\"",
                Some("Also, where did he say this? What should we take from it?"),
            ),
            quote("Rust's borrow checker", None),
        ],
        2 => vec![quote(
            "The primary problem is the steady march toward a full Rust-like lifetime system, which is where you end up as soon as you start returning (instead of yielding) notional references from one thing to another.\n\nReferences become second-class citizens: they are only created implicitly, at function boundaries, and cannot be stored in variables or object fields.",
            Some("This is the part I keep coming back to.\nIf references can't be stored, how do we model a cursor into an arena without an index type everywhere? Spell out the trade-off before we commit."),
        )],
        3 => vec![
            quote("elieve escapability shouldn't be surfaced as a first-class property of types", None),
            quote("iko Matsakis, Rust's borrow-checker lead (2024):\n\n\"'a no longer represents a lifetime but rather an orig", Some("Where did he say this?")),
            quote("akis, Rust's borrow", None),
        ],
        _ => vec![quote("The sidebar reconnects on wake, but it drops the first heartbeat after the socket comes back", None)],
    }
}

fn open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    for q in quotes(*s.fixture_run.variant) {
        s.composer_ext.quote.push(q);
    }
    *s.composer = TextEdit::new(ASK);
    prompt(s, tx);
}

/// The sent prompt, carrying the same quotes.
fn prompt(s: &mut App, tx: &mut Tx) {
    let n = s.transcripts.get(&sid(s)).map_or(0, |t| t.messages.len());
    let id = format!("msg_zzzzquotecards{n:04}");
    opencode_kit::model::set_user_comments(&id, quotes(*s.fixture_run.variant));
    emit(s, "session.prompted", json!({ "messageID": id, "text": ASK }), tx);
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story::session("Composer/Quote cards", KEYS).variants(VARIANTS),
]);
