//! A page's JavaScript dialogs (`alert`, `confirm`, `prompt`) and its `beforeunload`, held for the
//! app to show and answer, as Desktop's browser draws them in the tab instead of Electron's native
//! box (`gui-plugins/src/browser/page-prompts.ts`, `dialog-machine.ts`).
//!
//! Only for an app that said it shows them (`HostMsg::Hello { dialogs }`): an older app would never
//! answer, and the page would wait for ever. Without one, CEF keeps its own behaviour, which for a
//! windowless page suppresses the dialog.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cef::rc::Rc as _;
use cef::{
    Browser, CefString, ImplJsdialogCallback, ImplJsdialogHandler, JsdialogCallback, JsdialogHandler,
    JsdialogType, WrapJsdialogHandler, sys, wrap_jsdialog_handler,
};

use crate::output::Output;
use crate::wire::{DialogKind, EngineMsg, PageDialog};

/// Each held dialog and the callback that answers it, oldest first.
pub type Held = Arc<Mutex<Vec<(PageDialog, JsdialogCallback)>>>;

/// What every tab's handler shares: whether the app shows dialogs, and the next dialog id.
#[derive(Clone, Default)]
pub struct Shared {
    pub enabled: Arc<AtomicBool>,
    next: Arc<AtomicU64>,
}

wrap_jsdialog_handler! {
    pub struct TabDialogHandler {
        tab_id: u64,
        shared: Shared,
        held: Held,
        out_tx: Output,
    }

    impl JsdialogHandler {
        fn on_jsdialog(
            &self,
            _browser: Option<&mut Browser>,
            origin_url: Option<&CefString>,
            dialog_type: JsdialogType,
            message_text: Option<&CefString>,
            default_prompt_text: Option<&CefString>,
            callback: Option<&mut JsdialogCallback>,
            _suppress_message: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            use sys::cef_jsdialog_type_t as T;
            let kind = match *dialog_type.as_ref() {
                T::JSDIALOGTYPE_CONFIRM => DialogKind::Confirm,
                T::JSDIALOGTYPE_PROMPT => DialogKind::Prompt,
                _ => DialogKind::Alert,
            };
            let text = |s: Option<&CefString>| s.map(CefString::to_string).unwrap_or_default();
            self.hold(kind, text(origin_url), text(message_text), text(default_prompt_text), callback)
        }

        // Chromium shows no page text in "Leave site?"; neither does Desktop's.
        fn on_before_unload_dialog(
            &self,
            _browser: Option<&mut Browser>,
            _message_text: Option<&CefString>,
            _is_reload: ::std::os::raw::c_int,
            callback: Option<&mut JsdialogCallback>,
        ) -> ::std::os::raw::c_int {
            self.hold(DialogKind::Leave, String::new(), String::new(), String::new(), callback)
        }

        // A navigation or the page going away: Chromium cancels what waits, the callbacks go.
        fn on_reset_dialog_state(&self, _browser: Option<&mut Browser>) {
            self.forget();
        }

        // The page shows one dialog at a time: one that closed while still held was answered
        // another way (DevTools' `Page.handleJavaScriptDialog`), so the app's goes too.
        fn on_dialog_closed(&self, _browser: Option<&mut Browser>) {
            self.forget();
        }
    }
}

impl TabDialogHandler {
    fn forget(&self) {
        let had = !std::mem::take(&mut *self.held.lock().unwrap()).is_empty();
        if had {
            let _ = self.out_tx.send(EngineMsg::DialogsReset { tab_id: self.tab_id });
        }
    }

    /// Holds the dialog for the app and tells it; 0 (CEF's own handling) when the app doesn't show dialogs.
    fn hold(&self, kind: DialogKind, origin: String, message: String, default_text: String, callback: Option<&mut JsdialogCallback>) -> ::std::os::raw::c_int {
        let Some(callback) = callback else { return 0 };
        if !self.shared.enabled.load(Ordering::Relaxed) {
            return 0;
        }
        let id = self.shared.next.fetch_add(1, Ordering::Relaxed) + 1;
        let dialog = PageDialog { id, kind, origin, message, default_text };
        self.held.lock().unwrap().push((dialog.clone(), callback.clone()));
        let _ = self.out_tx.send(EngineMsg::Dialog { tab_id: self.tab_id, dialog });
        1
    }
}

/// Answers held dialog `id` of a tab; false when it's no longer held (answered, or reset).
pub fn answer(held: &Held, id: u64, accept: bool, text: &str) -> bool {
    let callback = {
        let mut held = held.lock().unwrap();
        let Some(at) = held.iter().position(|(d, _)| d.id == id) else { return false };
        held.remove(at).1
    };
    callback.cont(i32::from(accept), Some(&CefString::from(text)));
    true
}

/// Dismisses every held dialog of a tab, as an app that no longer shows them can't.
pub fn dismiss_all(held: &Held) {
    let all = std::mem::take(&mut *held.lock().unwrap());
    for (_, callback) in all {
        callback.cont(0, None);
    }
}

/// The tab's held dialogs, to tell an app that connects again.
pub fn pending(held: &Held) -> Vec<PageDialog> {
    held.lock().unwrap().iter().map(|(d, _)| d.clone()).collect()
}
