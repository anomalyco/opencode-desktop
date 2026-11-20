use cef::rc::Rc as _;
use cef::{Client, DisplayHandler, DownloadHandler, ImplClient, JsdialogHandler, LoadHandler, RenderHandler, RequestHandler, WrapClient, wrap_client};

wrap_client! {
    pub struct SidecarTabClient {
        render: RenderHandler,
        display: DisplayHandler,
        load: LoadHandler,
        request: RequestHandler,
        download: DownloadHandler,
        jsdialog: JsdialogHandler,
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
        fn jsdialog_handler(&self) -> Option<JsdialogHandler> {
            Some(self.jsdialog.clone())
        }
    }
}
