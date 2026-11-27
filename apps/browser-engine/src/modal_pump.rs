//! Keeps the engine's turn going while a native modal dialog is open.
//!
//! A page's print dialog on a person's tab is the system's own (`PrintDlgEx`), opened from a task
//! inside `cef::do_message_loop_work`, and it runs its own message loop until it closes. The work
//! call doesn't return until then, so the engine read no message from the app, and no other tab
//! loaded or painted: the whole browser stopped while the dialog was open.
//!
//! The dialog's loop still dispatches this thread's timer messages. While a work call has run for
//! longer than `STUCK`, the timer runs the engine's turn from inside it: the app's messages, CEF's
//! work (Chromium allows its tasks in this nested loop), and the frames.

use std::cell::Cell;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::SetTimer;

/// A work call longer than this is a native dialog's loop; CEF's own slices are a few ms.
const STUCK: Duration = Duration::from_millis(150);
/// How often the turn runs while a dialog is open.
const EVERY_MS: u32 = 16;

type Turn = *mut (dyn FnMut() + 'static);

thread_local! {
    /// The running work call's start and its turn; the turn is taken while it runs, so it never
    /// runs inside itself.
    static WORK: Cell<Option<(Instant, Option<Turn>)>> = const { Cell::new(None) };
}

/// Starts the timer on the CEF UI thread (the main thread). Nothing pumps this thread's messages
/// outside a native dialog's loop, so the timer costs nothing until one opens.
#[allow(unsafe_code)]
pub(crate) fn install() {
    // SAFETY: a thread timer with a callback; it lives as long as the thread.
    unsafe { SetTimer(None, 0, EVERY_MS, Some(on_timer)) };
}

/// `cef::do_message_loop_work`, with `turn` run from any native dialog's loop inside it.
#[allow(unsafe_code)]
pub(crate) fn work(turn: &mut dyn FnMut()) {
    // SAFETY: only the lifetime is erased; the pointer is cleared before `turn`'s borrow ends.
    let turn: Turn = unsafe { std::mem::transmute::<*mut (dyn FnMut() + '_), Turn>(turn) };
    let outer = WORK.replace(Some((Instant::now(), Some(turn))));
    cef::do_message_loop_work();
    WORK.set(outer);
}

#[allow(unsafe_code)]
unsafe extern "system" fn on_timer(_: HWND, _: u32, _: usize, _: u32) {
    let Some((since, Some(turn))) = WORK.get() else { return };
    if since.elapsed() < STUCK {
        return;
    }
    WORK.set(Some((since, None)));
    // SAFETY: `work` is still on the stack below this dialog's loop, so `turn` is live, and it is
    // taken out above, so this is its only caller.
    unsafe { (*turn)() };
    WORK.set(Some((since, Some(turn))));
}
