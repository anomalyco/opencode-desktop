use cef::rc::Rc as _;
use cef::{
    Client, ContextMenuHandler, DialogHandler, DisplayHandler, DownloadHandler, FindHandler,
    ImplClient, JsdialogHandler, LifeSpanHandler, LoadHandler, PermissionHandler, RenderHandler,
    RequestHandler, WrapClient, wrap_client,
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
    }
}
