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

/// Change `index` of script `script`'s id: its card's call and disclosure key. A key holds no
/// `/`, so the path stays out of it.
pub fn change_id(script: &str, index: usize) -> String {
    format!("{script}-file-{index}")
}

/// The diff change `file` shows, as `changedFileDiff` reads it: a patch with lines in it.
fn diff(file: &Value) -> Option<Value> {
    let patch = file["patch"].as_str()?;
    let (additions, deletions) = (file["additions"].as_u64().unwrap_or(0), file["deletions"].as_u64().unwrap_or(0));
    (additions + deletions > 0).then(|| {
        let status = file["status"].as_str().unwrap_or("modified");
        json!({ "files": [{ "file": file["file"], "patch": patch, "additions": additions, "deletions": deletions, "status": status }] })
    })
}

/// Whether change `file` draws as a card rather than a row of counts.
pub fn has_diff(file: &Value) -> bool {
    diff(file).is_some()
}

/// Between two cards, and between a script's changes and the files edited beside them:
/// `--timeline-gap-s` (`timeline-next.css`).
pub const CARD_GAP: f32 = 16.0;

/// A script's changes, `(script, index, file)` in order. Rows of counts sit flush with each other;
/// a card stands [`CARD_GAP`] from whatever is beside it, card or row, as any two items do (`timeline-next.css`'s
/// `[data-script-change="patch"] + [data-script-change="patch"]`).
pub fn changes<S: TimelineHost>(s: &S, p: &Pal, cx: Ctx, list: &[(&str, usize, &Value)]) -> El {
    let mut out = col().key("script-changes").w_full();
    // Whether the change above drew a card; `None` above the first.
    let mut above: Option<bool> = None;
    for &(script, index, file) in list {
        let id = change_id(script, index);
        let card = has_diff(file);
        let item = match diff(file) {
            Some(metadata) => {
                let call = Call { id: &id, name: "patch", status: &ToolStatus::Completed, input: &Value::Null, metadata: &metadata };
                let drawn = super::cached(s, p, cx, &[call], || super::file_group(s, p, cx, &[call]));
                col().key(format!("change-{id}")).w_full().child(drawn)
            }
            None => super::files::counts_row(s, p, cx, format!("change-{id}"), file),
        };
        // Only two rows of counts in a row sit flush.
        let gap = above.is_some_and(|a| a || card);
        out = out.child(if gap { item.mt(CARD_GAP) } else { item });
        above = Some(card);
    }
    out
}
