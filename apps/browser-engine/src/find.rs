//! CEF `FindHandler` forwarding in-page search match updates to the host (`find-machine.ts`).

use std::sync::atomic::{AtomicI32, Ordering};

use cef::rc::Rc as _;
use cef::{Browser, FindHandler, ImplFindHandler, Rect, WrapFindHandler, wrap_find_handler};

use crate::output::Output;
use crate::wire::EngineMsg;

wrap_find_handler! {
    pub struct TabFindHandler {
        tab_id: u64,
        out_tx: Output,
        // CEF passes `-1` for `count` or `active_match_ordinal` when unchanged from an earlier
        // incremental reply of the same search; keep the last non-negative values per session.
        last_id: AtomicI32,
        last_count: AtomicI32,
        last_active: AtomicI32,
    }

    impl FindHandler {
        fn on_find_result(
            &self,
            _browser: Option<&mut Browser>,
            identifier: ::std::os::raw::c_int,
            count: ::std::os::raw::c_int,
            _selection_rect: Option<&Rect>,
            active_match_ordinal: ::std::os::raw::c_int,
            final_update: ::std::os::raw::c_int,
        ) {
            if self.last_id.swap(identifier, Ordering::Relaxed) != identifier {
                self.last_count.store(0, Ordering::Relaxed);
                self.last_active.store(0, Ordering::Relaxed);
            }
            let resolved_count = if count >= 0 {
                self.last_count.store(count, Ordering::Relaxed);
                count
            } else {
                self.last_count.load(Ordering::Relaxed)
            };
            let resolved_active = if active_match_ordinal >= 0 {
                self.last_active.store(active_match_ordinal, Ordering::Relaxed);
                active_match_ordinal
            } else {
                self.last_active.load(Ordering::Relaxed)
            };
            let count_u32 = u32::try_from(resolved_count).unwrap_or(0);
            let mut active_u32 = u32::try_from(resolved_active).unwrap_or(0);
            if final_update != 0 && count_u32 > 0 && active_u32 == 0 {
                active_u32 = 1;
            }
            let _ = self.out_tx.send(EngineMsg::FindResult {
                tab_id: self.tab_id,
                count: count_u32,
                active_match: active_u32,
                final_update: final_update != 0,
            });
        }
    }
}
