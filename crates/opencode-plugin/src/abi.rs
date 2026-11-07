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
/// Experimental epoch 3 (epoch 2's contribution types, app plugins, services, stores, tasks and
/// icons, plus session reads, server requests, popovers, theme tokens and richer elements);
/// bumped on any breaking layout change.
pub const ABI_EPOCH: u16 = 3;
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
    /// A number whose changed digits roll (the app's odometer): `text` is what it reads, `value`
    /// the number it stands for, `key` its identity across renders.
    Odometer = 8,
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
    /// Scrolls its overflow vertically.
    pub const SCROLL: u64 = 1 << 13;
    /// Tabular figures (`font-variant-numeric: tabular-nums`).
    pub const TABULAR: u64 = 1 << 14;
    /// Children that set a group colour take it while this element is hovered.
    pub const HOVER_GROUP: u64 = 1 << 15;
    /// Its hover and press fills land at once rather than fading.
    pub const INSTANT_HOVER: u64 = 1 << 16;
    /// Lays out after its siblings without taking room (`El::absolute` at `pin`).
    pub const ABSOLUTE: u64 = 1 << 17;
    /// Tooltip shows above rather than below.
    pub const HELP_ABOVE: u64 = 1 << 18;
    /// Text that doesn't break.
    pub const NO_WRAP: u64 = 1 << 19;
}

/// [`ElSpec::shadow`]: Desktop's elevation tokens.
pub const SHADOW_NONE: u32 = 0;
pub const SHADOW_RAISED: u32 = 1;
pub const SHADOW_FLOATING: u32 = 2;
pub const SHADOW_OVERLAY: u32 = 3;

/// [`ElSpec::anchor_side`]: which side of the element keyed `anchor` an overlay sits on.
pub const SIDE_NONE: u32 = 0;
pub const SIDE_TOP: u32 = 1;
pub const SIDE_BOTTOM: u32 = 2;
pub const SIDE_LEFT: u32 = 3;
pub const SIDE_RIGHT: u32 = 4;
/// [`ElSpec::anchor_align`].
pub const ALIGN_START: u32 = 0;
pub const ALIGN_CENTER: u32 = 1;
pub const ALIGN_END: u32 = 2;

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

/// Answers a question the host asks (does this link match, what does this tool return): `input`
/// in, an answer written to `out`, `Ok(1)` for yes, `Ok(0)` for no, `Err` with a message.
pub type QueryFn = unsafe extern "C" fn(state: *mut c_void, ctx: *mut c_void, data: usize, input: AbiStr, out: *mut AbiOwnedBytes) -> AbiOutcome<u8>;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct QueryCallback {
    pub func: Option<QueryFn>,
    pub data: usize,
    pub drop_data: Option<DropFn>,
}

impl QueryCallback {
    pub const NONE: QueryCallback = QueryCallback { func: None, data: 0, drop_data: None };
}

pub const EVENT_PRESS: u32 = 0;
pub const EVENT_COMMAND: u32 = 1;
pub const EVENT_KEY: u32 = 2;
/// A link handler opening `text` (the href).
pub const EVENT_LINK: u32 = 3;
/// A code block action on `text` (the code) in language `extra`.
pub const EVENT_CODE_BLOCK: u32 = 4;
/// A menu item; `text` is its target (a server's id for `server.row`).
pub const EVENT_MENU: u32 = 5;
/// A job running on a host worker thread (`HostApiVtable::spawn`).
pub const EVENT_JOB: u32 = 6;
/// The pointer entered (`x` 1) or left (`x` 0) an element with a hover handler.
pub const EVENT_HOVER: u32 = 7;
/// A server request's answer (`HostApiVtable::server_request`): `text` is the body, `extra`
/// the error ("" on success), `mods` 0 on success and 1 on failure.
pub const EVENT_RESPONSE: u32 = 8;
/// A file the host saved (`HostApiVtable::save_file`): `text` is its path, `extra` the error.
pub const EVENT_SAVED: u32 = 9;

/// What ran a callback. The strings are views valid for the call.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AbiEvent {
    pub kind: u32,
    pub mods: u32,
    pub x: f32,
    pub y: f32,
    pub text: AbiStr<'static>,
    pub extra: AbiStr<'static>,
}

impl Default for AbiEvent {
    fn default() -> Self {
        AbiEvent { kind: 0, mods: 0, x: 0.0, y: 0.0, text: AbiStr::EMPTY, extra: AbiStr::EMPTY }
    }
}

impl core::fmt::Debug for AbiStr<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "AbiStr({} bytes)", self.len)
    }
}

pub const SURFACE_PANEL: u32 = 0;
pub const SURFACE_SETTING: u32 = 1;
pub const SURFACE_SLOT: u32 = 2;
pub const SURFACE_TITLEBAR: u32 = 3;
pub const SURFACE_PILL: u32 = 4;
pub const SURFACE_FILE: u32 = 5;
pub const SURFACE_SETTINGS_PAGE: u32 = 6;
pub const SURFACE_CODE_BLOCK: u32 = 7;
pub const SURFACE_POPOVER: u32 = 8;
/// A panel tab's label in the side region's strip (`PanelSpec::label`).
pub const SURFACE_TAB: u32 = 9;

/// [`RenderProps::flags`].
/// The session's view is the one on screen (a slot's `active`).
pub const RENDER_ACTIVE: u32 = 1 << 0;
/// The header needs the room: draw the compact form (a slot's `compact`).
pub const RENDER_COMPACT: u32 = 1 << 1;
/// The tab is the selected one in its strip (a tab label).
pub const RENDER_SELECTED: u32 = 1 << 2;

/// What a render call draws for.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RenderProps<'a> {
    pub surface: u32,
    /// The panel, setting, slot or viewer id the plugin registered.
    pub id: AbiStr<'a>,
    pub session_id: AbiStr<'a>,
    pub directory: AbiStr<'a>,
    /// A file viewer's file (absolute), a code block's language.
    pub path: AbiStr<'a>,
    /// A code block's source; a file viewer's file as the host loaded it (empty when it didn't).
    pub text: AbiStr<'a>,
    pub width: f32,
    pub height: f32,
    pub visible: u8,
    pub focused: u8,
    pub dark: u8,
    pub _pad: u8,
    pub flags: u32,
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
    pub icon_faint: Rgba,
    /// `background-bg-layer-04`.
    pub layer4: Rgba,
    pub border_strong: Rgba,
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
    /// Letter spacing in points.
    pub tracking: f32,
    /// [`SHADOW_RAISED`] and the rest: one of Desktop's elevations.
    pub shadow: u32,
    /// An odometer's number.
    pub value: f64,
    /// A text or icon colour while the nearest [`flags::HOVER_GROUP`] ancestor is hovered.
    pub group_color: Rgba,
    /// Left, top, right, bottom offsets with [`flags::ABSOLUTE`] (`NAN` unset).
    pub pin: [f32; 4],
    /// An overlay placed beside the element keyed `anchor` ([`SIDE_BOTTOM`] and the rest), and
    /// aligned along it ([`ALIGN_START`] …), `anchor_gap` points away. Draw it after its anchor.
    pub anchor: AbiStr<'a>,
    pub anchor_side: u32,
    pub anchor_align: u32,
    pub anchor_gap: f32,
    /// Runs as the pointer enters and leaves ([`EVENT_HOVER`]).
    pub hover: Callback,
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
        tracking: 0.0,
        shadow: SHADOW_NONE,
        value: 0.0,
        group_color: [0.0; 4],
        pin: [f32::NAN; 4],
        anchor: AbiStr::EMPTY,
        anchor_side: SIDE_NONE,
        anchor_align: ALIGN_START,
        anchor_gap: 0.0,
        hover: Callback::NONE,
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
/// No New tab card or `+` row: something else opens it (Desktop's panels without a `MenuItem`).
pub const PANEL_NO_LAUNCHER: u32 = 1 << 1;
/// Stored at the front of the strip (`BuiltinPanelTab.first`).
pub const PANEL_FIRST: u32 = 1 << 2;
/// Selected when the active tab closes (`fallback`).
pub const PANEL_FALLBACK: u32 = 1 << 3;
/// Can't be dragged to another pane (`draggable: false`).
pub const PANEL_FIXED: u32 = 1 << 4;

/// A settings page's [`ContributionAbi::flags`]: listed in the rail's last group, beside
/// Experimental and About (`SettingsPage.group: "about"`).
pub const SETTINGS_PAGE_ABOUT: u32 = 1 << 0;

#[repr(C)]
pub struct PanelSpecAbi<'a> {
    pub id: AbiStr<'a>,
    pub title: AbiStr<'a>,
    /// The launcher card's line under the title.
    pub description: AbiStr<'a>,
    /// An icon the plugin registered (`intern_icon`) or a host icon by name (any of Desktop's
    /// icon catalog: `"code"`, `"globe"`, `"window-analytics"` …).
    pub icon: AbiStr<'a>,
    pub order: i32,
    pub flags: u32,
    pub render: RenderCallback,
    /// Draws the tab's label in the strip ([`SURFACE_TAB`]), in place of its icon and title.
    pub label: RenderCallback,
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

/// [`ContributionAbi::kind`]: the contribution points beyond commands, panels and settings
/// rows, as Desktop's plugin API names them.
pub mod contrib {
    /// `target`: `session.panel` (the side panel's `+` menu and New tab cards), `server.add`,
    /// `server.row`. `run` gets the target's id as the event's text.
    pub const MENU_ITEM: u32 = 1;
    /// The app menu: `target` is `app`, `file`, `edit`, `view`, `go`, `window` or `help`.
    pub const MENUBAR_ITEM: u32 = 2;
    /// A Settings page (`target` empty) or a section on a host page (`general`, `servers`);
    /// `render` draws its body; `keywords` lines are its search entries (`id|title|keywords`).
    pub const SETTINGS_PAGE: u32 = 3;
    /// Opens links: `order` is its priority (highest wins), `query` matches an href, `run`
    /// opens it (the href is the event's text).
    pub const LINK_HANDLER: u32 = 4;
    /// A pill at the titlebar's trailing end: `render` draws it, `run` presses it.
    pub const TITLEBAR_ITEM: u32 = 5;
    /// A capsule above the root session's composer; `render` returns `ElHandle::NONE` to hide.
    pub const COMPOSER_PILL: u32 = 6;
    /// A hover button beside a code block's Copy; `target` lists languages (empty for all).
    pub const CODE_BLOCK_ACTION: u32 = 7;
    /// A fixed shell slot: `target` is `window.bottom`, `session.header`,
    /// `session.location.menu` or `session.panel.end`.
    pub const SLOT: u32 = 8;
    /// A colour theme: `target` is its JSON (`light` and `dark` token maps).
    pub const THEME: u32 = 9;
    /// A file viewer: `target` lists file extensions; `render` draws the file at `props.path`.
    pub const FILE_VIEWER: u32 = 10;
    /// A source of servers; its entries arrive through `HostApiVtable::set_servers`.
    pub const SERVER: u32 = 11;
    /// A tool on the app's client MCP: `target` is its JSON input schema; `query` runs it with
    /// the arguments' JSON and answers JSON (or text).
    pub const MCP_TOOL: u32 = 12;
    /// Replaces finished fenced code blocks: `target` lists languages; `render` draws one.
    pub const CODE_BLOCK: u32 = 13;
    /// A popover the host places beside an element and dismisses on a press outside or Esc:
    /// `render` draws its content ([`super::SURFACE_POPOVER`]), `order` is its gap. Opened
    /// with [`super::REQUEST_TOGGLE_POPOVER`].
    pub const POPOVER: u32 = 14;
}

/// [`ContributionAbi::flags`] of a [`contrib::POPOVER`]: where it sits beside its anchor, as
/// [`ElSpec::anchor_side`] (low byte) and [`ElSpec::anchor_align`] (second byte) name them.
pub const fn popover_flags(side: u32, align: u32) -> u32 {
    (side & 0xff) | ((align & 0xff) << 8)
}

/// One contribution of any kind in [`contrib`]; fields a kind doesn't use stay empty.
#[repr(C)]
pub struct ContributionAbi<'a> {
    pub kind: u32,
    pub flags: u32,
    pub order: i32,
    pub _pad: u32,
    pub id: AbiStr<'a>,
    pub title: AbiStr<'a>,
    pub description: AbiStr<'a>,
    pub keywords: AbiStr<'a>,
    pub icon: AbiStr<'a>,
    pub target: AbiStr<'a>,
    pub run: Callback,
    pub render: RenderCallback,
    pub query: QueryCallback,
}

/// An SVG icon the host copies into its icon pool, so nothing it draws points into a library a
/// reload unmaps. `body` is the SVG's inner markup, `view` its square viewBox.
#[repr(C)]
pub struct IconSpecAbi<'a> {
    pub name: AbiStr<'a>,
    pub body: AbiStr<'a>,
    pub view: f32,
}

pub const LIVE_PENDING: u32 = 0;
pub const LIVE_ACTIVE: u32 = 1;
pub const LIVE_DISABLED: u32 = 2;
pub const LIVE_FAILED: u32 = 3;
/// The provider's type doesn't match the consumer's (its id, version or layout differ).
pub const LIVE_MISMATCH: u32 = 4;

/// A contract's state as a consumer sees it. `value` is the provider's value while `Active`,
/// valid until the consumer's callback returns.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct LiveAbi {
    pub state: u32,
    pub generation: u32,
    pub value: usize,
}

pub const SERVER_STOPPED: u32 = 0;
pub const SERVER_STARTING: u32 = 1;
pub const SERVER_AUTH: u32 = 2;
pub const SERVER_READY: u32 = 3;
pub const SERVER_FAILED: u32 = 4;

/// A server a `contrib::SERVER` source lists.
#[repr(C)]
pub struct ServerEntryAbi<'a> {
    pub id: AbiStr<'a>,
    pub name: AbiStr<'a>,
    pub state: u32,
    pub _pad: u32,
    /// Its HTTP address once ready.
    pub url: AbiStr<'a>,
}

/// [`HostApiVtable::request`]: what a handler asks the app to do once it returns.
pub const REQUEST_RUN_COMMAND: u32 = 0;
pub const REQUEST_OPEN_LINK: u32 = 1;
pub const REQUEST_OPEN_SETTINGS: u32 = 2;
/// Opens this plugin's panel `arg` in the open session's side region, or closes it when it
/// shows there (`layout.toggle`).
pub const REQUEST_TOGGLE_PANEL: u32 = 3;
/// Opens this plugin's popover `id` beside the element keyed `anchor`, or closes it when it's
/// open: `arg` is `id\nanchor`.
pub const REQUEST_TOGGLE_POPOVER: u32 = 4;
/// Closes this plugin's open popover.
pub const REQUEST_CLOSE_POPOVER: u32 = 5;
/// A toast that stays until dismissed, for a failure: `arg` is `title\ndescription`.
pub const REQUEST_TOAST_ERROR: u32 = 6;
/// Drops this plugin's cached server reads whose path starts with `arg`, so the next read
/// fetches them again.
pub const REQUEST_INVALIDATE: u32 = 7;
/// Opens a new session's draft in folder `directory` with `prompt` typed (`layout.draft`):
/// `arg` is `directory\nprompt`.
pub const REQUEST_NEW_DRAFT: u32 = 8;

/// [`HostApiVtable::host_state`] questions.
/// This plugin's panel `arg` in session `arg2`: 0 closed, 1 in the strip, 2 selected in its
/// pane, 3 selected and on screen.
pub const STATE_PANEL: u32 = 0;
/// 1 while this plugin's popover `arg` is open.
pub const STATE_POPOVER: u32 = 1;
/// How many servers the app knows.
pub const STATE_SERVERS: u32 = 2;
/// 1 when the session's server runs on this machine.
pub const STATE_SERVER_LOCAL: u32 = 3;
/// The wall clock, in ms since the Unix epoch.
pub const STATE_NOW_MS: u32 = 4;
/// 1 when the app may write to its server (live, not read-only).
pub const STATE_CAN_WRITE: u32 = 5;
/// The local time zone's offset from UTC in seconds (as an `i64`'s bits) at the instant `arg`
/// (ms since the Unix epoch, in decimal).
pub const STATE_UTC_OFFSET: u32 = 6;

/// A step's tokens.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TokensAbi {
    pub input: u64,
    pub output: u64,
    pub reasoning: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

/// [`SessionAbi::flags`].
/// A step after the last compaction and before a staged revert reported its tokens (`usage`).
pub const SESSION_HAS_USAGE: u32 = 1 << 0;
/// It runs now (a subagent of it may be the one running).
pub const SESSION_BUSY: u32 = 1 << 1;
/// Its messages have loaded.
pub const SESSION_LOADED: u32 = 1 << 2;

/// A session as the app holds it ([`HostApiVtable::session`]). The strings are views valid
/// until the next host call.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SessionAbi<'a> {
    pub id: AbiStr<'a>,
    pub title: AbiStr<'a>,
    pub directory: AbiStr<'a>,
    pub project_id: AbiStr<'a>,
    pub parent_id: AbiStr<'a>,
    /// The model it runs with.
    pub provider_id: AbiStr<'a>,
    pub model_id: AbiStr<'a>,
    /// The model of the step `usage` came from.
    pub usage_provider_id: AbiStr<'a>,
    pub usage_model_id: AbiStr<'a>,
    pub created_ms: i64,
    pub updated_ms: i64,
    /// What it cost, its subagents' sessions included.
    pub cost: f64,
    /// The last step with tokens, after the last finished compaction and before a staged revert
    /// (Desktop's `lastAssistantWithUsage`).
    pub usage: TokensAbi,
    /// Changes whenever anything here does: a memo's version.
    pub version: u64,
    /// How many messages it has loaded.
    pub messages: u32,
    pub flags: u32,
}

impl Default for SessionAbi<'_> {
    fn default() -> Self {
        let e = AbiStr::EMPTY;
        SessionAbi {
            id: e,
            title: e,
            directory: e,
            project_id: e,
            parent_id: e,
            provider_id: e,
            model_id: e,
            usage_provider_id: e,
            usage_model_id: e,
            created_ms: 0,
            updated_ms: 0,
            cost: 0.0,
            usage: TokensAbi::default(),
            version: 0,
            messages: 0,
            flags: 0,
        }
    }
}

/// [`ModelAbi::flags`].
/// The catalog lists the model (not deprecated).
pub const MODEL_KNOWN: u32 = 1 << 0;
/// The catalog lists its provider.
pub const MODEL_PROVIDER_KNOWN: u32 = 1 << 1;
/// Some of its tokens cost money.
pub const MODEL_PAID: u32 = 1 << 2;

/// A model as the server's catalog names it ([`HostApiVtable::model`]).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ModelAbi<'a> {
    pub provider_name: AbiStr<'a>,
    pub name: AbiStr<'a>,
    /// Its context window in tokens; 0 when the catalog doesn't say.
    pub context_limit: u64,
    pub flags: u32,
    pub _pad: u32,
}

impl Default for ModelAbi<'_> {
    fn default() -> Self {
        ModelAbi { provider_name: AbiStr::EMPTY, name: AbiStr::EMPTY, context_limit: 0, flags: 0, _pad: 0 }
    }
}

pub const RESOURCE_LOADING: u32 = 0;
pub const RESOURCE_READY: u32 = 1;
pub const RESOURCE_FAILED: u32 = 2;

/// A server read the host caches for a plugin ([`HostApiVtable::resource`]).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ResourceAbi<'a> {
    pub state: u32,
    /// Grows each time an answer lands: a memo's version.
    pub generation: u32,
    /// The answer's JSON once ready (the last one while a newer read loads), the error once
    /// failed. Valid until the next host call.
    pub body: AbiStr<'a>,
}

impl Default for ResourceAbi<'_> {
    fn default() -> Self {
        ResourceAbi { state: RESOURCE_LOADING, generation: 0, body: AbiStr::EMPTY }
    }
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
    // Setup (app or window).
    pub add_contribution: unsafe extern "C" fn(ctx: *mut c_void, spec: *const ContributionAbi) -> AbiStatus,
    /// Publishes `value` as contract `id` (only plugin `x` may publish `x` or `x.<name>`); the
    /// host owns it from here and frees it with `drop_value` (in this library) when replaced.
    pub provide: unsafe extern "C" fn(ctx: *mut c_void, id: AbiStr, type_hash: u64, value: usize, drop_value: Option<DropFn>) -> AbiStatus,
    /// A handle to contract `id`, whatever its provider's state; read it with `live`.
    pub consume: unsafe extern "C" fn(ctx: *mut c_void, id: AbiStr, type_hash: u64) -> u32,
    // Any callback on the app's thread.
    /// Copies an SVG icon into the host's pool; the id goes in `ElSpec::icon_id`.
    pub intern_icon: unsafe extern "C" fn(ctx: *mut c_void, spec: *const IconSpecAbi) -> u32,
    pub live: unsafe extern "C" fn(ctx: *mut c_void, handle: u32, out: *mut LiveAbi),
    /// Writes a view of store `name`'s value (for `session`, or global when empty), valid until
    /// the next host call, into `out`; 0 when unset.
    pub store_get: unsafe extern "C" fn(ctx: *mut c_void, name: AbiStr, session: AbiStr, out: *mut AbiStr<'static>) -> u8,
    // Setup, handlers and poll.
    pub store_set: unsafe extern "C" fn(ctx: *mut c_void, name: AbiStr, session: AbiStr, value: AbiStr) -> AbiStatus,
    /// Runs `job` on a host worker thread after `delay_ms`, keeping this library mapped until it
    /// returns. The job runs with a null state and context.
    pub spawn: unsafe extern "C" fn(ctx: *mut c_void, job: Callback, delay_ms: u64) -> AbiStatus,
    /// Replaces the entries of this plugin's server source `source`.
    pub set_servers: unsafe extern "C" fn(ctx: *mut c_void, source: AbiStr, entries: *const ServerEntryAbi, len: usize),
    /// One of the `REQUEST_*` kinds, carried out after the handler returns.
    pub request: unsafe extern "C" fn(ctx: *mut c_void, kind: u32, arg: AbiStr),
    // Render.
    /// Writes session `id` as the app holds it into `out` (views valid until the next host
    /// call); 0 when the app doesn't know it.
    pub session: unsafe extern "C" fn(ctx: *mut c_void, id: AbiStr, out: *mut SessionAbi<'static>) -> u8,
    /// Writes how the server's catalog names model `model` of provider `provider` into `out`.
    pub model: unsafe extern "C" fn(ctx: *mut c_void, provider: AbiStr, model: AbiStr, out: *mut ModelAbi<'static>) -> u8,
    /// Reads `GET path` from the session's server, cached for this plugin under `path` until
    /// `version` changes (or [`REQUEST_INVALIDATE`]); a read that isn't ready starts one and
    /// redraws when it lands. A run without a server stays loading.
    pub resource: unsafe extern "C" fn(ctx: *mut c_void, path: AbiStr, version: u64, out: *mut ResourceAbi<'static>),
    /// Writes theme token `name` (Desktop's CSS name without `--`, such as `text-strong` or
    /// `background-bg-layer-04`) of the theme being drawn into `out`; 0 when it has none.
    pub token: unsafe extern "C" fn(ctx: *mut c_void, name: AbiStr, out: *mut Rgba) -> u8,
    // Render, handlers and poll.
    /// Answers one of the `STATE_*` questions.
    pub host_state: unsafe extern "C" fn(ctx: *mut c_void, what: u32, arg: AbiStr, arg2: AbiStr) -> u64,
    // Handlers and poll.
    /// Sends `method path` (with a JSON `body`, or none when empty) to the server off the
    /// app's thread; `done` runs on it with [`EVENT_RESPONSE`] once it answers.
    pub server_request: unsafe extern "C" fn(ctx: *mut c_void, method: AbiStr, path: AbiStr, body: AbiStr, done: Callback) -> AbiStatus,
    /// Saves `content` as `name` where the app saves downloads (`system.save`); `done` runs
    /// with [`EVENT_SAVED`] once it's written.
    pub save_file: unsafe extern "C" fn(ctx: *mut c_void, name: AbiStr, content: AbiStr, done: Callback) -> AbiStatus,
}

pub type SetupFn = unsafe extern "C" fn(ctx: *mut c_void) -> AbiOutcome<*mut c_void>;
pub type DropStateFn = unsafe extern "C" fn(state: *mut c_void) -> AbiOutcome<u8>;
pub type PollFn = unsafe extern "C" fn(state: *mut c_void, ctx: *mut c_void) -> AbiOutcome<bool>;

/// What the host calls in a plugin, besides the callbacks it registered. A plugin has an app
/// half (one per process: services, workers, the app menu), a window half (one per window: its
/// UI), or both.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PluginVtable {
    /// Builds the process-wide state; publishes services and app menu items through `ctx`.
    pub app_setup: Option<SetupFn>,
    pub app_drop_state: Option<DropStateFn>,
    pub app_poll: Option<PollFn>,
    /// Builds the per-window state; registers commands, panels and the rest through `ctx`.
    pub window_setup: Option<SetupFn>,
    pub drop_state: Option<DropStateFn>,
    /// Runs only when the plugin's waker fired or its `wake_after` came due; `true` when
    /// visible state changed.
    pub poll: Option<PollFn>,
    /// Ephemeral window state carried across a hot reload.
    pub save_hot_state: Option<unsafe extern "C" fn(state: *const c_void, out: *mut AbiOwnedBytes) -> AbiOutcome<u8>>,
    pub load_hot_state: Option<unsafe extern "C" fn(state: *mut c_void, bytes: AbiStr) -> AbiOutcome<u8>>,
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
