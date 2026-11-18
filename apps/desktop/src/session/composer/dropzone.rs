//! Desktop host adapter for `opencode-feature-composer-input::dropzone`
//! (`features/composer-input`).

pub use opencode_feature_composer_input::dropzone::*;

use std::path::{Path, PathBuf};

use rune::{El, Tx};
use session_view::palette::Pal;
use session_view::pickers::PickerHost as _;

use crate::App;

impl DropzoneHost for App {
    fn dropzone(&self) -> Dropzone {
        self.attach.drop
    }

    fn dropzone_mut(&mut self) -> &mut Dropzone {
        &mut self.attach.drop
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

pub fn drop(s: &mut App, paths: &[PathBuf], tx: &mut Tx) {
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
        self.pickers.attachment_input(&self.selection().model)
    }
}
