use std::sync::Arc;
use std::sync::atomic::Ordering;

use cef::rc::Rc as _;
use cef::{
    Browser, CefString, Client, ContextMenuHandler, DialogHandler, DisplayHandler, DownloadHandler,
    FindHandler, Frame, ImplClient, ImplFrame, ImplListValue, ImplProcessMessage, JsdialogHandler,
    LifeSpanHandler, LoadHandler, PermissionHandler, ProcessId, ProcessMessage, RenderHandler,
    RequestHandler, WrapClient, sys, wrap_client,
};

wrap_client! {
    pub struct SidecarTabClient {
        render: RenderHandler,
        display: DisplayHandler,
        load: LoadHandler,
        request: RequestHandler,
        download: DownloadHandler,
        life_span: LifeSpanHandler,
        jsdialog: JsdialogHandler,
        permission: PermissionHandler,
        dialog: DialogHandler,
        find: FindHandler,
        context_menu: ContextMenuHandler,
        prompts: Arc<crate::prompts::TabPrompts>,
    }

    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            Some(self.render.clone())
        }
        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(self.display.clone())
        }
        fn load_handler(&self) -> Option<LoadHandler> {
            Some(self.load.clone())
        }
        fn request_handler(&self) -> Option<RequestHandler> {
            Some(self.request.clone())
        }
        fn download_handler(&self) -> Option<DownloadHandler> {
            Some(self.download.clone())
        }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(self.life_span.clone())
        }
        fn jsdialog_handler(&self) -> Option<JsdialogHandler> {
            Some(self.jsdialog.clone())
        }
        fn permission_handler(&self) -> Option<PermissionHandler> {
            Some(self.permission.clone())
        }
        fn dialog_handler(&self) -> Option<DialogHandler> {
            Some(self.dialog.clone())
        }
        fn find_handler(&self) -> Option<FindHandler> {
            Some(self.find.clone())
        }
        fn context_menu_handler(&self) -> Option<ContextMenuHandler> {
            Some(self.context_menu.clone())
        }
        fn on_process_message_received(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            source_process: ProcessId,
            message: Option<&mut ProcessMessage>,
        ) -> ::std::os::raw::c_int {
            if source_process != ProcessId::from(sys::cef_process_id_t::PID_RENDERER) {
                return 0;
            }
            let Some(frame) = frame else { return 0 };
            if frame.is_main() == 0 {
                return 0;
            }
            let Some(msg) = message else { return 0 };
            let name = CefString::from(&msg.name()).to_string();
            if name != "opencode.renderer_pid" {
                return 0;
            }
            if let Some(args) = msg.argument_list() {
                let pid = args.int(0);
                if pid > 0 {
                    self.prompts.renderer_pid.store(pid as u32, Ordering::Release);
                }
            }
            1
        }
    }
}
