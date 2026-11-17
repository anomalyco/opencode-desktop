//! CEF display, load, DevTools observer, and Mach `IOSurface` frame sender.

use std::collections::HashMap;
#[cfg(target_os = "macos")]
use std::ffi::{CString, c_char, c_void};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use crate::output::{Output, Paint};
use std::sync::{Arc, Mutex};

use cef::rc::Rc as _;
use cef::{
    AcceleratedPaintInfo, Browser, Callback, CefString, CursorInfo, CursorType,
    DevToolsMessageObserver, DisplayHandler, Errorcode, Frame, ImplDevToolsMessageObserver,
    ImplDisplayHandler, ImplFrame, ImplLoadHandler, ImplRenderHandler, ImplRequest,
    ImplRequestHandler, ImplResourceRequestHandler, LoadHandler, PaintElementType, Rect,
    RenderHandler, Request, RequestHandler, ResourceRequestHandler, Response, ReturnValue,
    ScreenInfo, TerminationStatus, WindowOpenDisposition, WrapDevToolsMessageObserver,
    WrapDisplayHandler, WrapLoadHandler, WrapRenderHandler, WrapRequestHandler,
    WrapResourceRequestHandler, sys, wrap_dev_tools_message_observer, wrap_display_handler,
    wrap_load_handler, wrap_render_handler, wrap_request_handler, wrap_resource_request_handler,
};
use serde_json::Value;

use crate::wire::{CursorKind, EngineMsg, MOD_ALT, MOD_CMD, MOD_CTRL, MOD_SHIFT, TabSnapshot};

#[cfg(target_os = "macos")]
mod mach_tx {
    use super::*;

    type MachPort = u32;
    type KernReturn = i32;

    const MACH_MSGH_BITS_COMPLEX: u32 = 0x8000_0000;
    const MACH_MSG_TYPE_MOVE_SEND: u8 = 17;
    const MACH_MSG_TYPE_COPY_SEND: u32 = 19;
    const MACH_MSG_PORT_DESCRIPTOR: u8 = 0;
    const MACH_SEND_MSG: i32 = 0x0000_0001;
    const TASK_BOOTSTRAP_PORT: i32 = 4;

    unsafe extern "C" {
        fn mach_task_self() -> MachPort;
        fn task_get_special_port(task: MachPort, which: i32, port: *mut MachPort) -> KernReturn;
        fn mach_port_deallocate(task: MachPort, name: MachPort) -> KernReturn;
        fn bootstrap_look_up(bp: MachPort, service_name: *const c_char, sp: *mut MachPort) -> KernReturn;
        fn mach_msg(
            msg: *mut MachMsgHeader,
            option: i32,
            send_size: u32,
            rcv_size: u32,
            rcv_name: MachPort,
            timeout: u32,
            notify: MachPort,
        ) -> KernReturn;
    }

    #[link(name = "IOSurface", kind = "framework")]
    unsafe extern "C" {
        fn IOSurfaceCreateMachPort(buffer: *const c_void) -> MachPort;
    }

    #[repr(C)]
    #[derive(Copy, Clone, Default)]
    struct MachMsgHeader {
        msgh_bits: u32,
        msgh_size: u32,
        msgh_remote_port: MachPort,
        msgh_local_port: MachPort,
        msgh_voucher_port: MachPort,
        msgh_id: i32,
    }

    #[repr(C)]
    #[derive(Copy, Clone, Default)]
    struct MachMsgBody {
        msgh_descriptor_count: u32,
    }

    #[repr(C)]
    #[derive(Copy, Clone, Default)]
    struct MachMsgPortDescriptor {
        name: MachPort,
        pad1: u32,
        pad2: u16,
        disposition: u8,
        type_: u8,
    }

    #[repr(C)]
    #[derive(Copy, Clone, Default)]
    struct FrameSendMsg {
        header: MachMsgHeader,
        body: MachMsgBody,
        port_desc: MachMsgPortDescriptor,
        tab_id: u64,
        frame_id: u64,
        width: u32,
        height: u32,
    }

    pub fn lookup(name: &str) -> u32 {
        let Ok(c_name) = CString::new(name) else { return 0 };
        let mut bp: MachPort = 0;
        if unsafe { task_get_special_port(mach_task_self(), TASK_BOOTSTRAP_PORT, &mut bp) } != 0 {
            return 0;
        }
        let mut port: MachPort = 0;
        if unsafe { bootstrap_look_up(bp, c_name.as_ptr(), &mut port) } == 0 {
            port
        } else {
            0
        }
    }

    pub unsafe fn send_surface(
        remote_port: u32,
        tab_id: u64,
        frame_id: u64,
        width: u32,
        height: u32,
        raw: *const c_void,
    ) -> bool {
        if remote_port == 0 || raw.is_null() {
            return false;
        }
        let surface_port = unsafe { IOSurfaceCreateMachPort(raw) };
        if surface_port == 0 {
            return false;
        }
        let mut msg = FrameSendMsg {
            header: MachMsgHeader {
                msgh_bits: MACH_MSGH_BITS_COMPLEX | MACH_MSG_TYPE_COPY_SEND,
                msgh_size: std::mem::size_of::<FrameSendMsg>() as u32,
                msgh_remote_port: remote_port,
                msgh_local_port: 0,
                msgh_voucher_port: 0,
                msgh_id: 0x4345_4632,
            },
            body: MachMsgBody { msgh_descriptor_count: 1 },
            port_desc: MachMsgPortDescriptor {
                name: surface_port,
                pad1: 0,
                pad2: 0,
                disposition: MACH_MSG_TYPE_MOVE_SEND,
                type_: MACH_MSG_PORT_DESCRIPTOR,
            },
            tab_id,
            frame_id,
            width,
            height,
        };
        let kr = unsafe {
            mach_msg(&mut msg.header, MACH_SEND_MSG, msg.header.msgh_size, 0, 0, 0, 0)
        };
        if kr != 0 {
            unsafe {
                let _ = mach_port_deallocate(mach_task_self(), surface_port);
            }
            false
        } else {
            true
        }
    }
}

#[cfg(target_os = "macos")]
pub use mach_tx::lookup as lookup_mach_port;

/// Windows: the tab's D3D11 frame ring; None when the engine has no D3D11 device (CPU frames).
#[cfg(windows)]
pub type FrameRing = Option<Arc<Mutex<crate::d3d_frames::TabFrames>>>;
#[cfg(not(windows))]
pub type FrameRing = ();

/// The native cursor CEF hands `OnCursorChange` (unused: the app draws its own cursor).
#[cfg(windows)]
type NativeCursor = sys::HCURSOR;
#[cfg(target_os = "linux")]
type NativeCursor = sys::cef_cursor_handle_t;
#[cfg(target_os = "macos")]
type NativeCursor = *mut u8;

#[derive(Clone, Debug)]
pub struct ViewRectState {
    pub width_pt: f32,
    pub height_pt: f32,
    pub scale: f32,
}

wrap_render_handler! {
    pub struct TabRenderHandler {
        tab_id: u64,
        view_state: Arc<Mutex<ViewRectState>>,
        mach_port: Arc<AtomicU32>,
        out_tx: Output,
        frame_seq: Arc<AtomicU64>,
        frames: FrameRing,
    }

    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            let Some(rect) = rect else { return };
            let v = self.view_state.lock().unwrap();
            rect.x = 0;
            rect.y = 0;
            rect.width = v.width_pt.round().max(1.0) as i32;
            rect.height = v.height_pt.round().max(1.0) as i32;
        }

        fn screen_info(&self, _browser: Option<&mut Browser>, info: Option<&mut ScreenInfo>) -> i32 {
            let Some(info) = info else { return 0 };
            let v = self.view_state.lock().unwrap();
            let r = Rect {
                x: 0,
                y: 0,
                width: v.width_pt.round().max(1.0) as i32,
                height: v.height_pt.round().max(1.0) as i32,
            };
            info.device_scale_factor = v.scale;
            info.depth = 32;
            info.depth_per_component = 8;
            info.is_monochrome = 0;
            info.rect = r.clone();
            info.available_rect = r;
            1
        }

        fn on_accelerated_paint(
            &self,
            _browser: Option<&mut Browser>,
            type_: PaintElementType,
            _dirty: Option<&[Rect]>,
            info: Option<&AcceleratedPaintInfo>,
        ) {
            if type_ != PaintElementType::default() {
                return;
            }
            let Some(info) = info else { return };
            let id = self.frame_seq.fetch_add(1, Ordering::Relaxed) + 1;
            #[cfg(target_os = "macos")]
            {
                let port = self.mach_port.load(Ordering::Relaxed);
                unsafe {
                    let _ = mach_tx::send_surface(
                        port,
                        self.tab_id,
                        id,
                        info.extra.coded_size.width as u32,
                        info.extra.coded_size.height as u32,
                        info.shared_texture_io_surface,
                    );
                }
            }
            #[cfg(windows)]
            if let Some(ring) = &self.frames {
                let handle = windows::Win32::Foundation::HANDLE(info.shared_texture_handle);
                let copied = ring.lock().unwrap().copy(handle, id);
                if let Some(f) = copied {
                    let _ = self.out_tx.send(EngineMsg::SharedFrame {
                        tab_id: self.tab_id,
                        frame_id: id,
                        width: f.width,
                        height: f.height,
                        texture: f.texture,
                        release_fence: f.release_fence,
                    });
                }
            }
            #[cfg(not(any(target_os = "macos", windows)))]
            let _ = (id, info, &self.frames, &self.mach_port);
        }

        fn on_paint(
            &self,
            _browser: Option<&mut Browser>,
            type_: PaintElementType,
            _dirty: Option<&[Rect]>,
            buffer: *const u8,
            width: i32,
            height: i32,
        ) {
            if type_ != PaintElementType::default() || buffer.is_null() || width <= 0 || height <= 0 {
                return;
            }
            let id = self.frame_seq.fetch_add(1, Ordering::Relaxed) + 1;
            let len = (width as usize) * (height as usize) * 4;
            let slice = unsafe { std::slice::from_raw_parts(buffer, len) };
            // OnPaint owns the full buffer only for this call. A replacement must contain all
            // pixels: the app may never have seen the paints preceding this dirty rectangle.
            self.out_tx.paint(self.tab_id, Paint {
                frame_id: id,
                width: width as u32,
                height: height as u32,
                bgra: slice.to_vec(),
            });
        }
    }
}

wrap_display_handler! {
    pub struct TabDisplayHandler {
        snap: Arc<Mutex<TabSnapshot>>,
        out_tx: Output,
    }

    impl DisplayHandler {
        fn on_address_change(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            url: Option<&CefString>,
        ) {
            if frame.is_some_and(|f| f.is_main() != 1) {
                return;
            }
            if let Some(url) = url {
                let mut s = self.snap.lock().unwrap();
                let url = url.to_string();
                // Chromium's error page commits the failed address; another address is another document.
                if s.url != url {
                    s.load_error = None;
                }
                s.url = url;
                let _ = self.out_tx.send(EngineMsg::TabUpdate { tab: s.clone() });
            }
        }

        fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
            if let Some(title) = title {
                let mut s = self.snap.lock().unwrap();
                // Chromium's error page names itself by host alone; the failed page keeps its address's name.
                if s.load_error.as_deref().is_some_and(|e| e != "page_crashed") {
                    return;
                }
                s.title = title.to_string();
                let _ = self.out_tx.send(EngineMsg::TabUpdate { tab: s.clone() });
            }
        }

        fn on_cursor_change(
            &self,
            _browser: Option<&mut Browser>,
            _cursor: NativeCursor,
            type_: CursorType,
            _custom: Option<&CursorInfo>,
        ) -> i32 {
            use sys::cef_cursor_type_t as C;
            let kind = match *type_.as_ref() {
                C::CT_HAND => CursorKind::Pointer,
                C::CT_IBEAM | C::CT_VERTICALTEXT => CursorKind::Text,
                C::CT_CROSS => CursorKind::Crosshair,
                C::CT_COLUMNRESIZE | C::CT_EASTWESTRESIZE | C::CT_EASTRESIZE | C::CT_WESTRESIZE => {
                    CursorKind::ResizeColumn
                }
                C::CT_ROWRESIZE | C::CT_NORTHSOUTHRESIZE | C::CT_NORTHRESIZE | C::CT_SOUTHRESIZE => {
                    CursorKind::ResizeRow
                }
                C::CT_GRAB => CursorKind::Grab,
                C::CT_GRABBING => CursorKind::Grabbing,
                C::CT_NOTALLOWED => CursorKind::NotAllowed,
                C::CT_MOVE => CursorKind::Move,
                _ => CursorKind::Default,
            };
            let mut s = self.snap.lock().unwrap();
            if s.cursor != kind {
                s.cursor = kind;
                let _ = self.out_tx.send(EngineMsg::TabUpdate { tab: s.clone() });
            }
            1
        }
    }
}

wrap_load_handler! {
    pub struct TabLoadHandler {
        tab_id: u64,
        file_roots: Arc<Mutex<Vec<String>>>,
        snap: Arc<Mutex<TabSnapshot>>,
        out_tx: Output,
    }

    impl LoadHandler {
        fn on_loading_state_change(
            &self,
            _browser: Option<&mut Browser>,
            is_loading: i32,
            can_go_back: i32,
            can_go_forward: i32,
        ) {
            let mut s = self.snap.lock().unwrap();
            s.loading = is_loading != 0;
            s.can_go_back = can_go_back != 0;
            s.can_go_forward = can_go_forward != 0;
            let _ = self.out_tx.send(EngineMsg::TabUpdate { tab: s.clone() });
        }

        // A document from the network (any HTTP status) replaces a failed or crashed page; Chromium's
        // own error page ends with no status.
        fn on_load_end(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, http_status_code: i32) {
            if frame.is_some_and(|f| f.is_main() != 1) || http_status_code <= 0 {
                return;
            }
            let mut s = self.snap.lock().unwrap();
            if s.load_error.take().is_some() {
                let _ = self.out_tx.send(EngineMsg::TabUpdate { tab: s.clone() });
            }
        }

        // A main-frame load that failed, as Desktop's did-fail-load: a cancelled one (a newer
        // navigation, a download) keeps the page.
        fn on_load_error(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            error_code: Errorcode,
            error_text: Option<&CefString>,
            failed_url: Option<&CefString>,
        ) {
            use sys::cef_errorcode_t as E;
            let code = *error_code.as_ref();
            if frame.is_some_and(|f| f.is_main() != 1) || code == E::ERR_ABORTED {
                return;
            }
            // Chromium blocks unsafe cross-scheme redirects (`302 -> file://` or `302 -> data:`)
            // and disallowed schemes in the network layer before `on_before_browse`: keep the
            // open page and report the policy refusal, matching Desktop's `will-redirect`.
            if matches!(code, E::ERR_UNSAFE_REDIRECT | E::ERR_DISALLOWED_URL_SCHEME | E::ERR_UNKNOWN_URL_SCHEME) {
                let roots = self.file_roots.lock().unwrap().clone();
                let why = if roots.is_empty() {
                    crate::policy::Refusal::Web
                } else {
                    crate::policy::Refusal::Workspace
                };
                let _ = self.out_tx.send(EngineMsg::Refused {
                    tab_id: self.tab_id,
                    reason: why.code().to_owned(),
                });
                return;
            }
            let mut s = self.snap.lock().unwrap();
            if let Some(url) = failed_url.map(CefString::to_string).filter(|u| !u.is_empty()) {
                // As Desktop's tab names a failed page: its host and port.
                let rest = url.split_once("://").map_or(url.as_str(), |(_, r)| r);
                s.title = rest.split(['/', '?', '#']).next().unwrap_or(rest).to_owned();
                s.url = url;
            }
            let text = error_text.map(CefString::to_string).unwrap_or_default();
            s.load_error = Some(if text.is_empty() { format!("net error {}", *error_code.as_ref() as i32) } else { text });
            let _ = self.out_tx.send(EngineMsg::TabUpdate { tab: s.clone() });
        }
    }
}

wrap_resource_request_handler! {
    pub struct TabResourceRequestHandler {
        file_roots: Arc<Mutex<Vec<String>>>,
    }

    impl ResourceRequestHandler {
        // Sub-resources (`img`, `script`, `link`, `fetch`, `XHR`) may never read `file:` URLs
        // outside the tab's allowed workspace roots (`pane.ts:103-107`).
        fn on_before_resource_load(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            request: Option<&mut Request>,
            _callback: Option<&mut Callback>,
        ) -> ReturnValue {
            let url = request.map(|r| CefString::from(&r.url()).to_string()).unwrap_or_default();
            let roots = self.file_roots.lock().unwrap().clone();
            if crate::policy::subresource_allowed(&url, &roots) {
                ReturnValue::CONTINUE
            } else {
                ReturnValue::CANCEL
            }
        }

        fn on_resource_redirect(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _request: Option<&mut Request>,
            _response: Option<&mut Response>,
            new_url: Option<&mut CefString>,
        ) {
            let Some(new_url) = new_url else { return };
            let target = new_url.to_string();
            let roots = self.file_roots.lock().unwrap().clone();
            if !crate::policy::subresource_allowed(&target, &roots) {
                *new_url = CefString::from("");
            }
        }

        // Never launch external OS protocol handlers (`mailto:`, `ms-calculator:`, ...) from CEF.
        fn on_protocol_execution(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _request: Option<&mut Request>,
            allow_os_execution: Option<&mut ::std::os::raw::c_int>,
        ) {
            if let Some(allow) = allow_os_execution {
                *allow = 0;
            }
        }
    }
}

wrap_request_handler! {
    pub struct TabRequestHandler {
        tab_id: u64,
        file_roots: Arc<Mutex<Vec<String>>>,
        snap: Arc<Mutex<TabSnapshot>>,
        out_tx: Output,
    }

    impl RequestHandler {
        // Navigation and redirect guard (`chromium.ts:432-446`): main-frame navigations require
        // `about:blank`, credential-free `http:`/`https:`, or a `file:` URL inside `file_roots`;
        // sub-frames additionally forbid `file:` URLs outside `file_roots`.
        fn on_before_browse(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            request: Option<&mut Request>,
            _user_gesture: ::std::os::raw::c_int,
            _is_redirect: ::std::os::raw::c_int,
        ) -> ::std::os::raw::c_int {
            let is_main = frame.map_or(true, |f| f.is_main() == 1);
            let url = request.map(|r| CefString::from(&r.url()).to_string()).unwrap_or_default();
            let roots = self.file_roots.lock().unwrap().clone();
            if let Some(why) = crate::policy::frame_guard(&url, is_main, &roots) {
                let _ = self.out_tx.send(EngineMsg::Refused {
                    tab_id: self.tab_id,
                    reason: why.code().to_owned(),
                });
                return 1;
            }
            0
        }

        fn on_open_urlfrom_tab(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            target_url: Option<&CefString>,
            _target_disposition: WindowOpenDisposition,
            _user_gesture: ::std::os::raw::c_int,
        ) -> ::std::os::raw::c_int {
            let url = target_url.map(CefString::to_string).unwrap_or_default();
            let roots = self.file_roots.lock().unwrap().clone();
            if let Some(why) = crate::policy::frame_guard(&url, true, &roots) {
                let _ = self.out_tx.send(EngineMsg::Refused {
                    tab_id: self.tab_id,
                    reason: why.code().to_owned(),
                });
                return 1;
            }
            0
        }

        fn resource_request_handler(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _request: Option<&mut Request>,
            _is_navigation: ::std::os::raw::c_int,
            _is_download: ::std::os::raw::c_int,
            _request_initiator: Option<&CefString>,
            _disable_default_handling: Option<&mut ::std::os::raw::c_int>,
        ) -> Option<ResourceRequestHandler> {
            Some(TabResourceRequestHandler::new(self.file_roots.clone()))
        }

        // The page's renderer went away (crashed, killed, out of memory): the tab keeps its address,
        // and a reload loads it in a new renderer, as Desktop's crashed tab does.
        fn on_render_process_terminated(
            &self,
            _browser: Option<&mut Browser>,
            _status: TerminationStatus,
            _error_code: i32,
            _error_string: Option<&CefString>,
        ) {
            let mut s = self.snap.lock().unwrap();
            s.loading = false;
            s.load_error = Some("page_crashed".into());
            let _ = self.out_tx.send(EngineMsg::TabUpdate { tab: s.clone() });
        }
    }
}

wrap_dev_tools_message_observer! {
    pub struct TabCdpObserver {
        tab_id: u64,
        id_map: Arc<Mutex<HashMap<i32, u64>>>,
        internal_picked: Arc<Mutex<Vec<i64>>>,
        out_tx: Output,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(
            &self,
            _browser: Option<&mut Browser>,
            message_id: i32,
            success: i32,
            result: Option<&[u8]>,
        ) {
            let Some(req_id) = self.id_map.lock().unwrap().remove(&message_id) else {
                return;
            };
            let val = result
                .and_then(|b| serde_json::from_slice::<Value>(b).ok())
                .unwrap_or(Value::Null);
            let _ = self.out_tx.send(EngineMsg::CdpResponse {
                tab_id: self.tab_id,
                request_id: req_id,
                ok: success != 0,
                result: val,
            });
        }

        fn on_dev_tools_event(
            &self,
            _browser: Option<&mut Browser>,
            method: Option<&CefString>,
            params: Option<&[u8]>,
        ) {
            let Some(m) = method.map(CefString::to_string) else { return };
            let val = params
                .and_then(|b| serde_json::from_slice::<Value>(b).ok())
                .unwrap_or(Value::Null);
            if m == "Overlay.inspectNodeRequested"
                && let Some(backend_id) = val["backendNodeId"].as_i64()
            {
                self.internal_picked.lock().unwrap().push(backend_id);
            }
            let session_id = val["sessionId"].as_str().map(str::to_owned);
            let _ = self.out_tx.send(EngineMsg::CdpEvent {
                tab_id: self.tab_id,
                method: m,
                params: val,
                session_id,
            });
        }
    }
}

/// An event flag as the `u32` CEF's event structs take (the flag enum is `i32` on Windows).
#[allow(clippy::unnecessary_cast)]
pub fn event_flag(flag: sys::cef_event_flags_t) -> u32 {
    flag.0 as u32
}

pub fn cef_mods(bits: u8) -> u32 {
    let mut flags = 0u32;
    if bits & MOD_SHIFT != 0 {
        flags |= event_flag(sys::cef_event_flags_t::EVENTFLAG_SHIFT_DOWN);
    }
    if bits & MOD_CTRL != 0 {
        flags |= event_flag(sys::cef_event_flags_t::EVENTFLAG_CONTROL_DOWN);
    }
    if bits & MOD_ALT != 0 {
        flags |= event_flag(sys::cef_event_flags_t::EVENTFLAG_ALT_DOWN);
    }
    if bits & MOD_CMD != 0 {
        flags |= event_flag(sys::cef_event_flags_t::EVENTFLAG_COMMAND_DOWN);
    }
    flags
}

/// The wheel delta CEF takes for the app's scroll in points: points elsewhere, but on Windows
/// `WHEEL_DELTA` (120) units that CEF turns into the system's lines (or characters, sideways) of
/// 100/3 px each. Converted so the page scrolls by the points the app reports, as on macOS.
#[cfg(not(windows))]
pub fn wheel_delta(dx: f32, dy: f32) -> (f32, f32) {
    (dx, dy)
}

#[cfg(windows)]
pub fn wheel_delta(dx: f32, dy: f32) -> (f32, f32) {
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETWHEELSCROLLCHARS, SPI_GETWHEELSCROLLLINES, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };
    let setting = |action| {
        let mut value = 3u32;
        let read = unsafe {
            SystemParametersInfoW(action, 0, Some((&raw mut value).cast()), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0))
        };
        // WHEEL_PAGESCROLL (u32::MAX) pages instead of lines; keep the default then.
        if read.is_ok() && (1..1000).contains(&value) { value as f32 } else { 3.0 }
    };
    let units = |px: f32, per_notch: f32| px * 120.0 / (per_notch * 100.0 / 3.0);
    (units(dx, setting(SPI_GETWHEELSCROLLCHARS)), units(dy, setting(SPI_GETWHEELSCROLLLINES)))
}

/// The key's `native_key_code`: a macOS virtual key code, or on Windows the `WM_KEYDOWN`
/// `lParam` with the key's scan code, which gives the page its `KeyboardEvent.code`.
#[cfg(target_os = "macos")]
pub fn native_key_code(_vk: i32, mac: i32) -> i32 {
    mac
}

/// Chromium's Linux keyboard events use XKB keycodes (evdev + 8), including under Ozone.
/// The wire carries logical keys; resolve the US-layout physical key for known keys, and
/// leave the physical code unspecified for text outside that layout.
#[cfg(target_os = "linux")]
pub fn native_key_code(vk: i32, _mac: i32) -> i32 {
    for (row, first) in [(b"1234567890".as_slice(), 10), (b"QWERTYUIOP", 24), (b"ASDFGHJKL", 38), (b"ZXCVBNM", 52)] {
        if let Some(offset) = row.iter().position(|&key| i32::from(key) == vk) {
            return first + offset as i32;
        }
    }
    match vk {
        0x1b => 9, 0xbd => 20, 0xbb => 21, 0x08 => 22, 0x09 => 23,
        0xdb => 34, 0xdd => 35, 0x0d => 36, 0xba => 47, 0xde => 48, 0xc0 => 49,
        0xdc => 51, 0xbc => 59, 0xbe => 60, 0xbf => 61, 0x20 => 65,
        0x24 => 110, 0x26 => 111, 0x21 => 112, 0x25 => 113, 0x27 => 114,
        0x23 => 115, 0x28 => 116, 0x22 => 117, 0x2e => 119,
        _ => 0,
    }
}

#[cfg(windows)]
pub fn native_key_code(vk: i32, _mac: i32) -> i32 {
    use windows::Win32::UI::Input::KeyboardAndMouse::{MAPVK_VK_TO_VSC_EX, MapVirtualKeyW};
    let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC_EX) };
    // Extended keys (arrows, Home, Delete…) come back as 0xE0nn; lParam bit 24 marks them.
    let extended = u32::from(scan & 0xFF00 == 0xE000) << 24;
    (((scan & 0xFF) << 16) | extended | 1) as i32
}

pub fn ascii_vk(ch: char) -> i32 {
    match ch {
        'a'..='z' | 'A'..='Z' => ch.to_ascii_uppercase() as i32,
        '0'..='9' => ch as i32,
        ' ' => 0x20, '-' | '_' => 0xBD, '=' | '+' => 0xBB, '[' | '{' => 0xDB,
        ']' | '}' => 0xDD, '\\' | '|' => 0xDC, ';' | ':' => 0xBA, '\'' | '"' => 0xDE,
        ',' | '<' => 0xBC, '.' | '>' => 0xBE, '/' | '?' => 0xBF, '`' | '~' => 0xC0,
        _ => ch as i32,
    }
}

pub fn mac_key_code(ch: char) -> i32 {
    match ch.to_ascii_lowercase() {
        'a' => 0, 's' => 1, 'd' => 2, 'f' => 3, 'h' => 4, 'g' => 5, 'z' => 6, 'x' => 7,
        'c' => 8, 'v' => 9, 'b' => 11, 'q' => 12, 'w' => 13, 'e' => 14, 'r' => 15,
        'y' => 16, 't' => 17, '1' | '!' => 18, '2' | '@' => 19, '3' | '#' => 20,
        '4' | '$' => 21, '6' | '^' => 22, '5' | '%' => 23, '=' | '+' => 24, '9' | '(' => 25,
        '7' | '&' => 26, '-' | '_' => 27, '8' | '*' => 28, '0' | ')' => 29, ']' | '}' => 30,
        'o' => 31, 'u' => 32, '[' | '{' => 33, 'i' => 34, 'p' => 35, 'l' => 37, 'j' => 38,
        '\'' | '"' => 39, 'k' => 40, ';' | ':' => 41, '\\' | '|' => 42, ',' | '<' => 43,
        '/' | '?' => 44, 'n' => 45, 'm' => 46, '.' | '>' => 47, ' ' => 49, '`' | '~' => 50,
        _ => 0,
    }
}

pub fn named_codes(name: &str) -> Option<(i32, i32, Option<u16>)> {
    match name {
        "Enter" => Some((0x0D, 36, Some('\r' as u16))),
        "Backspace" => Some((0x08, 51, None)),
        "Delete" => Some((0x2E, 117, None)),
        "Tab" => Some((0x09, 48, None)),
        "Escape" => Some((0x1B, 53, None)),
        "ArrowLeft" => Some((0x25, 123, None)),
        "ArrowRight" => Some((0x27, 124, None)),
        "ArrowDown" => Some((0x28, 125, None)),
        "ArrowUp" => Some((0x26, 126, None)),
        "Home" => Some((0x24, 115, None)),
        "End" => Some((0x23, 119, None)),
        "PageUp" => Some((0x21, 116, None)),
        "PageDown" => Some((0x22, 121, None)),
        _ => None,
    }
}
