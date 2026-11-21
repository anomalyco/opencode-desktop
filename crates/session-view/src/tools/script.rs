//! The changes a script made to files (`session-ui/src/tools/tool-renderer.tsx`'s
//! `ScriptChanges`, anomalyco/opencode#65002 and #65247): one card each, in order, so the same file
//! changed twice is two cards. A change with a diff draws as a direct patch's file does, through
//! the same card ([`super::file_group`]); one without (past the size cap, a binary file, counts
//! alone) reads as a patch's file header does ([`super::files::counts_row`]). Each opens by the
//! change's own id, which only appends as the script runs, so an open card keeps its place.

use opencode_kit::model::ToolStatus;
use rune::{El, col};
use serde_json::{Value, json};

use super::{Call, Ctx};
use crate::TimelineHost;
use crate::palette::Pal;

/// The id of the change at offset `index` of script `script` in assistant message `msg`: its
/// card's call and disclosure key. Providers can reuse a call id across messages, so the message
/// is part of it. A key holds no `/`, so the path stays out of it.
pub fn change_id(msg: &str, script: &str, index: usize) -> String {
    format!("{msg}-{script}-file-{index}")
}

/// Whether change `file` draws as a card rather than a row of counts, as `changedFileDiff`
/// reads it: a patch with lines in it.
pub fn is_card(file: &Value) -> bool {
    use crate::activity::rules::count;
    file["patch"].is_string() && (count(&file["additions"]) > 0 || count(&file["deletions"]) > 0)
}

/// The one-file patch call's metadata change `file` draws through, built only when its card does.
fn payload(file: &Value) -> Value {
    use crate::activity::rules::count;
    let status = file["status"].as_str().unwrap_or("modified");
    json!({ "files": [{ "file": file["file"], "patch": file["patch"], "additions": count(&file["additions"]), "deletions": count(&file["deletions"]), "status": status }] })
}

/// Between two cards, and between a script's changes and the files edited beside them:
/// `--timeline-gap-s` (`timeline-next.css`).
pub const CARD_GAP: f32 = 16.0;

/// A script's changes, `(msg, script, index, file)` in order. Rows of counts sit flush with each other;
/// a card stands [`CARD_GAP`] from whatever is beside it, card or row, as any two items do (`timeline-next.css`'s
/// `[data-script-change="patch"] + [data-script-change="patch"]`).
pub fn changes<S: TimelineHost>(s: &S, p: &Pal, cx: Ctx, list: &[(&str, &str, usize, &Value)]) -> El {
    let mut out = col().key("script-changes").w_full();
    // Whether the change above drew a card; `None` above the first.
    let mut above: Option<bool> = None;
    for &(msg, script, index, file) in list {
        let id = change_id(msg, script, index);
        let card = is_card(file);
        let item = if card {
            // Keyed by the change as reported, so the payload and its patch copy are built only
            // when the card builds.
            let probe = Call { id: &id, name: "patch", status: &ToolStatus::Completed, input: &Value::Null, metadata: file };
            let drawn = super::cached(s, p, cx, &[probe], || {
                let metadata = payload(file);
                let call = Call { metadata: &metadata, ..probe };
                super::file_group(s, p, cx, &[call])
            });
            col().key(format!("change-{id}")).w_full().child(drawn)
        } else {
            super::files::counts_row(s, p, cx, format!("change-{id}"), file)
        };
        // Only two rows of counts in a row sit flush.
        let gap = above.is_some_and(|a| a || card);
        out = out.child(if gap { item.mt(CARD_GAP) } else { item });
        above = Some(card);
    }
    out
}
