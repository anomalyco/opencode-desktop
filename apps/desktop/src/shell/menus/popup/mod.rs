//! Desktop host adapter for [`opencode_ui::popup`].

pub mod context;

use opencode_kit::servers::health::Indicator;
use rune::El;
use rune_knobs::Knobs;
use session_view::palette::Pal;

use crate::App;

pub type Popup = opencode_ui::popup::Popup<App>;

#[allow(unused_imports)]
pub use opencode_ui::popup::{
    Align, Item, Menu, MenuButton, NATIVE, PRESS_ON_DOWN, PopupAnchor, PopupItem, PopupMenu, Side,
    dismiss, key, menu_button, open, open_drawn, open_searchable, poll, view,
};

/// `menu` as a drawn surface, row `active` lit (specialized to [`App`] for storybook previews).
#[allow(dead_code)]
pub fn surface(p: &Pal, menu: &Menu, active: Option<usize>) -> El {
    opencode_ui::popup::surface::<App>(p, menu, active)
}

impl opencode_ui::popup::PopupHost for App {
    fn popup(&self) -> &Popup {
        &self.popup
    }

    fn popup_mut(&mut self) -> &mut Popup {
        &mut self.popup
    }

    fn knobs(&self) -> &Knobs {
        &self.knobs
    }

    fn clock(&self) -> f64 {
        *self.clock
    }

    fn on_focus_taken(&mut self) {
        self.composer.focused = false;
    }

    fn max_width(&self) -> Option<f32> {
        self.narrow().then(|| (self.layout.narrow.width() - 8.0).max(1.0))
    }

    fn project_badge(&self, p: &Pal, dir: &str) -> Option<El> {
        let project = self.project_at(dir);
        Some(crate::icons::badge(self, p, project, &crate::view::project_or_dir(project, dir)))
    }

    fn status_indicator(&self, p: &Pal, indicator: Indicator) -> El {
        crate::servers::indicator::mark(p, indicator)
    }

    fn hairline(&self) -> f32 {
        crate::view::chrome().hairline
    }
}
