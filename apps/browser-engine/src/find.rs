//! CEF `FindHandler` forwarding in-page search match updates to the host (`find-machine.ts`).

use cef::rc::Rc as _;
use cef::{Browser, FindHandler, ImplFindHandler, Rect, WrapFindHandler, wrap_find_handler};

use crate::output::Output;
use crate::wire::EngineMsg;

wrap_find_handler! {
    pub struct TabFindHandler {
        tab_id: u64,
        out_tx: Output,
    }

    impl FindHandler {
        fn on_find_result(
            &self,
            _browser: Option<&mut Browser>,
            _identifier: ::std::os::raw::c_int,
            count: ::std::os::raw::c_int,
            _selection_rect: Option<&Rect>,
            active_match_ordinal: ::std::os::raw::c_int,
            final_update: ::std::os::raw::c_int,
        ) {
            let _ = self.out_tx.send(EngineMsg::FindResult {
                tab_id: self.tab_id,
                count: u32::try_from(count).unwrap_or(0),
                active_match: u32::try_from(active_match_ordinal).unwrap_or(0),
                final_update: final_update != 0,
            });
        }
    }
}
