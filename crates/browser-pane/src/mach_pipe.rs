//! Zero-copy cross-process `IOSurface` pipe over macOS Mach ports (`IOSurfaceCreateMachPort` /
//! `IOSurfaceLookupFromMachPort`).

use std::collections::HashMap;
use std::ffi::{CString, c_char, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use rune::NativeSurface;
use rune::native_surface::ColorInfo;

pub type MachPort = u32;
type KernReturn = i32;

const MACH_MSGH_BITS_COMPLEX: u32 = 0x8000_0000;
const MACH_MSG_TYPE_MOVE_SEND: u8 = 17;
const MACH_MSG_TYPE_COPY_SEND: u32 = 19;
const MACH_MSG_TYPE_MAKE_SEND: u32 = 20;
const MACH_MSG_PORT_DESCRIPTOR: u8 = 0;
const MACH_PORT_RIGHT_RECEIVE: u32 = 1;
const MACH_SEND_MSG: i32 = 0x0000_0001;
const MACH_RCV_MSG: i32 = 0x0000_0002;
const MACH_RCV_TIMEOUT: i32 = 0x0000_0100;
const TASK_BOOTSTRAP_PORT: i32 = 4;

unsafe extern "C" {
    fn mach_task_self() -> MachPort;
    fn task_get_special_port(task: MachPort, which: i32, port: *mut MachPort) -> KernReturn;
    fn mach_port_allocate(task: MachPort, right: u32, name: *mut MachPort) -> KernReturn;
    fn mach_port_insert_right(
        task: MachPort,
        name: MachPort,
        poly: MachPort,
        poly_poly: u32,
    ) -> KernReturn;
    fn mach_port_deallocate(task: MachPort, name: MachPort) -> KernReturn;
    fn mach_port_mod_refs(task: MachPort, name: MachPort, right: u32, delta: i32) -> KernReturn;
    fn bootstrap_register2(
        bp: MachPort,
        service_name: *const c_char,
        sp: MachPort,
        flags: u64,
    ) -> KernReturn;
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
    fn IOSurfaceLookupFromMachPort(port: MachPort) -> *mut c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *const c_void);
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

#[repr(C)]
#[derive(Copy, Clone, Default)]
struct FrameRecvMsg {
    msg: FrameSendMsg,
    trailer_a: [u8; 32],
    trailer_b: [u8; 32],
}

fn bootstrap_port() -> Result<MachPort, String> {
    let mut bp: MachPort = 0;
    let kr = unsafe { task_get_special_port(mach_task_self(), TASK_BOOTSTRAP_PORT, &mut bp) };
    if kr == 0 {
        Ok(bp)
    } else {
        Err(format!("task_get_special_port failed: {kr}"))
    }
}

pub struct MachReceiver {
    pub service_name: String,
    recv_port: MachPort,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl MachReceiver {
    pub fn start(
        surfaces: Arc<Mutex<HashMap<u64, NativeSurface>>>,
        unique_tag: u64,
    ) -> Result<Self, String> {
        let service_name = format!(
            "co.anomaly.opencode.browser.{}.{}",
            std::process::id(),
            unique_tag
        );
        let c_name = CString::new(service_name.as_str()).map_err(|e| e.to_string())?;
        let bp = bootstrap_port()?;
        let mut recv_port: MachPort = 0;
        unsafe {
            let task = mach_task_self();
            let kr = mach_port_allocate(task, MACH_PORT_RIGHT_RECEIVE, &mut recv_port);
            if kr != 0 {
                return Err(format!("mach_port_allocate failed: {kr}"));
            }
            let kr = mach_port_insert_right(task, recv_port, recv_port, MACH_MSG_TYPE_MAKE_SEND);
            if kr != 0 {
                return Err(format!("mach_port_insert_right failed: {kr}"));
            }
            let kr = bootstrap_register2(bp, c_name.as_ptr(), recv_port, 0);
            if kr != 0 {
                return Err(format!("bootstrap_register2({service_name}) failed: {kr}"));
            }
        }

        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = stop.clone();

        let handle = thread::Builder::new()
            .name("rune-browser-mach".into())
            .spawn(move || {
                while !stop_flag.load(Ordering::Relaxed) {
                    let mut rmsg = FrameRecvMsg::default();
                    let kr = unsafe {
                        mach_msg(
                            &mut rmsg.msg.header,
                            MACH_RCV_MSG | MACH_RCV_TIMEOUT,
                            0,
                            std::mem::size_of::<FrameRecvMsg>() as u32,
                            recv_port,
                            100,
                            0,
                        )
                    };
                    if kr != 0 {
                        continue;
                    }
                    let port = rmsg.msg.port_desc.name;
                    if port == 0 {
                        continue;
                    }
                    let io_surface = unsafe { IOSurfaceLookupFromMachPort(port) };
                    unsafe {
                        let _ = mach_port_deallocate(mach_task_self(), port);
                    }
                    if io_surface.is_null() {
                        continue;
                    }
                    let pb_res = unsafe {
                        rune::native_surface::macos::PixelBuffer::from_iosurface(
                            io_surface,
                            ColorInfo::default(),
                        )
                    };
                    unsafe {
                        CFRelease(io_surface);
                    }
                    if let Ok(pb) = pb_res {
                        let surface = surfaces.lock().unwrap().get(&rmsg.msg.tab_id).cloned();
                        if let Some(surface) = surface {
                            let _ = surface.push(pb, rmsg.msg.frame_id);
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;

        Ok(Self { service_name, recv_port, stop, handle: Some(handle) })
    }
}

impl Drop for MachReceiver {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        if self.recv_port != 0 {
            unsafe {
                let task = mach_task_self();
                let _ = mach_port_deallocate(task, self.recv_port);
                let _ = mach_port_mod_refs(task, self.recv_port, MACH_PORT_RIGHT_RECEIVE, -1);
            }
            self.recv_port = 0;
        }
    }
}

/// Looks up a registered `MachReceiver` service name in the user's bootstrap namespace.
pub fn lookup_service(service_name: &str) -> Result<MachPort, String> {
    let c_name = CString::new(service_name).map_err(|e| e.to_string())?;
    let bp = bootstrap_port()?;
    let mut port: MachPort = 0;
    let kr = unsafe { bootstrap_look_up(bp, c_name.as_ptr(), &mut port) };
    if kr == 0 && port != 0 {
        Ok(port)
    } else {
        Err(format!("bootstrap_look_up({service_name}) failed: {kr}"))
    }
}

/// Sends an `IOSurfaceRef` to a `MachReceiver` port without copying pixels.
///
/// # Safety
/// `io_surface` must be a valid `IOSurfaceRef`.
pub unsafe fn send_iosurface_frame(
    remote_port: MachPort,
    tab_id: u64,
    frame_id: u64,
    width: u32,
    height: u32,
    io_surface: *const c_void,
) -> bool {
    if remote_port == 0 || io_surface.is_null() {
        return false;
    }
    let surface_port = unsafe { IOSurfaceCreateMachPort(io_surface) };
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
        mach_msg(
            &mut msg.header,
            MACH_SEND_MSG,
            msg.header.msgh_size,
            0,
            0,
            0,
            0,
        )
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
