use std::sync::Arc;

use cef::rc::Rc as _;
use cef::{
    Browser, CefString, Client, CommandHandler, ContextMenuHandler, DialogHandler, DisplayHandler, DownloadHandler,
    FindHandler, Frame, FrameHandler, ImplBrowser, ImplClient, ImplFrame, ImplFrameHandler,
    ImplCommandHandler, ImplListValue, ImplProcessMessage, JsdialogHandler, LifeSpanHandler, LoadHandler,
    PermissionHandler, ProcessId, ProcessMessage, RenderHandler, RequestHandler, WrapClient,
    WindowOpenDisposition, WrapCommandHandler, WrapFrameHandler, sys, wrap_client, wrap_command_handler,
    wrap_frame_handler,
};

pub(crate) fn ping_renderer_pid(frame: &Frame) {
    if frame.is_main() == 0 {
        return;
    }
    if let Some(mut msg) = cef::process_message_create(Some(&"opencode.renderer_pid_ping".into())) {
        frame.send_process_message(sys::cef_process_id_t::PID_RENDERER.into(), Some(&mut msg));
    }
}

wrap_frame_handler! {
    pub struct TabFrameHandler {
        tab_id: u64,
        prompts: Arc<crate::prompts::TabPrompts>,
        events: std::sync::mpsc::Sender<crate::Event>,
    }

    impl FrameHandler {
        fn on_frame_attached(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            reattached: ::std::os::raw::c_int,
        ) {
            let Some(frame) = frame else { return };
            if frame.is_main() == 0 {
                return;
            }
            let fid = CefString::from(&frame.identifier()).to_string();
            if reattached != 0 || self.prompts.needs_frame_pid(&fid) {
                ping_renderer_pid(frame);
            }
        }

        fn on_main_frame_changed(
            &self,
            _browser: Option<&mut Browser>,
            _old_frame: Option<&mut Frame>,
            new_frame: Option<&mut Frame>,
        ) {
            let fid = new_frame
                .as_deref()
                .map(|f| CefString::from(&f.identifier()).to_string())
                .filter(|id| !id.is_empty());
            if self.prompts.on_main_frame_changed(fid)
                && let Some(frame) = new_frame
            {
                ping_renderer_pid(frame);
            }
        }

        fn on_frame_destroyed(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>) {
            let Some(frame) = frame else { return };
            let fid = CefString::from(&frame.identifier()).to_string();
            self.prompts.on_frame_destroyed(&fid);
            if frame.is_main() == 0 {
                let _ = self.events.send(crate::Event::WebNav(crate::extensions::web_nav::WebNavSignal {
                    tab_id: self.tab_id,
                    cef_frame_id: fid,
                    parent_cef_id: None,
                    is_main: false,
                    url: String::new(),
                    kind: crate::extensions::web_nav::WebNavKind::FrameDestroyed,
                }));
            }
        }
    }
}

/// `IDC_PRINT` and `IDC_BASIC_PRINT` (`cef_command_ids.h`): the browser's own print command.
const PRINT_COMMANDS: [i32; 2] = [35003, 35007];

// Refuses the browser's print command (Ctrl+P, Ctrl+Shift+P) on tabs where nobody may be watching
// (`TabPrompts::print_gated`): its native dialog would stop the whole engine.
wrap_command_handler! {
    pub struct TabCommandHandler {
        prompts: Arc<crate::prompts::TabPrompts>,
    }

    impl CommandHandler {
        fn on_chrome_command(
            &self,
            _browser: Option<&mut Browser>,
            command_id: ::std::os::raw::c_int,
            _disposition: WindowOpenDisposition,
        ) -> ::std::os::raw::c_int {
            i32::from(PRINT_COMMANDS.contains(&command_id) && self.prompts.print_gated.load(std::sync::atomic::Ordering::Acquire))
        }
    }
}

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
        frame_handler: FrameHandler,
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
        fn frame_handler(&self) -> Option<FrameHandler> {
            Some(self.frame_handler.clone())
        }
        fn command_handler(&self) -> Option<CommandHandler> {
            Some(TabCommandHandler::new(self.prompts.clone()))
        }
        fn on_process_message_received(
            &self,
            browser: Option<&mut Browser>,
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
                    let fid = CefString::from(&frame.identifier()).to_string();
                    let main_id = browser
                        .and_then(|b| b.main_frame())
                        .map(|m| CefString::from(&m.identifier()).to_string());
                    self.prompts.record_frame_pid(fid, pid as u32, main_id.as_deref());
                }
            }
            1
        }
    }
}
