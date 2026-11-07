//! The `#[repr(C)]` protocol between the host (`opencode-desktop`) and a plugin library.
//!
//! A plugin exports one symbol, `opencode_plugin_v1` ([`PluginEntryFn`]). The host calls it with
//! a [`HostHandshake`] that carries the host's [`HostApiVtable`] and allocator; the plugin answers
//! with a [`PluginDescriptor`] that carries its [`PluginVtable`]. Both sides then check the magic,
//! epoch, [`ABI_LAYOUT_HASH`] and vtable sizes before calling anything else.
//!
//! Every function pointer is `extern "C"`, never `"C-unwind"`: a panic must not cross. The SDK
//! catches it inside the plugin and reports [`AbiStatus::Panicked`].
//!
//! Experimental: any change to a type here bumps [`ABI_EPOCH`] (or, for an appended vtable
//! entry, [`ABI_MINOR`]). The layout hash catches a change that forgets to.

use core::ffi::c_void;
use core::marker::PhantomData;
use core::mem::MaybeUninit;

/// "OCRUNEPL".
pub const ABI_MAGIC: u64 = 0x4F43_5255_4E45_504C;
pub const ABI_ENDIAN: u32 = 0x0102_0304;
/// Experimental epoch 1; bumped on any breaking layout change.
pub const ABI_EPOCH: u16 = 1;
/// Additive tail extensions to the vtables within an epoch.
pub const ABI_MINOR: u16 = 0;
/// What `plugin.toml`'s `api` names for this epoch.
pub const API_VERSION: &str = "0.1-experimental";
pub const SDK_VERSION: &str = env!("CARGO_PKG_VERSION");

pub const OS_OTHER: u8 = 0;
pub const OS_WINDOWS: u8 = 1;
pub const OS_MACOS: u8 = 2;
pub const OS_LINUX: u8 = 3;
pub const ARCH_OTHER: u8 = 0;
pub const ARCH_X86_64: u8 = 1;
pub const ARCH_AARCH64: u8 = 2;

/// The OS this crate was built for, as the handshake names it.
pub const HOST_OS: u8 = if cfg!(target_os = "windows") {
    OS_WINDOWS
} else if cfg!(target_os = "macos") {
    OS_MACOS
} else if cfg!(target_os = "linux") {
    OS_LINUX
} else {
    OS_OTHER
};

/// The architecture this crate was built for, as the handshake names it.
pub const HOST_ARCH: u8 = if cfg!(target_arch = "x86_64") {
    ARCH_X86_64
} else if cfg!(target_arch = "aarch64") {
    ARCH_AARCH64
} else {
    ARCH_OTHER
};

/// Borrowed UTF-8 (or byte) slice, valid for the duration of the `extern "C"` call. Zero-copy.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct AbiStr<'a> {
    pub ptr: *const u8,
    pub len: usize,
    pub _lt: PhantomData<&'a str>,
}

impl<'a> AbiStr<'a> {
    pub const EMPTY: AbiStr<'static> = AbiStr { ptr: core::ptr::null(), len: 0, _lt: PhantomData };

    pub const fn new(s: &'a str) -> Self {
        AbiStr { ptr: s.as_ptr(), len: s.len(), _lt: PhantomData }
    }

    pub const fn from_bytes(b: &'a [u8]) -> Self {
        AbiStr { ptr: b.as_ptr(), len: b.len(), _lt: PhantomData }
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The bytes.
    ///
    /// # Safety
    /// `ptr` must point at `len` readable bytes for `'a` (or `len` must be 0).
    pub unsafe fn as_bytes(&self) -> &'a [u8] {
        if self.len == 0 || self.ptr.is_null() {
            return &[];
        }
        // SAFETY: the caller guarantees `ptr..ptr+len` is readable for `'a`.
        unsafe { core::slice::from_raw_parts(self.ptr, self.len) }
    }

    /// The text, with invalid UTF-8 read as empty.
    ///
    /// # Safety
    /// As [`Self::as_bytes`].
    pub unsafe fn as_str(&self) -> &'a str {
        // SAFETY: forwarded to the caller.
        core::str::from_utf8(unsafe { self.as_bytes() }).unwrap_or("")
    }
}

impl<'a> From<&'a str> for AbiStr<'a> {
    fn from(s: &'a str) -> Self {
        AbiStr::new(s)
    }
}

/// Owned bytes carrying the destructor of the module that allocated them, so either side can
/// drop them without computing a `Layout` across compiler versions.
#[repr(C)]
pub struct AbiOwnedBytes {
    pub ptr: *mut u8,
    pub len: usize,
    pub cap: usize,
    pub drop_fn: Option<unsafe extern "C" fn(ptr: *mut u8, len: usize, cap: usize)>,
}

unsafe extern "C" fn drop_vec(ptr: *mut u8, len: usize, cap: usize) {
    // SAFETY: `ptr/len/cap` came from `Vec::into_raw_parts`-equivalent in `from_vec` of this module.
    drop(unsafe { Vec::from_raw_parts(ptr, len, cap) });
}

impl AbiOwnedBytes {
    pub const EMPTY: AbiOwnedBytes = AbiOwnedBytes { ptr: core::ptr::null_mut(), len: 0, cap: 0, drop_fn: None };

    pub fn from_vec(v: Vec<u8>) -> Self {
        let mut v = core::mem::ManuallyDrop::new(v);
        AbiOwnedBytes { ptr: v.as_mut_ptr(), len: v.len(), cap: v.capacity(), drop_fn: Some(drop_vec) }
    }

    pub fn from_string(s: String) -> Self {
        Self::from_vec(s.into_bytes())
    }

    pub fn as_bytes(&self) -> &[u8] {
        if self.ptr.is_null() || self.len == 0 {
            return &[];
        }
        // SAFETY: the creator handed over `len` initialised bytes at `ptr`, alive until dropped.
        unsafe { core::slice::from_raw_parts(self.ptr, self.len) }
    }

    /// The bytes as text, lossily.
    pub fn to_string_lossy(&self) -> String {
        String::from_utf8_lossy(self.as_bytes()).into_owned()
    }
}

impl Drop for AbiOwnedBytes {
    fn drop(&mut self) {
        if let Some(drop_fn) = self.drop_fn.take() {
            // SAFETY: `drop_fn` is the creator's destructor for exactly these parts, run once.
            unsafe { drop_fn(self.ptr, self.len, self.cap) };
        }
    }
}

/// Uniform outcome of every plugin callback; a panic becomes `Panicked` inside the plugin.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbiStatus {
    Ok = 0,
    /// E.g. a command declining its keybind so the key event propagates.
    Declined = 1,
    Err = 2,
    Panicked = 3,
}

#[repr(C)]
pub struct AbiOutcome<T> {
    pub status: AbiStatus,
    /// Initialised when `status` is `Ok` or `Declined`.
    pub value: MaybeUninit<T>,
    /// Set on `Err` and `Panicked`.
    pub message: AbiOwnedBytes,
}

impl<T> AbiOutcome<T> {
    pub fn ok(value: T) -> Self {
        AbiOutcome { status: AbiStatus::Ok, value: MaybeUninit::new(value), message: AbiOwnedBytes::EMPTY }
    }

    pub fn declined(value: T) -> Self {
        AbiOutcome { status: AbiStatus::Declined, value: MaybeUninit::new(value), message: AbiOwnedBytes::EMPTY }
    }

    pub fn err(message: String) -> Self {
        AbiOutcome { status: AbiStatus::Err, value: MaybeUninit::uninit(), message: AbiOwnedBytes::from_string(message) }
    }

    pub fn panicked(message: String) -> Self {
        AbiOutcome { status: AbiStatus::Panicked, value: MaybeUninit::uninit(), message: AbiOwnedBytes::from_string(message) }
    }

    /// The value, or the status and message when there is none.
    pub fn into_result(self) -> Result<(AbiStatus, T), (AbiStatus, String)> {
        match self.status {
            // SAFETY: `ok` and `declined` are the only constructors of these statuses, and both write `value`.
            AbiStatus::Ok | AbiStatus::Declined => Ok((self.status, unsafe { self.value.assume_init_read() })),
            status => Err((status, self.message.to_string_lossy())),
        }
    }
}

/// Opaque 32-bit slot index into the host's per-render `PluginRenderArena`.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ElHandle(pub u32);

/// A host-owned text field (`rune::TextEdit`): its text, selection, IME state and undo history
/// live in the host, so they survive a plugin reload.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EditorHandle(pub u32);

#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SurfaceHandle(pub u32);

impl ElHandle {
    pub const NONE: ElHandle = ElHandle(u32::MAX);
}
impl EditorHandle {
    pub const NONE: EditorHandle = EditorHandle(u32::MAX);
}
impl SurfaceHandle {
    pub const NONE: SurfaceHandle = SurfaceHandle(u32::MAX);
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElKind {
    Row = 0,
    Col = 1,
    Box = 2,
    Text = 3,
    /// `text` names a host icon; `font_size` is its size and `text_color` its colour.
    Icon = 4,
    Spacer = 5,
    NativeSurface = 6,
    /// A text field bound to `editor`; `text` is its placeholder.
    TextInput = 7,
}

/// [`ElSpec::flags`].
pub mod flags {
    pub const MONO: u64 = 1 << 0;
    pub const WRAP: u64 = 1 << 1;
    pub const TRUNCATE: u64 = 1 << 2;
    pub const TRUNCATE_FADE: u64 = 1 << 3;
    pub const FOCUSABLE: u64 = 1 << 4;
    pub const CLIP: u64 = 1 << 5;
    pub const CENTER: u64 = 1 << 6;
    pub const CENTER_Y: u64 = 1 << 7;
    pub const ITEMS_CENTER: u64 = 1 << 8;
    pub const ITEMS_END: u64 = 1 << 9;
    pub const JUSTIFY_BETWEEN: u64 = 1 << 10;
    pub const SELECTABLE: u64 = 1 << 11;
    pub const SHRINK: u64 = 1 << 12;
}

/// The plugin's handler for a press, a command or a change.
pub type ActionFn = unsafe extern "C" fn(state: *mut c_void, ctx: *mut c_void, data: usize, event: *const AbiEvent) -> AbiOutcome<bool>;
/// Draws a panel, a settings control or another surface.
pub type RenderFn = unsafe extern "C" fn(state: *const c_void, ctx: *mut c_void, data: usize, props: *const RenderProps) -> AbiOutcome<ElHandle>;
/// Builds a memoised subtree; called synchronously inside [`HostApiVtable::memo`].
pub type BuildFn = unsafe extern "C" fn(data: *mut c_void, ctx: *mut c_void) -> AbiOutcome<ElHandle>;
/// Frees a callback's captured data; compiled in the module that captured it.
pub type DropFn = unsafe extern "C" fn(data: usize);

/// A plugin closure the host holds: a trampoline, its captures (inline when they fit in a
/// `usize`, else boxed) and their destructor. The host owns it once passed and calls
/// `drop_data` exactly once.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Callback {
    pub func: Option<ActionFn>,
    pub data: usize,
    pub drop_data: Option<DropFn>,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RenderCallback {
    pub func: Option<RenderFn>,
    pub data: usize,
    pub drop_data: Option<DropFn>,
}

impl Callback {
    pub const NONE: Callback = Callback { func: None, data: 0, drop_data: None };
}
impl RenderCallback {
    pub const NONE: RenderCallback = RenderCallback { func: None, data: 0, drop_data: None };
}

pub const EVENT_PRESS: u32 = 0;
pub const EVENT_COMMAND: u32 = 1;
pub const EVENT_KEY: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AbiEvent {
    pub kind: u32,
    pub mods: u32,
    pub x: f32,
    pub y: f32,
}

pub const SURFACE_PANEL: u32 = 0;
pub const SURFACE_SETTING: u32 = 1;

/// What a render call draws for.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RenderProps<'a> {
    pub surface: u32,
    /// The panel or setting id the plugin registered.
    pub id: AbiStr<'a>,
    pub session_id: AbiStr<'a>,
    pub directory: AbiStr<'a>,
    pub width: f32,
    pub height: f32,
    pub visible: u8,
    pub focused: u8,
    pub dark: u8,
    pub _pad: u8,
}

pub type Rgba = [f32; 4];

/// The theme tokens a plugin draws with (the host's `Pal`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AbiPalette {
    pub dark: u32,
    pub theme: u32,
    pub bg_base: Rgba,
    pub layer1: Rgba,
    pub layer2: Rgba,
    pub layer3: Rgba,
    pub text: Rgba,
    pub muted: Rgba,
    pub faint: Rgba,
    pub accent: Rgba,
    pub bg_accent: Rgba,
    pub on_fill: Rgba,
    pub icon: Rgba,
    pub icon_muted: Rgba,
    pub border: Rgba,
    pub border_muted: Rgba,
    pub hover: Rgba,
    pub pressed: Rgba,
    pub success: Rgba,
    pub warning: Rgba,
    pub danger: Rgba,
    pub switch_thumb: Rgba,
}

/// Single-call element constructor (one FFI call per element instead of one per setter).
/// `NAN` means unset for sizes; `-1.0` for `width`/`height` means full.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ElSpec<'a> {
    pub kind: ElKind,
    pub key: AbiStr<'a>,
    pub role: AbiStr<'a>,
    pub label: AbiStr<'a>,
    pub help: AbiStr<'a>,
    pub text: AbiStr<'a>,
    pub icon_id: u32,
    pub surface: SurfaceHandle,
    pub editor: EditorHandle,
    pub width: f32,
    pub height: f32,
    pub min_w: f32,
    pub min_h: f32,
    pub max_w: f32,
    pub max_h: f32,
    pub grow: f32,
    pub shrink: f32,
    /// Top, right, bottom, left.
    pub pad: [f32; 4],
    pub margin: [f32; 4],
    pub gap: f32,
    /// Top-left, top-right, bottom-right, bottom-left.
    pub radius: [f32; 4],
    pub border_w: f32,
    pub fill: Rgba,
    pub hover_fill: Rgba,
    pub press_fill: Rgba,
    pub border_color: Rgba,
    pub text_color: Rgba,
    pub font_size: f32,
    pub font_weight: f32,
    pub line_height: f32,
    pub opacity: f32,
    pub flags: u64,
    pub press: Callback,
}

impl ElSpec<'static> {
    pub const EMPTY: ElSpec<'static> = ElSpec {
        kind: ElKind::Box,
        key: AbiStr::EMPTY,
        role: AbiStr::EMPTY,
        label: AbiStr::EMPTY,
        help: AbiStr::EMPTY,
        text: AbiStr::EMPTY,
        icon_id: 0,
        surface: SurfaceHandle::NONE,
        editor: EditorHandle::NONE,
        width: f32::NAN,
        height: f32::NAN,
        min_w: f32::NAN,
        min_h: f32::NAN,
        max_w: f32::NAN,
        max_h: f32::NAN,
        grow: 0.0,
        shrink: 0.0,
        pad: [0.0; 4],
        margin: [0.0; 4],
        gap: 0.0,
        radius: [0.0; 4],
        border_w: 0.0,
        fill: [0.0; 4],
        hover_fill: [0.0; 4],
        press_fill: [0.0; 4],
        border_color: [0.0; 4],
        text_color: [0.0; 4],
        font_size: 0.0,
        font_weight: 0.0,
        line_height: 0.0,
        opacity: 1.0,
        flags: 0,
        press: Callback::NONE,
    };
}

/// [`CommandSpecAbi::flags`].
pub const COMMAND_EDITABLE: u32 = 1 << 0;
pub const COMMAND_HIDDEN: u32 = 1 << 1;

#[repr(C)]
pub struct CommandSpecAbi<'a> {
    pub id: AbiStr<'a>,
    pub title: AbiStr<'a>,
    pub description: AbiStr<'a>,
    pub keywords: AbiStr<'a>,
    pub slash: AbiStr<'a>,
    /// Keybinds in `rune::Keybind` syntax (`mod+shift+h`), per OS; empty for none.
    pub bind_windows: AbiStr<'a>,
    pub bind_macos: AbiStr<'a>,
    pub bind_linux: AbiStr<'a>,
    /// `0` general, `1` session, `2` navigation, `3` model, `4` terminal, `5` prompt.
    pub section: u32,
    pub flags: u32,
    pub run: Callback,
}

/// [`PanelSpecAbi::flags`].
pub const PANEL_TRANSIENT: u32 = 1 << 0;

#[repr(C)]
pub struct PanelSpecAbi<'a> {
    pub id: AbiStr<'a>,
    pub title: AbiStr<'a>,
    /// A host icon name (`"code"`, `"globe"`, `"console"`, `"file-tree"`, `"folder"`).
    pub icon: AbiStr<'a>,
    pub order: i32,
    pub flags: u32,
    pub render: RenderCallback,
}

#[repr(C)]
pub struct SettingSpecAbi<'a> {
    pub id: AbiStr<'a>,
    pub title: AbiStr<'a>,
    pub description: AbiStr<'a>,
    pub keywords: AbiStr<'a>,
    /// Draws the row's control (a switch, a field).
    pub render: RenderCallback,
}

/// The host's heap. `export_plugin!`'s `#[global_allocator]` forwards every plugin allocation
/// here, so plugins share the host's counting allocator.
#[repr(C)]
pub struct HostAllocVtable {
    pub alloc: unsafe extern "C" fn(size: usize, align: usize) -> *mut u8,
    pub alloc_zeroed: unsafe extern "C" fn(size: usize, align: usize) -> *mut u8,
    pub dealloc: unsafe extern "C" fn(ptr: *mut u8, size: usize, align: usize),
    pub realloc: unsafe extern "C" fn(ptr: *mut u8, size: usize, align: usize, new_size: usize) -> *mut u8,
}

/// A refcounted, `Send + Sync` handle that wakes the host's event loop and marks the owning
/// plugin's poll dirty.
#[repr(C)]
pub struct WakerAbi {
    pub data: *const c_void,
    pub wake: Option<unsafe extern "C" fn(data: *const c_void)>,
    pub clone: Option<unsafe extern "C" fn(data: *const c_void) -> *const c_void>,
    pub drop: Option<unsafe extern "C" fn(data: *const c_void)>,
}

pub const LOG_ERROR: u32 = 0;
pub const LOG_WARN: u32 = 1;
pub const LOG_INFO: u32 = 2;
pub const LOG_DEBUG: u32 = 3;

/// What the host offers a plugin. `ctx` is the per-call context the host passed into the
/// callback being run; each entry names the callbacks it is valid in, and the host ignores a
/// call made from the wrong one.
#[repr(C)]
pub struct HostApiVtable {
    pub log: unsafe extern "C" fn(level: u32, message: AbiStr),
    // Setup.
    pub add_command: unsafe extern "C" fn(ctx: *mut c_void, spec: *const CommandSpecAbi) -> AbiStatus,
    pub add_panel: unsafe extern "C" fn(ctx: *mut c_void, spec: *const PanelSpecAbi) -> AbiStatus,
    pub add_setting: unsafe extern "C" fn(ctx: *mut c_void, spec: *const SettingSpecAbi) -> AbiStatus,
    pub create_editor: unsafe extern "C" fn(ctx: *mut c_void, initial: AbiStr) -> EditorHandle,
    pub waker: unsafe extern "C" fn(ctx: *mut c_void) -> WakerAbi,
    // Any callback.
    /// Writes a view of the field's text valid until the next host call into `out`.
    pub editor_text: unsafe extern "C" fn(ctx: *mut c_void, editor: EditorHandle, out: *mut AbiStr<'static>) -> u8,
    pub editor_version: unsafe extern "C" fn(ctx: *mut c_void, editor: EditorHandle) -> u64,
    // Handlers and poll.
    pub editor_set_text: unsafe extern "C" fn(ctx: *mut c_void, editor: EditorHandle, text: AbiStr),
    pub instant: unsafe extern "C" fn(ctx: *mut c_void),
    pub wake_after: unsafe extern "C" fn(ctx: *mut c_void, millis: u64),
    pub copy_text: unsafe extern "C" fn(ctx: *mut c_void, text: AbiStr),
    pub open_panel: unsafe extern "C" fn(ctx: *mut c_void, panel: AbiStr),
    pub toast: unsafe extern "C" fn(ctx: *mut c_void, text: AbiStr),
    // Render.
    pub palette: unsafe extern "C" fn(ctx: *mut c_void, out: *mut AbiPalette),
    pub el_build: unsafe extern "C" fn(ctx: *mut c_void, spec: *const ElSpec) -> ElHandle,
    pub el_apply: unsafe extern "C" fn(ctx: *mut c_void, el: ElHandle, spec: *const ElSpec),
    pub el_child: unsafe extern "C" fn(ctx: *mut c_void, parent: ElHandle, child: ElHandle),
    pub memo: unsafe extern "C" fn(ctx: *mut c_void, key: AbiStr, version: u64, build: BuildFn, data: *mut c_void) -> ElHandle,
}

/// What the host calls in a plugin, besides the callbacks it registered.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PluginVtable {
    /// Builds the per-window state; registers commands, panels and settings through `ctx`.
    pub window_setup: unsafe extern "C" fn(ctx: *mut c_void) -> AbiOutcome<*mut c_void>,
    pub drop_state: unsafe extern "C" fn(state: *mut c_void) -> AbiOutcome<u8>,
    /// Runs only when the plugin's waker fired or its `wake_after` came due; `true` when
    /// visible state changed.
    pub poll: unsafe extern "C" fn(state: *mut c_void, ctx: *mut c_void) -> AbiOutcome<bool>,
    /// Ephemeral state carried across a hot reload.
    pub save_hot_state: unsafe extern "C" fn(state: *const c_void, out: *mut AbiOwnedBytes) -> AbiOutcome<u8>,
    pub load_hot_state: unsafe extern "C" fn(state: *mut c_void, bytes: AbiStr) -> AbiOutcome<u8>,
}

#[repr(C)]
pub struct HostHandshake {
    pub magic: u64,
    pub endian: u32,
    pub abi_epoch: u16,
    pub abi_minor: u16,
    pub abi_layout_hash: u64,
    pub pointer_width: u8,
    pub os: u8,
    pub arch: u8,
    pub _pad: u8,
    pub host_vtable_size: u32,
    pub host_vtable: *const HostApiVtable,
    pub host_alloc: *const HostAllocVtable,
}

#[repr(C)]
pub struct PluginDescriptor {
    pub magic: u64,
    pub endian: u32,
    pub abi_epoch: u16,
    pub abi_minor: u16,
    pub abi_layout_hash: u64,
    pub sdk_version: AbiStr<'static>,
    pub plugin_vtable_size: u32,
    pub plugin_vtable: *const PluginVtable,
}

// SAFETY: a descriptor only points at the plugin's immutable statics.
unsafe impl Sync for PluginDescriptor {}

pub type PluginEntryFn = unsafe extern "C" fn(host: *const HostHandshake) -> *const PluginDescriptor;

/// The symbol every plugin library exports.
pub const ENTRY_SYMBOL: &[u8] = b"opencode_plugin_v1\0";

pub use crate::layout_hash::ABI_LAYOUT_HASH;
