//! Explicit state machine for transient overlay layers (dialogs, popovers, sheets, menus, toasts,
//! and inline layers).
//!
//! Matches Desktop's layer-stack dismissal semantics (`ui/src/components/popover-machine.ts`,
//! `ui/src/context/dialog-machine.ts`): Escape dismisses only the topmost layer, and never reaches
//! past a non-escape-dismissible top layer.

use std::fmt;
use std::sync::Arc;

/// Visual and interaction category of an overlay entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OverlayKind {
    Dialog,
    Popover,
    Sheet,
    Menu,
    Toast,
    Inline,
}

/// One active layer on the overlay stack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OverlayEntry {
    pub id: &'static str,
    pub kind: OverlayKind,
    pub return_focus: Option<String>,
    pub dismiss_on_escape: bool,
}

impl OverlayEntry {
    /// Creates an entry that dismisses on Escape with no saved focus target.
    pub const fn new(id: &'static str, kind: OverlayKind) -> Self {
        Self { id, kind, return_focus: None, dismiss_on_escape: true }
    }

    /// Records the focus target to restore when this overlay is dismissed.
    pub fn with_return_focus(mut self, focus: impl Into<String>) -> Self {
        self.return_focus = Some(focus.into());
        self
    }

    /// Configures whether pressing Escape dismisses this overlay when it is topmost.
    pub const fn with_dismiss_on_escape(mut self, dismiss: bool) -> Self {
        self.dismiss_on_escape = dismiss;
        self
    }
}

/// Explicit state of the overlay stack so an empty stack cannot hold an empty `Active` vector.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum OverlayState {
    #[default]
    Empty,
    /// Non-empty stack ordered from bottom (index 0) to top (last element).
    Active(Vec<OverlayEntry>),
}

/// Input events for the overlay stack state machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OverlayEvent {
    /// Push a layer onto the top of the stack (moving it to the top if `id` is already present).
    Push(OverlayEntry),
    /// Close a specific layer by id.
    Close(&'static str),
    /// Handle an Escape key press against the topmost layer.
    Escape,
    /// Dismiss every layer matching `OverlayKind`, top-to-bottom.
    ClearKind(OverlayKind),
}

/// Side effects emitted by [`step`] for the host shell to execute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OverlayEffect {
    Dismissed {
        id: &'static str,
        restore_focus: Option<String>,
    },
    None,
}

/// Ordered stack of active overlays managed via [`step`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OverlayStack {
    state: OverlayState,
}

impl OverlayStack {
    /// Creates an empty overlay stack.
    pub const fn new() -> Self {
        Self { state: OverlayState::Empty }
    }

    /// Current explicit state of the stack.
    pub fn state(&self) -> &OverlayState {
        &self.state
    }

    /// Active entries from bottom to top.
    pub fn entries(&self) -> &[OverlayEntry] {
        match &self.state {
            OverlayState::Empty => &[],
            OverlayState::Active(entries) => entries.as_slice(),
        }
    }

    /// The topmost active overlay, if any.
    pub fn top(&self) -> Option<&OverlayEntry> {
        self.entries().last()
    }

    /// Returns `true` if a layer with `id` is currently active.
    pub fn contains(&self, id: &str) -> bool {
        self.entries().iter().any(|entry| entry.id == id)
    }

    /// Returns `true` if at least one layer of `kind` is active.
    pub fn has_kind(&self, kind: OverlayKind) -> bool {
        self.entries().iter().any(|entry| entry.kind == kind)
    }

    /// Number of active overlays.
    pub fn len(&self) -> usize {
        self.entries().len()
    }

    /// Returns `true` when no overlays are active.
    pub fn is_empty(&self) -> bool {
        matches!(self.state, OverlayState::Empty)
    }

    /// Advances the overlay state machine by one event and returns effects to apply.
    pub fn step(&mut self, event: OverlayEvent) -> Vec<OverlayEffect> {
        step(self, event)
    }
}

/// Pure transition function for [`OverlayStack`].
pub fn step(stack: &mut OverlayStack, event: OverlayEvent) -> Vec<OverlayEffect> {
    match event {
        OverlayEvent::Push(entry) => {
            match &mut stack.state {
                OverlayState::Empty => {
                    stack.state = OverlayState::Active(vec![entry]);
                }
                OverlayState::Active(entries) => {
                    entries.retain(|existing| existing.id != entry.id);
                    entries.push(entry);
                }
            }
            Vec::new()
        }
        OverlayEvent::Close(id) => {
            let OverlayState::Active(entries) = &mut stack.state else {
                return Vec::new();
            };
            let Some(index) = entries.iter().rposition(|entry| entry.id == id) else {
                return Vec::new();
            };
            let removed = entries.remove(index);
            if entries.is_empty() {
                stack.state = OverlayState::Empty;
            }
            vec![OverlayEffect::Dismissed { id: removed.id, restore_focus: removed.return_focus }]
        }
        OverlayEvent::Escape => {
            let OverlayState::Active(entries) = &mut stack.state else {
                return Vec::new();
            };
            if !entries.last().is_some_and(|top| top.dismiss_on_escape) {
                return Vec::new();
            }
            let Some(removed) = entries.pop() else {
                stack.state = OverlayState::Empty;
                return Vec::new();
            };
            if entries.is_empty() {
                stack.state = OverlayState::Empty;
            }
            vec![OverlayEffect::Dismissed { id: removed.id, restore_focus: removed.return_focus }]
        }
        OverlayEvent::ClearKind(kind) => {
            let OverlayState::Active(entries) = &mut stack.state else {
                return Vec::new();
            };
            let mut effects = Vec::new();
            let mut kept = Vec::with_capacity(entries.len());
            for entry in entries.drain(..) {
                if entry.kind == kind {
                    effects.push(OverlayEffect::Dismissed {
                        id: entry.id,
                        restore_focus: entry.return_focus,
                    });
                } else {
                    kept.push(entry);
                }
            }
            // Emit dismissals top-to-bottom so the topmost overlay restores focus first.
            effects.reverse();
            if kept.is_empty() {
                stack.state = OverlayState::Empty;
            } else {
                *entries = kept;
            }
            effects
        }
    }
}

/// Host callback invoked when an overlay is dismissed through [`OverlayDispatcher`].
pub type OverlayDismissFn<S> = fn(&mut S, &mut rune::Tx);

/// Host callback invoked to handle a key event for a registered overlay.
pub type OverlayKeyFn<S> = fn(&mut S, &rune::KeyEvent, &mut rune::Tx) -> bool;

/// Host callback invoked to render a registered overlay's elements into the window layer tree.
pub type OverlayRenderFn<S> = fn(&S, &mut rune::Cx<S>) -> Vec<rune::El>;

/// Host predicate reporting whether a dismissible page layer is currently open.
pub type EscapePredicateFn<S> = fn(&S) -> bool;

/// Host callback invoked when Escape dismisses a registered page layer.
pub type EscapeActionFn<S> = fn(&mut S, &rune::KeyEvent, &mut rune::Tx);

/// Registration descriptor for a dismissible page layer handled on Escape.
pub struct EscapeLayer<S> {
    pub id: &'static str,
    /// Higher priority dismisses first (topmost layer wins).
    pub priority: u16,
    pub is_open: EscapePredicateFn<S>,
    pub on_escape: EscapeActionFn<S>,
}

impl<S> Clone for EscapeLayer<S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S> Copy for EscapeLayer<S> {}

impl<S> EscapeLayer<S> {
    /// Creates an Escape layer descriptor where higher `priority` dismisses first.
    pub const fn new(
        id: &'static str,
        priority: u16,
        is_open: EscapePredicateFn<S>,
        on_escape: EscapeActionFn<S>,
    ) -> Self {
        Self { id, priority, is_open, on_escape }
    }
}

/// Default z-order band for standard modal dialogs registered via [`OverlayRegistration::dialog`].
pub const DEFAULT_DIALOG_Z: u16 = 450;

/// Full registration descriptor for a kernel-managed overlay (render, key dispatch, and Escape dismiss).
pub struct OverlayRegistration<S> {
    pub id: &'static str,
    pub z_order: u16,
    pub key_priority: u16,
    pub on_dismiss: Option<OverlayDismissFn<S>>,
    pub on_key: Option<OverlayKeyFn<S>>,
    pub render: Option<OverlayRenderFn<S>>,
}

impl<S> Clone for OverlayRegistration<S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S> Copy for OverlayRegistration<S> {}

impl<S> OverlayRegistration<S> {
    /// Creates an empty registration for `id` at [`DEFAULT_DIALOG_Z`].
    pub const fn new(id: &'static str) -> Self {
        Self {
            id,
            z_order: DEFAULT_DIALOG_Z,
            key_priority: DEFAULT_DIALOG_Z,
            on_dismiss: None,
            on_key: None,
            render: None,
        }
    }

    /// Creates a dialog registration with render, key, and dismiss callbacks at [`DEFAULT_DIALOG_Z`].
    pub const fn dialog(
        id: &'static str,
        render: OverlayRenderFn<S>,
        on_key: Option<OverlayKeyFn<S>>,
        on_dismiss: Option<OverlayDismissFn<S>>,
    ) -> Self {
        Self {
            id,
            z_order: DEFAULT_DIALOG_Z,
            key_priority: DEFAULT_DIALOG_Z,
            on_dismiss,
            on_key,
            render: Some(render),
        }
    }

    pub const fn with_z(mut self, z_order: u16) -> Self {
        self.z_order = z_order;
        self
    }

    pub const fn with_key_priority(mut self, key_priority: u16) -> Self {
        self.key_priority = key_priority;
        self
    }

    pub const fn with_dismiss(mut self, on_dismiss: OverlayDismissFn<S>) -> Self {
        self.on_dismiss = Some(on_dismiss);
        self
    }

    pub const fn with_key(mut self, on_key: OverlayKeyFn<S>) -> Self {
        self.on_key = Some(on_key);
        self
    }

    pub const fn with_render(mut self, render: OverlayRenderFn<S>) -> Self {
        self.render = Some(render);
        self
    }
}

struct DispatchTable<S> {
    entries: Vec<OverlayRegistration<S>>,
    render_bands: Vec<(u16, u16)>,
    key_bands: Vec<(u16, u16)>,
    rendered: Vec<OverlayRegistration<S>>,
    keyed: Vec<OverlayRegistration<S>>,
    escape_layers: Vec<EscapeLayer<S>>,
    resolved: bool,
}

impl<S> Default for DispatchTable<S> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            render_bands: Vec::new(),
            key_bands: Vec::new(),
            rendered: Vec::new(),
            keyed: Vec::new(),
            escape_layers: Vec::new(),
            resolved: true,
        }
    }
}

impl<S> Clone for DispatchTable<S> {
    fn clone(&self) -> Self {
        Self {
            entries: self.entries.clone(),
            render_bands: self.render_bands.clone(),
            key_bands: self.key_bands.clone(),
            rendered: self.rendered.clone(),
            keyed: self.keyed.clone(),
            escape_layers: self.escape_layers.clone(),
            resolved: self.resolved,
        }
    }
}

/// Registry mapping overlay IDs to host dismiss, key-dispatch, and render callbacks, plus
/// priority-ordered dismissible page Escape layers.
///
/// Cloning is O(1) (shared immutable handler lists once resolved), allowing a host that embeds
/// [`crate::Kernel<S>`] inside `S` to clone the dispatcher handle before passing `&mut S` into a
/// registered callback without borrow conflicts.
pub struct OverlayDispatcher<S> {
    inner: Option<Arc<DispatchTable<S>>>,
}

impl<S> Clone for OverlayDispatcher<S> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl<S> Default for OverlayDispatcher<S> {
    fn default() -> Self {
        Self::new()
    }
}

fn validate_bands(kind: &str, bands: &[(u16, u16)]) {
    let mut sorted = bands.to_vec();
    for &(min, max) in &sorted {
        assert!(min <= max, "overlay dispatcher: inverted {kind} band ({min}..={max})");
    }
    sorted.sort_unstable();
    for pair in sorted.windows(2) {
        let (a_min, a_max) = pair[0];
        let (b_min, b_max) = pair[1];
        assert!(
            a_max < b_min,
            "overlay dispatcher: overlapping {kind} bands ({a_min}..={a_max}) and ({b_min}..={b_max})"
        );
    }
}

impl<S> OverlayDispatcher<S> {
    /// Creates an empty overlay dispatcher with no registered handlers.
    pub const fn new() -> Self {
        Self { inner: None }
    }

    fn table_mut(&mut self) -> &mut DispatchTable<S> {
        Arc::make_mut(self.inner.get_or_insert_with(|| Arc::new(DispatchTable::default())))
    }

    /// Declares the inclusive `z_order` bands the host renders. When non-empty, [`Self::resolve`]
    /// panics if any overlay with a render callback falls outside these bands.
    pub fn add_render_bands(&mut self, bands: &[(u16, u16)]) {
        let table = self.table_mut();
        table.render_bands.extend_from_slice(bands);
        table.resolved = false;
    }

    /// Declares the inclusive `key_priority` bands the host dispatches. When non-empty,
    /// [`Self::resolve`] panics if any overlay with a key callback falls outside these bands.
    pub fn add_key_bands(&mut self, bands: &[(u16, u16)]) {
        let table = self.table_mut();
        table.key_bands.extend_from_slice(bands);
        table.resolved = false;
    }

    /// Validates all registered overlays, bands, and Escape layers and pre-sorts render, key, and
    /// Escape dispatch lists so per-frame and per-key calls allocate and sort nothing.
    pub fn resolve(&mut self) {
        let Some(inner) = self.inner.as_mut() else { return };
        let table = Arc::make_mut(inner);
        if table.resolved {
            return;
        }
        validate_bands("render", &table.render_bands);
        validate_bands("key", &table.key_bands);

        let mut seen_overlays = std::collections::HashSet::with_capacity(table.entries.len());
        for entry in &table.entries {
            assert!(
                seen_overlays.insert(entry.id),
                "overlay dispatcher: duplicate overlay id '{}'",
                entry.id
            );
            if entry.render.is_some() && !table.render_bands.is_empty() {
                assert!(
                    table.render_bands.iter().any(|&(min, max)| entry.z_order >= min && entry.z_order <= max),
                    "overlay dispatcher: overlay '{}' render z_order {} is not covered by any declared render band {:?}",
                    entry.id,
                    entry.z_order,
                    table.render_bands
                );
            }
            if entry.on_key.is_some() && !table.key_bands.is_empty() {
                assert!(
                    table.key_bands.iter().any(|&(min, max)| entry.key_priority >= min && entry.key_priority <= max),
                    "overlay dispatcher: overlay '{}' key_priority {} is not covered by any declared key band {:?}",
                    entry.id,
                    entry.key_priority,
                    table.key_bands
                );
            }
        }

        let mut seen_escape = std::collections::HashSet::with_capacity(table.escape_layers.len());
        for layer in &table.escape_layers {
            assert!(
                seen_escape.insert(layer.id),
                "overlay dispatcher: duplicate escape layer id '{}'",
                layer.id
            );
        }

        table.rendered = table.entries.iter().copied().filter(|e| e.render.is_some()).collect();
        table.rendered.sort_by_key(|e| e.z_order);

        table.keyed = table.entries.iter().copied().filter(|e| e.on_key.is_some()).collect();
        table.keyed.sort_by_key(|e| e.key_priority);

        table.escape_layers.sort_by_key(|layer| std::cmp::Reverse(layer.priority));
        table.resolved = true;
    }

    /// Registers a dismissible page Escape layer. Duplicate IDs panic on [`Self::resolve`].
    pub fn register_escape_layer(&mut self, layer: EscapeLayer<S>) {
        let table = self.table_mut();
        table.escape_layers.push(layer);
        table.resolved = false;
    }

    /// Removes a registered Escape layer by `id`, returning `true` if one was removed.
    pub fn unregister_escape_layer(&mut self, id: &str) -> bool {
        let Some(table) = self.inner.as_mut().map(Arc::make_mut) else {
            return false;
        };
        let Some(index) = table.escape_layers.iter().position(|entry| entry.id == id) else {
            return false;
        };
        table.escape_layers.remove(index);
        true
    }

    /// Registered Escape layers in descending priority order (topmost layer first).
    pub fn escape_layers(&self) -> &[EscapeLayer<S>] {
        self.inner.as_deref().map_or(&[], |t| t.escape_layers.as_slice())
    }

    /// Dismisses the highest-priority registered Escape layer whose `is_open` predicate returns
    /// `true`, invoking its `on_escape` callback and returning `true`. Returns `false` when no
    /// registered layer is open.
    pub fn handle_escape(&self, state: &mut S, ev: &rune::KeyEvent, tx: &mut rune::Tx) -> bool {
        let Some(table) = self.inner.as_deref() else {
            return false;
        };
        assert!(
            table.resolved,
            "OverlayDispatcher: unresolved registrations; call resolve() or KernelBuilder::build() before dispatching"
        );
        for layer in &table.escape_layers {
            if (layer.is_open)(state) {
                (layer.on_escape)(state, ev, tx);
                return true;
            }
        }
        false
    }

    /// Registers a full overlay descriptor (render, key, dismiss, z-order). Duplicate IDs panic on [`Self::resolve`].
    pub fn register_overlay(&mut self, reg: OverlayRegistration<S>) {
        let table = self.table_mut();
        table.entries.push(reg);
        table.resolved = false;
    }

    /// Registers a dismiss callback for overlay `id`, preserving any existing render/key handlers for `id`.
    pub fn register(&mut self, id: &'static str, on_dismiss: OverlayDismissFn<S>) {
        let table = self.table_mut();
        if let Some(existing) = table.entries.iter_mut().find(|entry| entry.id == id) {
            existing.on_dismiss = Some(on_dismiss);
            table.resolved = false;
            return;
        }
        table.entries.push(OverlayRegistration::new(id).with_dismiss(on_dismiss));
        table.resolved = false;
    }

    /// Removes the registration for `id`, returning its dismiss callback if present.
    pub fn unregister(&mut self, id: &str) -> Option<OverlayDismissFn<S>> {
        let table = self.inner.as_mut().map(Arc::make_mut)?;
        let index = table.entries.iter().position(|entry| entry.id == id)?;
        let removed = table.entries.remove(index);
        table.resolved = false;
        removed.on_dismiss
    }

    /// Looks up the dismiss callback registered for `id`.
    pub fn get(&self, id: &str) -> Option<OverlayDismissFn<S>> {
        self.inner.as_deref()?.entries.iter().find(|entry| entry.id == id).and_then(|entry| entry.on_dismiss)
    }

    /// Returns `true` if a registration exists for `id`.
    pub fn contains(&self, id: &str) -> bool {
        self.inner.as_deref().is_some_and(|t| t.entries.iter().any(|entry| entry.id == id))
    }

    /// Number of registered overlays.
    pub fn len(&self) -> usize {
        self.inner.as_deref().map_or(0, |t| t.entries.len())
    }

    /// Returns `true` when no overlays are registered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Iterates registered overlay IDs in registration order.
    pub fn ids(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.inner.as_deref().into_iter().flat_map(|t| t.entries.iter().map(|entry| entry.id))
    }

    /// Invokes the dismiss callback registered for `id`, returning `true` if one was found.
    pub fn dispatch(&self, id: &str, state: &mut S, tx: &mut rune::Tx) -> bool {
        let Some(on_dismiss) = self.get(id) else {
            return false;
        };
        on_dismiss(state, tx);
        true
    }

    /// Dispatches a key event across pre-sorted overlay key handlers whose `key_priority` falls
    /// in `min_prio..=max_prio` (ascending). Returns `true` if any overlay consumed it.
    pub fn handle_key_band(
        &self,
        state: &mut S,
        ev: &rune::KeyEvent,
        tx: &mut rune::Tx,
        min_prio: u16,
        max_prio: u16,
    ) -> bool {
        let Some(table) = self.inner.as_deref() else {
            return false;
        };
        assert!(
            table.resolved,
            "OverlayDispatcher: unresolved registrations; call resolve() or KernelBuilder::build() before dispatching"
        );
        for entry in &table.keyed {
            if entry.key_priority < min_prio {
                continue;
            }
            if entry.key_priority > max_prio {
                break;
            }
            if let Some(on_key) = entry.on_key
                && on_key(state, ev, tx)
            {
                return true;
            }
        }
        false
    }

    /// Renders all pre-sorted registered overlays whose `z_order` falls in `min_z..=max_z` (ascending).
    pub fn render_band(&self, state: &S, cx: &mut rune::Cx<S>, min_z: u16, max_z: u16) -> Vec<rune::El> {
        let Some(table) = self.inner.as_deref() else {
            return Vec::new();
        };
        assert!(
            table.resolved,
            "OverlayDispatcher: unresolved registrations; call resolve() or KernelBuilder::build() before rendering"
        );
        let mut out = Vec::new();
        for entry in &table.rendered {
            if entry.z_order < min_z {
                continue;
            }
            if entry.z_order > max_z {
                break;
            }
            if let Some(render) = entry.render {
                out.extend(render(state, cx));
            }
        }
        out
    }

    /// Applies overlay dismissal effects to `state` and `tx`: restores recorded focus via
    /// `tx.focus(...)` and invokes any registered dismiss callback. Returns `true` if any
    /// dismissal effect was processed.
    pub fn apply_effects(&self, effects: &[OverlayEffect], state: &mut S, tx: &mut rune::Tx) -> bool {
        let mut dismissed = false;
        for effect in effects {
            if let OverlayEffect::Dismissed { id, restore_focus } = effect {
                dismissed = true;
                if let Some(focus) = restore_focus {
                    tx.focus(focus.clone(), tx.focus_visible());
                }
                self.dispatch(id, state, tx);
            }
        }
        dismissed
    }
}

impl<S> fmt::Debug for OverlayDispatcher<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OverlayDispatcher")
            .field("len", &self.len())
            .field("escape_layers", &self.escape_layers().len())
            .finish()
    }
}
