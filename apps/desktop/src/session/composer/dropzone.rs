//! Desktop host adapter for `opencode-feature-composer-input::dropzone`
//! (`features/composer-input`).

pub use opencode_feature_composer_input::dropzone::*;

use std::path::{Path, PathBuf};

use rune::{El, Tx};
use session_view::composer::ComposerHost as _;
use session_view::palette::Pal;
use session_view::pickers::PickerHost as _;

use crate::App;

impl DropzoneHost for App {
    fn dropzone(&self) -> Dropzone {
        self.composer_ext.attach.drop
    }

    fn dropzone_mut(&mut self) -> &mut Dropzone {
        &mut self.composer_ext.attach.drop
    }

    fn attachment_input(&self) -> AttachmentInput {
        App::attachment_input(self)
    }

    fn attach_path(&mut self, path: &Path) {
        App::attach_path(self, path);
    }
}

pub fn hover(s: &mut App, over: bool, tx: &mut Tx) {
    opencode_feature_composer_input::dropzone::hover(s, over, tx);
}

fn take_dragged_tree_file(s: &App, path: &Path) -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        s.kernel.slots.get(&crate::open_in::OPEN_IN).take_dragged_rel(path)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (s, path);
        None
    }
}

pub fn drop(s: &mut App, paths: &[PathBuf], tx: &mut Tx) {
    // A workspace file dragged from the Files/Review tree drops as an inline `@<path>` mention pill
    // and focuses the composer (`packages/app/src/composer/attachments/attachments.ts:217-223`).
    if paths.len() == 1
        && let Some(rel) = take_dragged_tree_file(s, &paths[0])
    {
        opencode_feature_composer_input::dropzone::drop(s, &[], tx);
        let shown = crate::mentions::shown_path(&rel);
        let caret = s.composer.focus.min(s.composer.text.len());
        s.composer.replace_with_pill(caret..caret, format!("file:{shown}"), &format!("@{shown}"));
        s.composer.insert(" ");
        s.composer.focused = true;
        s.composer.mark_edited();
        s.composer_on_change(tx);
        tx.focus("prompt", false);
        tx.instant();
        return;
    }
    opencode_feature_composer_input::dropzone::drop(s, paths, tx);
}

pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    opencode_feature_composer_input::dropzone::poll(s, tx)
}

pub fn overlay(s: &App, p: &Pal, size: (f32, f32)) -> El {
    opencode_feature_composer_input::dropzone::overlay(s, p, size)
}

impl App {
    /// What the selected model reads natively besides text.
    pub fn attachment_input(&self) -> AttachmentInput {
        self.composer_ext.pickers.attachment_input(&self.selection().model)
    }
}
