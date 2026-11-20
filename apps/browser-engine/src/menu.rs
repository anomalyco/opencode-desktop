//! CEF `ContextMenuHandler` replacing Chromium's default popup menu with a policy-checked
//! descriptor sent to the host (`packages/gui-plugins/src/browser/{page-menu,native-page-menu}.ts`).
//!
//! Security invariants:
//! - Chromium's built-in `MenuModel` is cleared (`model.clear()`) and `run_context_menu` always
//!   returns `1`, so unmanaged DevTools / View Source / Print / Save-as entries never appear.
//! - Link and image URLs are checked against `browser_policy` before reaching the menu or
//!   executing `SaveImage`.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use cef::rc::Rc as _;
use cef::{
    Browser, BrowserHost, CefString, CefStringList, ContextMenuHandler, ContextMenuParams, Frame,
    ImplBrowser, ImplBrowserHost, ImplContextMenuHandler, ImplContextMenuParams, ImplFrame,
    ImplMenuModel, ImplRunContextMenuCallback, MenuModel, RunContextMenuCallback,
    WrapContextMenuHandler, sys, wrap_context_menu_handler,
};

use crate::output::Output;
use crate::prompts::PromptShared;
use crate::wire::{ContextMenuCommand, EngineMsg, PageContextMenu};

/// Reads strings out of a borrowed CEF string list without taking ownership.
pub fn read_cef_string_list(list: &CefStringList) -> Vec<String> {
    let raw: *const sys::_cef_string_list_t = list.into();
    if raw.is_null() {
        return Vec::new();
    }
    // SAFETY: `list` is a live CEF string list borrowed for the duration of the callback.
    unsafe {
        let count = sys::cef_string_list_size(raw as *mut _);
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let mut val: sys::cef_string_t = std::mem::zeroed();
            if sys::cef_string_list_value(raw as *mut _, i, &mut val) > 0 {
                let s = CefString::from(std::ptr::from_ref(&val)).to_string();
                sys::cef_string_utf16_clear(&mut val);
                if !s.is_empty() {
                    out.push(s);
                }
            }
        }
        out
    }
}

struct HeldMenu {
    id: u64,
    callback: RunContextMenuCallback,
}

/// Per-tab held CEF context menu callback (at most one active at a time).
#[derive(Default)]
pub struct TabMenuState {
    held: Mutex<Option<HeldMenu>>,
}

impl TabMenuState {
    pub fn cancel_all(&self) {
        if let Some(prev) = self.held.lock().unwrap().take() {
            prev.callback.cancel();
        }
    }

    pub fn execute(
        &self,
        id: u64,
        action: Option<ContextMenuCommand>,
        browser: &Browser,
        host: &BrowserHost,
        file_roots: &[String],
        is_preview: bool,
    ) {
        let matched = {
            let mut slot = self.held.lock().unwrap();
            if slot.as_ref().is_some_and(|h| h.id == id) {
                slot.take()
            } else {
                None
            }
        };
        if let Some(held) = matched {
            // Always cancel CEF's native menu runner; we execute the chosen action directly.
            held.callback.cancel();
        } else {
            return;
        }
        let Some(cmd) = action else { return };
        match cmd {
            ContextMenuCommand::Back => browser.go_back(),
            ContextMenuCommand::Forward => browser.go_forward(),
            ContextMenuCommand::Reload => browser.reload(),
            ContextMenuCommand::HardReload => browser.reload_ignore_cache(),
            ContextMenuCommand::Cut => {
                if let Some(f) = browser.focused_frame().or_else(|| browser.main_frame()) {
                    f.cut();
                }
            }
            ContextMenuCommand::Copy => {
                if let Some(f) = browser.focused_frame().or_else(|| browser.main_frame()) {
                    f.copy();
                }
            }
            ContextMenuCommand::Paste => {
                if let Some(f) = browser.focused_frame().or_else(|| browser.main_frame()) {
                    f.paste();
                }
            }
            ContextMenuCommand::PastePlain => {
                if let Some(f) = browser.focused_frame().or_else(|| browser.main_frame()) {
                    f.paste_and_match_style();
                }
            }
            ContextMenuCommand::SelectAll => {
                if let Some(f) = browser.focused_frame().or_else(|| browser.main_frame()) {
                    f.select_all();
                }
            }
            ContextMenuCommand::SaveImage { url } => {
                if !is_preview
                    && (browser_policy::destination_origin(&url).is_some()
                        || browser_policy::file_url_within(&url, file_roots))
                {
                    host.start_download(Some(&CefString::from(url.as_str())));
                }
            }
            ContextMenuCommand::ReplaceMisspelling { word } => {
                host.replace_misspelling(Some(&CefString::from(word.as_str())));
            }
            ContextMenuCommand::AddToDictionary { word } => {
                host.add_word_to_dictionary(Some(&CefString::from(word.as_str())));
            }
        }
    }
}

fn allowed_menu_url(
    raw: &str,
    roots: &[String],
    preview: Option<&Arc<crate::preview::PreviewGrant>>,
) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > crate::prompts::MAX_POPUP_URL_BYTES {
        return None;
    }
    if let Some(grant) = preview {
        if let Some(display) = grant.compute_display_url(trimmed) {
            return Some(display);
        }
        return browser_policy::destination_origin(trimmed)
            .is_some()
            .then(|| trimmed.to_owned());
    }
    if browser_policy::destination_origin(trimmed).is_some()
        || browser_policy::file_url_within(trimmed, roots)
    {
        Some(trimmed.to_owned())
    } else {
        None
    }
}

wrap_context_menu_handler! {
    pub struct TabContextMenuHandler {
        tab_id: u64,
        shared: PromptShared,
        file_roots: Arc<Mutex<Vec<String>>>,
        preview: Option<Arc<crate::preview::PreviewGrant>>,
        menu_state: Arc<TabMenuState>,
        out_tx: Output,
    }

    impl ContextMenuHandler {
        fn on_before_context_menu(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _params: Option<&mut ContextMenuParams>,
            model: Option<&mut MenuModel>,
        ) {
            // Strip Chromium's default menu (DevTools, View Source, Print, etc.), keeping one
            // sentinel entry so CEF invokes `run_context_menu` instead of suppressing it.
            if let Some(model) = model {
                model.clear();
                model.add_item(1, Some(&CefString::from("menu")));
            }
        }

        fn run_context_menu(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            params: Option<&mut ContextMenuParams>,
            _model: Option<&mut MenuModel>,
            callback: Option<&mut RunContextMenuCallback>,
        ) -> ::std::os::raw::c_int {
            let Some(callback) = callback else { return 1 };
            if !self.shared.enabled.load(Ordering::Relaxed) {
                callback.cancel();
                return 1;
            }
            let Some(params) = params else {
                callback.cancel();
                return 1;
            };
            let roots = self.file_roots.lock().unwrap().clone();
            let raw_link = CefString::from(&params.link_url()).to_string();
            let raw_src = CefString::from(&params.source_url()).to_string();
            let raw_page = CefString::from(&params.page_url()).to_string();
            let raw_frame = CefString::from(&params.frame_url()).to_string();
            let page_url = self.preview.as_ref().map_or(raw_page.clone(), |g| g.sanitize_reported_url(&raw_page));
            let frame_url = self.preview.as_ref().map_or(raw_frame.clone(), |g| g.sanitize_reported_url(&raw_frame));
            let selection_text = CefString::from(&params.selection_text()).to_string();
            let misspelled_raw = CefString::from(&params.misspelled_word()).to_string();
            let mut sugg_list = CefStringList::new();
            let suggestions = if !misspelled_raw.is_empty()
                && params.dictionary_suggestions(Some(&mut sugg_list)) != 0
            {
                sugg_list.into_iter().take(5).collect()
            } else {
                Vec::new()
            };
            let edit = params.edit_state_flags().as_ref().0 as u32;
            let is_image = *params.media_type().as_ref()
                == sys::cef_context_menu_media_type_t::CM_MEDIATYPE_IMAGE;
            let id = self.shared.next_id();
            let menu = PageContextMenu {
                id,
                x: params.xcoord(),
                y: params.ycoord(),
                page_url,
                frame_url,
                link_url: allowed_menu_url(&raw_link, &roots, self.preview.as_ref()),
                // Never expose `src_url` / `SaveImage` on preview tabs (`notes/review-rd281.md` #5).
                src_url: if is_image && self.preview.is_none() {
                    allowed_menu_url(&raw_src, &roots, None)
                } else {
                    None
                },
                is_image,
                has_image_contents: params.has_image_contents() != 0,
                is_editable: params.is_editable() != 0,
                selection_text: selection_text.chars().take(512).collect(),
                can_cut: (edit & sys::cef_context_menu_edit_state_flags_t::CM_EDITFLAG_CAN_CUT.0 as u32) != 0,
                can_copy: (edit & sys::cef_context_menu_edit_state_flags_t::CM_EDITFLAG_CAN_COPY.0 as u32) != 0,
                can_paste: (edit & sys::cef_context_menu_edit_state_flags_t::CM_EDITFLAG_CAN_PASTE.0 as u32) != 0,
                can_select_all: (edit & sys::cef_context_menu_edit_state_flags_t::CM_EDITFLAG_CAN_SELECT_ALL.0 as u32) != 0,
                misspelled_word: (!misspelled_raw.is_empty()).then_some(misspelled_raw),
                suggestions,
            };
            if let Some(prev) = self.menu_state.held.lock().unwrap().replace(HeldMenu {
                id,
                callback: callback.clone(),
            }) {
                prev.callback.cancel();
            }
            let _ = self.out_tx.send(EngineMsg::ContextMenu { tab_id: self.tab_id, menu });
            1
        }
    }
}
