//! The contexts a plugin's code runs with: setup, render, handlers and poll.

use core::ffi::c_void;
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;

use crate::abi::{AbiEvent, AbiOutcome, AbiPalette, AbiStr, Callback, EditorHandle, ElHandle, RenderCallback, RenderProps, WakerAbi};
use crate::boundary::{ctx, host, outcome, pack, unpack, with_ctx};
use crate::el::{El, Handler};
use crate::registry::{CommandSpec, PanelSpec, SettingSpec};

/// A plugin's per-window state, built by `setup` once the window opens.
pub trait WindowPlugin: Sized + 'static {
    fn setup(cx: &mut WindowSetupCx<Self>) -> Result<Self, PluginError>;

    /// Runs only when this plugin's waker fired or a `wake_after` came due; return `true` only
    /// when visible state changed. An idle plugin costs the host one atomic load and no call.
    fn poll(&mut self, _cx: &mut PollCx) -> bool {
        false
    }

    /// Ephemeral state to carry across a hot reload (host-owned state such as editors and
    /// open tabs survives on its own).
    fn save_hot_state(&self) -> Vec<u8> {
        Vec::new()
    }

    fn load_hot_state(&mut self, _bytes: &[u8]) {}
}

/// Why setup failed; shown on the plugin's row in Settings.
#[derive(Debug)]
pub struct PluginError(pub String);

impl PluginError {
    pub fn new(message: impl Into<String>) -> Self {
        PluginError(message.into())
    }
}

impl core::fmt::Display for PluginError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Registers this window's contributions. Each is withdrawn when the plugin unloads.
pub struct WindowSetupCx<P> {
    pub(crate) _p: PhantomData<fn(&mut P)>,
}

impl<P: WindowPlugin> WindowSetupCx<P> {
    /// A command in the palette (and its keybind). `run` may call `cx.decline()` to let the
    /// key event through.
    pub fn add_command(&mut self, spec: CommandSpec, run: impl Fn(&mut P, &mut ActionCx) + 'static) {
        let run = action_callback::<P, _>(run);
        spec.with_abi(run, |abi| {
            // SAFETY: `abi` borrows `spec` for the call; the host takes ownership of `run`.
            unsafe { (host().add_command)(ctx(), abi) }
        });
    }

    /// A tab in the session side region.
    pub fn add_panel(&mut self, spec: PanelSpec, render: impl Fn(&P, &mut ViewCx<P>, &Props) -> El + 'static) {
        let render = render_callback::<P, _>(render);
        spec.with_abi(render, |abi| {
            // SAFETY: as `add_command`.
            unsafe { (host().add_panel)(ctx(), abi) }
        });
    }

    /// A row in Settings › Plugins; `render` draws its control.
    pub fn add_setting(&mut self, spec: SettingSpec, render: impl Fn(&P, &mut ViewCx<P>, &Props) -> El + 'static) {
        let render = render_callback::<P, _>(render);
        spec.with_abi(render, |abi| {
            // SAFETY: as `add_command`.
            unsafe { (host().add_setting)(ctx(), abi) }
        });
    }

    /// A host-owned text field. Across a hot reload the n-th editor a setup creates is the
    /// same field, text and undo history included.
    pub fn create_editor(&mut self, initial: &str) -> EditorHandle {
        // SAFETY: `initial` is alive for the call.
        unsafe { (host().create_editor)(ctx(), AbiStr::new(initial)) }
    }

    /// Wakes this plugin's `poll` from any thread.
    pub fn waker(&mut self) -> PluginWaker {
        // SAFETY: called inside setup, whose context the host accepts.
        PluginWaker(unsafe { (host().waker)(ctx()) })
    }
}

/// Wakes the host's event loop and marks this plugin's poll dirty. `Send + Sync`.
pub struct PluginWaker(WakerAbi);

// SAFETY: the host's waker data is an `Arc` of atomics and a thread-safe event-loop handle.
unsafe impl Send for PluginWaker {}
// SAFETY: as above.
unsafe impl Sync for PluginWaker {}

impl PluginWaker {
    pub fn wake(&self) {
        if let Some(wake) = self.0.wake {
            // SAFETY: `data` is alive until this waker drops.
            unsafe { wake(self.0.data) };
        }
    }
}

impl Clone for PluginWaker {
    fn clone(&self) -> Self {
        // SAFETY: `clone` returns a new reference to the same host data.
        let data = self.0.clone.map_or(core::ptr::null(), |clone| unsafe { clone(self.0.data) });
        PluginWaker(WakerAbi { data, ..self.0 })
    }
}

impl Drop for PluginWaker {
    fn drop(&mut self) {
        if let Some(drop) = self.0.drop {
            // SAFETY: releases this waker's reference, once.
            unsafe { drop(self.0.data) };
        }
    }
}

/// What a render draws for.
pub struct Props {
    pub id: String,
    pub session_id: String,
    pub directory: String,
    pub width: f32,
    pub height: f32,
    pub visible: bool,
    pub focused: bool,
}

/// The theme tokens of the render running on this thread.
pub type Palette = AbiPalette;

pub(crate) fn palette() -> Palette {
    let mut p = AbiPalette::default();
    // SAFETY: `p` is a valid out pointer; the host fills it or leaves the default.
    unsafe { (host().palette)(ctx(), &mut p) };
    p
}

/// Passed to a render.
pub struct ViewCx<P> {
    _p: PhantomData<fn(&P)>,
}

impl<P: WindowPlugin> ViewCx<P> {
    pub fn pal(&self) -> Palette {
        palette()
    }

    pub fn dark(&self) -> bool {
        palette().dark != 0
    }

    /// The subtree `build` returns, rebuilt only when `version` changes. The host keeps it in
    /// the window's own memo cache under this plugin's prefix, so an unchanged frame doesn't
    /// call into the plugin at all, and an unload evicts exactly this plugin's entries. The
    /// memo keys its subtree (`p:<plugin>:<key>`), so leave the root it returns unkeyed.
    pub fn memo<F: FnOnce(&mut ViewCx<P>) -> El>(&mut self, key: &str, version: impl Hash, build: F) -> El {
        let mut h = FxHasher::default();
        version.hash(&mut h);
        let mut slot = Some(build);
        unsafe extern "C" fn tramp<P: WindowPlugin, F: FnOnce(&mut ViewCx<P>) -> El>(data: *mut c_void, ctx: *mut c_void) -> AbiOutcome<ElHandle> {
            // SAFETY: `data` is the `slot` below, alive for the duration of the host's `memo` call.
            let slot = unsafe { &mut *data.cast::<Option<F>>() };
            with_ctx(ctx, || outcome(|| slot.take().map_or(ElHandle::NONE, |build| build(&mut ViewCx { _p: PhantomData }).finish())))
        }
        // SAFETY: `slot` outlives the call; the host calls `tramp` at most once, synchronously.
        let handle = unsafe { (host().memo)(ctx(), AbiStr::new(key), h.finish(), tramp::<P, F>, (&raw mut slot).cast()) };
        El::from_handle(handle)
    }

    /// A press handler for an element.
    pub fn handler(&self, f: impl Fn(&mut P, &mut ActionCx) + 'static) -> Handler {
        Handler(action_callback::<P, _>(f))
    }

    /// The text of a host-owned field.
    pub fn editor_text(&self, editor: EditorHandle) -> String {
        editor_text(editor)
    }

    /// Changes whenever the field's text, selection, composition or focus does: put it in a
    /// memo's version.
    pub fn editor_version(&self, editor: EditorHandle) -> u64 {
        // SAFETY: any context accepts it.
        unsafe { (host().editor_version)(ctx(), editor) }
    }
}

fn editor_text(editor: EditorHandle) -> String {
    let mut out = AbiStr::EMPTY;
    // SAFETY: the host writes a view valid until its next call, which we copy before making.
    unsafe {
        if (host().editor_text)(ctx(), editor, &mut out) == 0 {
            return String::new();
        }
        out.as_str().to_owned()
    }
}

/// Passed to a handler, a command and `poll`: asks the host for things it carries out after
/// the call returns.
pub struct ActionCx {
    pub(crate) changed: bool,
    pub(crate) declined: bool,
    pub(crate) event: AbiEvent,
}

/// Passed to `WindowPlugin::poll`.
pub type PollCx = ActionCx;

impl ActionCx {
    /// The press or command that ran this.
    pub fn event(&self) -> AbiEvent {
        self.event
    }

    /// Lands this change without animation.
    pub fn instant(&mut self) {
        // SAFETY: handler and poll contexts accept it.
        unsafe { (host().instant)(ctx()) };
    }

    /// Polls this plugin again after `millis` (collapsed with the host's other timers).
    pub fn wake_after(&mut self, millis: u64) {
        // SAFETY: as `instant`.
        unsafe { (host().wake_after)(ctx(), millis) };
    }

    pub fn copy_text(&mut self, text: &str) {
        // SAFETY: as `instant`.
        unsafe { (host().copy_text)(ctx(), AbiStr::new(text)) };
    }

    /// Opens (or focuses) one of this plugin's panels in the open session's side region.
    pub fn open_panel(&mut self, panel: &str) {
        // SAFETY: as `instant`.
        unsafe { (host().open_panel)(ctx(), AbiStr::new(panel)) };
    }

    pub fn toast(&mut self, text: &str) {
        // SAFETY: as `instant`.
        unsafe { (host().toast)(ctx(), AbiStr::new(text)) };
    }

    pub fn editor_text(&self, editor: EditorHandle) -> String {
        editor_text(editor)
    }

    pub fn set_editor_text(&mut self, editor: EditorHandle, text: &str) {
        // SAFETY: as `instant`.
        unsafe { (host().editor_set_text)(ctx(), editor, AbiStr::new(text)) };
    }

    /// Nothing visible changed: the host won't redraw for this handler.
    pub fn unchanged(&mut self) {
        self.changed = false;
    }

    /// A command passes on its keybind, so the key reaches whatever else wants it.
    pub fn decline(&mut self) {
        self.declined = true;
        self.changed = false;
    }
}

pub(crate) fn action_callback<P: WindowPlugin, F: Fn(&mut P, &mut ActionCx) + 'static>(f: F) -> Callback {
    unsafe extern "C" fn tramp<P: WindowPlugin, F: Fn(&mut P, &mut ActionCx) + 'static>(
        state: *mut c_void,
        ctx: *mut c_void,
        data: usize,
        event: *const AbiEvent,
    ) -> AbiOutcome<bool> {
        let r = with_ctx(ctx, || {
            crate::boundary::guard(|| {
                // SAFETY: the host passes this plugin instance's state and the data `pack` made for `F`.
                let (state, f) = unsafe { (&mut *state.cast::<P>(), unpack::<F>(&data)) };
                // SAFETY: `event` is null or valid for the call.
                let event = unsafe { event.as_ref() }.copied().unwrap_or_default();
                let mut cx = ActionCx { changed: true, declined: false, event };
                f(state, &mut cx);
                (cx.changed, cx.declined)
            })
        });
        match r {
            Ok((changed, true)) => AbiOutcome::declined(changed),
            Ok((changed, false)) => AbiOutcome::ok(changed),
            Err(message) => AbiOutcome::panicked(message),
        }
    }
    let (data, drop_data) = pack(f);
    Callback { func: Some(tramp::<P, F>), data, drop_data }
}

pub(crate) fn render_callback<P: WindowPlugin, F: Fn(&P, &mut ViewCx<P>, &Props) -> El + 'static>(f: F) -> RenderCallback {
    unsafe extern "C" fn tramp<P: WindowPlugin, F: Fn(&P, &mut ViewCx<P>, &Props) -> El + 'static>(
        state: *const c_void,
        ctx: *mut c_void,
        data: usize,
        props: *const RenderProps,
    ) -> AbiOutcome<ElHandle> {
        with_ctx(ctx, || {
            outcome(|| {
                // SAFETY: the host passes this plugin instance's state and the data `pack` made for `F`.
                let (state, f) = unsafe { (&*state.cast::<P>(), unpack::<F>(&data)) };
                // SAFETY: `props` is valid for the call, its strings too.
                let props = unsafe { props.as_ref() }.map(|p| unsafe {
                    Props {
                        id: p.id.as_str().to_owned(),
                        session_id: p.session_id.as_str().to_owned(),
                        directory: p.directory.as_str().to_owned(),
                        width: p.width,
                        height: p.height,
                        visible: p.visible != 0,
                        focused: p.focused != 0,
                    }
                });
                let props = props.unwrap_or(Props { id: String::new(), session_id: String::new(), directory: String::new(), width: 0.0, height: 0.0, visible: true, focused: false });
                f(state, &mut ViewCx { _p: PhantomData }, &props).finish()
            })
        })
    }
    let (data, drop_data) = pack(f);
    RenderCallback { func: Some(tramp::<P, F>), data, drop_data }
}

/// FxHash, for memo versions: fast, and stable within a process.
#[derive(Default)]
pub struct FxHasher(u64);

impl Hasher for FxHasher {
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.write_u64(u64::from_le_bytes(word));
        }
    }

    fn write_u64(&mut self, v: u64) {
        self.0 = (self.0.rotate_left(5) ^ v).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}
