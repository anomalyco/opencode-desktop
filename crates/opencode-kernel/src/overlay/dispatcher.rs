//! Declarative overlay rendering, key-dispatch, and page-Escape registry.
//!
//! Mirrors [`crate::LifecycleScheduler`]: the host shell declares named [`OverlayLayer`]s (drawn
//! bottom-to-top), [`KeyBand`]s (dispatched first-to-last), and [`EscapeBand`]s (dismissed
//! first-to-last). Features target those named bands and add explicit ordering edges (`above`,
//! `after_key`, `after`) only where intra-band precedence matters. [`OverlayDispatcher::resolve`]
//! validates every declaration once at start-up (failing loudly on any missing band, duplicate id,
//! backward edge, or cycle) and pre-sorts every dispatch slice so per-frame rendering and per-key
//! handling allocate and sort nothing.

use std::fmt;
use std::ops::Range;
use std::sync::Arc;

use super::OverlayEffect;
use super::resolve::resolve_table;

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

/// A named key-dispatch band declared by the host in execution order (first band runs first).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyBand {
    pub name: &'static str,
}

impl KeyBand {
    /// Creates a key-dispatch band descriptor.
    pub const fn new(name: &'static str) -> Self {
        Self { name }
    }
}

/// A named visual stacking layer declared by the host in bottom-to-top drawing order.
///
/// Each layer names its `default_key_band` so an overlay registering a key handler inherits the
/// key band matching its visual tier automatically unless it overrides with
/// [`OverlayRegistration::with_key_in`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OverlayLayer {
    pub name: &'static str,
    pub default_key_band: KeyBand,
}

impl OverlayLayer {
    /// Creates a visual stacking layer descriptor with its paired default [`KeyBand`].
    pub const fn new(name: &'static str, default_key_band: KeyBand) -> Self {
        Self { name, default_key_band }
    }
}

/// A named page-Escape dismissal band declared by the host in dismissal order (first band
/// dismisses first).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EscapeBand {
    pub name: &'static str,
}

impl EscapeBand {
    /// Creates a page-Escape band descriptor.
    pub const fn new(name: &'static str) -> Self {
        Self { name }
    }
}

/// Registration descriptor for a dismissible page layer handled on Escape.
pub struct EscapeLayer<S> {
    pub id: &'static str,
    pub band: EscapeBand,
    pub after: Vec<&'static str>,
    pub is_open: EscapePredicateFn<S>,
    pub on_escape: EscapeActionFn<S>,
}

impl<S> Clone for EscapeLayer<S> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            band: self.band,
            after: self.after.clone(),
            is_open: self.is_open,
            on_escape: self.on_escape,
        }
    }
}

impl<S> EscapeLayer<S> {
    /// Creates an Escape layer targeting `band`.
    pub fn new(
        id: &'static str,
        band: EscapeBand,
        is_open: EscapePredicateFn<S>,
        on_escape: EscapeActionFn<S>,
    ) -> Self {
        Self { id, band, after: Vec::new(), is_open, on_escape }
    }

    /// Declares that this Escape layer is checked after `dep` (`dep` dismisses first).
    pub fn after(mut self, dep: &'static str) -> Self {
        self.after.push(dep);
        self
    }

    /// Declares that this Escape layer is checked after every layer in `deps`.
    pub fn after_all(mut self, deps: &[&'static str]) -> Self {
        self.after.extend_from_slice(deps);
        self
    }
}

/// Full registration descriptor for a kernel-managed overlay (render, key dispatch, and Escape
/// dismiss).
pub struct OverlayRegistration<S> {
    pub id: &'static str,
    pub layer: Option<OverlayLayer>,
    pub above: Vec<&'static str>,
    pub key_band: Option<KeyBand>,
    pub after_key: Vec<&'static str>,
    pub on_dismiss: Option<OverlayDismissFn<S>>,
    pub on_key: Option<OverlayKeyFn<S>>,
    pub render: Option<OverlayRenderFn<S>>,
}

impl<S> Clone for OverlayRegistration<S> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            layer: self.layer,
            above: self.above.clone(),
            key_band: self.key_band,
            after_key: self.after_key.clone(),
            on_dismiss: self.on_dismiss,
            on_key: self.on_key,
            render: self.render,
        }
    }
}

impl<S> OverlayRegistration<S> {
    /// Creates an empty registration for `id`. Attach `.with_render(layer, ...)`,
    /// `.with_key(...)` / `.with_key_in(band, ...)`, and/or `.with_dismiss(...)`.
    pub fn new(id: &'static str) -> Self {
        Self {
            id,
            layer: None,
            above: Vec::new(),
            key_band: None,
            after_key: Vec::new(),
            on_dismiss: None,
            on_key: None,
            render: None,
        }
    }

    /// Creates a standard dialog registration in `layer` with render, optional key, and optional
    /// dismiss callbacks. Key handling inherits `layer.default_key_band`.
    pub fn dialog(
        id: &'static str,
        layer: OverlayLayer,
        render: OverlayRenderFn<S>,
        on_key: Option<OverlayKeyFn<S>>,
        on_dismiss: Option<OverlayDismissFn<S>>,
    ) -> Self {
        Self {
            id,
            layer: Some(layer),
            above: Vec::new(),
            key_band: None,
            after_key: Vec::new(),
            on_dismiss,
            on_key,
            render: Some(render),
        }
    }

    /// Attaches a render callback targeting visual `layer`.
    pub fn with_render(mut self, layer: OverlayLayer, render: OverlayRenderFn<S>) -> Self {
        self.layer = Some(layer);
        self.render = Some(render);
        self
    }

    /// Declares that this overlay draws above (after) `below_id`.
    ///
    /// `below_id` must be a renderable overlay in the same or an earlier [`OverlayLayer`].
    pub fn above(mut self, below_id: &'static str) -> Self {
        self.above.push(below_id);
        self
    }

    /// Attaches a key handler inheriting this overlay's [`OverlayLayer::default_key_band`].
    ///
    /// Panics on [`OverlayDispatcher::resolve`] if this overlay has no visual layer; use
    /// [`Self::with_key_in`] for non-overlay handlers such as full-page screens.
    pub fn with_key(mut self, on_key: OverlayKeyFn<S>) -> Self {
        self.on_key = Some(on_key);
        self
    }

    /// Attaches a key handler targeting an explicit [`KeyBand`], overriding any
    /// [`OverlayLayer::default_key_band`].
    pub fn with_key_in(mut self, band: KeyBand, on_key: OverlayKeyFn<S>) -> Self {
        self.key_band = Some(band);
        self.on_key = Some(on_key);
        self
    }

    /// Declares that this overlay's key handler runs after `earlier_id`'s key handler.
    ///
    /// `earlier_id` must be a keyed overlay in the same or an earlier [`KeyBand`].
    pub fn after_key(mut self, earlier_id: &'static str) -> Self {
        self.after_key.push(earlier_id);
        self
    }

    /// Attaches an `OverlayStack` Escape-dismissal callback.
    pub fn with_dismiss(mut self, on_dismiss: OverlayDismissFn<S>) -> Self {
        self.on_dismiss = Some(on_dismiss);
        self
    }
}

pub(super) struct RenderedEntry<S> {
    pub(super) id: &'static str,
    pub(super) layer: &'static str,
    pub(super) render: OverlayRenderFn<S>,
}

impl<S> Clone for RenderedEntry<S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S> Copy for RenderedEntry<S> {}

pub(super) struct KeyedEntry<S> {
    pub(super) id: &'static str,
    pub(super) band: &'static str,
    pub(super) on_key: OverlayKeyFn<S>,
}

impl<S> Clone for KeyedEntry<S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S> Copy for KeyedEntry<S> {}

pub(super) struct EscapeEntry<S> {
    pub(super) id: &'static str,
    pub(super) band: &'static str,
    pub(super) is_open: EscapePredicateFn<S>,
    pub(super) on_escape: EscapeActionFn<S>,
}

impl<S> Clone for EscapeEntry<S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S> Copy for EscapeEntry<S> {}

pub(super) struct DispatchTable<S> {
    pub(super) layers: Vec<OverlayLayer>,
    pub(super) key_bands: Vec<KeyBand>,
    pub(super) escape_bands: Vec<EscapeBand>,
    pub(super) pending_overlays: Vec<OverlayRegistration<S>>,
    pub(super) pending_escape: Vec<EscapeLayer<S>>,
    pub(super) rendered: Vec<RenderedEntry<S>>,
    pub(super) rendered_ranges: Vec<(OverlayLayer, Range<usize>)>,
    pub(super) keyed: Vec<KeyedEntry<S>>,
    pub(super) keyed_ranges: Vec<(KeyBand, Range<usize>)>,
    pub(super) escape_resolved: Vec<EscapeEntry<S>>,
    pub(super) dismiss: Vec<(&'static str, OverlayDismissFn<S>)>,
    pub(super) resolved: bool,
}

impl<S> Default for DispatchTable<S> {
    fn default() -> Self {
        Self {
            layers: Vec::new(),
            key_bands: Vec::new(),
            escape_bands: Vec::new(),
            pending_overlays: Vec::new(),
            pending_escape: Vec::new(),
            rendered: Vec::new(),
            rendered_ranges: Vec::new(),
            keyed: Vec::new(),
            keyed_ranges: Vec::new(),
            escape_resolved: Vec::new(),
            dismiss: Vec::new(),
            resolved: true,
        }
    }
}

impl<S> Clone for DispatchTable<S> {
    fn clone(&self) -> Self {
        Self {
            layers: self.layers.clone(),
            key_bands: self.key_bands.clone(),
            escape_bands: self.escape_bands.clone(),
            pending_overlays: self.pending_overlays.clone(),
            pending_escape: self.pending_escape.clone(),
            rendered: self.rendered.clone(),
            rendered_ranges: self.rendered_ranges.clone(),
            keyed: self.keyed.clone(),
            keyed_ranges: self.keyed_ranges.clone(),
            escape_resolved: self.escape_resolved.clone(),
            dismiss: self.dismiss.clone(),
            resolved: self.resolved,
        }
    }
}

/// Registry mapping overlay IDs to host dismiss, key-dispatch, and render callbacks, plus
/// band-ordered dismissible page Escape layers.
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

impl<S> OverlayDispatcher<S> {
    /// Creates an empty overlay dispatcher with no registered handlers.
    pub const fn new() -> Self {
        Self { inner: None }
    }

    fn table_mut(&mut self) -> &mut DispatchTable<S> {
        Arc::make_mut(self.inner.get_or_insert_with(|| Arc::new(DispatchTable::default())))
    }

    /// Declares visual stacking layers in bottom-to-top drawing order.
    pub fn add_layers(&mut self, layers: &[OverlayLayer]) {
        let table = self.table_mut();
        table.layers.extend_from_slice(layers);
        table.resolved = false;
    }

    /// Declares key-dispatch bands in execution order (first band runs first).
    pub fn add_key_bands(&mut self, bands: &[KeyBand]) {
        let table = self.table_mut();
        table.key_bands.extend_from_slice(bands);
        table.resolved = false;
    }

    /// Declares page-Escape bands in dismissal order (first band dismisses first).
    pub fn add_escape_bands(&mut self, bands: &[EscapeBand]) {
        let table = self.table_mut();
        table.escape_bands.extend_from_slice(bands);
        table.resolved = false;
    }

    /// Registers a full overlay descriptor. Duplicate IDs panic on [`Self::resolve`].
    pub fn register_overlay(&mut self, reg: OverlayRegistration<S>) {
        let table = self.table_mut();
        table.pending_overlays.push(reg);
        table.resolved = false;
    }

    /// Registers a dismissible page Escape layer. Duplicate IDs panic on [`Self::resolve`].
    pub fn register_escape_layer(&mut self, layer: EscapeLayer<S>) {
        let table = self.table_mut();
        table.pending_escape.push(layer);
        table.resolved = false;
    }

    /// Registers a dismiss callback for overlay `id`, preserving any existing render/key handlers.
    pub fn register(&mut self, id: &'static str, on_dismiss: OverlayDismissFn<S>) {
        let table = self.table_mut();
        if let Some(existing) = table.pending_overlays.iter_mut().find(|entry| entry.id == id) {
            existing.on_dismiss = Some(on_dismiss);
            table.resolved = false;
            return;
        }
        table.pending_overlays.push(OverlayRegistration::new(id).with_dismiss(on_dismiss));
        table.resolved = false;
    }

    /// Validates all declared layers, key bands, Escape bands, registrations, and ordering edges,
    /// then resolves every list once into deterministic, registration-order-independent slices.
    pub fn resolve(&mut self) {
        let Some(inner) = self.inner.as_mut() else { return };
        let table = Arc::make_mut(inner);
        if table.resolved {
            return;
        }
        resolve_table(table);
        table.resolved = true;
    }

    /// Looks up the dismiss callback registered for `id`.
    pub fn get(&self, id: &str) -> Option<OverlayDismissFn<S>> {
        let table = self.inner.as_deref()?;
        if table.resolved {
            return table.dismiss.iter().find(|(k, _)| *k == id).map(|(_, f)| *f);
        }
        table.pending_overlays.iter().find(|entry| entry.id == id).and_then(|entry| entry.on_dismiss)
    }

    /// Returns `true` if a registration exists for `id`.
    pub fn contains(&self, id: &str) -> bool {
        self.inner.as_deref().is_some_and(|t| t.pending_overlays.iter().any(|entry| entry.id == id))
    }

    /// Number of registered overlays.
    pub fn len(&self) -> usize {
        self.inner.as_deref().map_or(0, |t| t.pending_overlays.len())
    }

    /// Returns `true` when no overlays are registered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Iterates registered overlay IDs in registration order.
    pub fn ids(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.inner.as_deref().into_iter().flat_map(|t| t.pending_overlays.iter().map(|entry| entry.id))
    }

    /// Resolved `(layer_name, overlay_id)` render sequence from bottom to top.
    pub fn resolved_render(&self) -> impl Iterator<Item = (&'static str, &'static str)> + '_ {
        if let Some(table) = self.inner.as_deref() {
            assert!(table.resolved, "OverlayDispatcher: call resolve() before inspecting render order");
        }
        self.inner.as_deref().into_iter().flat_map(|t| t.rendered.iter().map(|e| (e.layer, e.id)))
    }

    /// Resolved `(band_name, overlay_id)` key-dispatch sequence from first to last.
    pub fn resolved_keys(&self) -> impl Iterator<Item = (&'static str, &'static str)> + '_ {
        if let Some(table) = self.inner.as_deref() {
            assert!(table.resolved, "OverlayDispatcher: call resolve() before inspecting key order");
        }
        self.inner.as_deref().into_iter().flat_map(|t| t.keyed.iter().map(|e| (e.band, e.id)))
    }

    /// Resolved `(band_name, escape_id)` page-Escape sequence from first to last dismissed.
    pub fn resolved_escape(&self) -> impl Iterator<Item = (&'static str, &'static str)> + '_ {
        if let Some(table) = self.inner.as_deref() {
            assert!(table.resolved, "OverlayDispatcher: call resolve() before inspecting Escape order");
        }
        self.inner.as_deref().into_iter().flat_map(|t| t.escape_resolved.iter().map(|e| (e.band, e.id)))
    }

    /// Invokes the dismiss callback registered for `id`, returning `true` if one was found.
    pub fn dispatch(&self, id: &str, state: &mut S, tx: &mut rune::Tx) -> bool {
        let Some(on_dismiss) = self.get(id) else {
            return false;
        };
        on_dismiss(state, tx);
        true
    }

    /// Dispatches a key event across pre-sorted overlay key handlers in declared `band`.
    /// Panics if `band` was not declared or the dispatcher has not been resolved.
    pub fn handle_key_band(
        &self,
        band: KeyBand,
        state: &mut S,
        ev: &rune::KeyEvent,
        tx: &mut rune::Tx,
    ) -> bool {
        let Some(table) = self.inner.as_deref() else {
            return false;
        };
        assert!(
            table.resolved,
            "OverlayDispatcher: unresolved registrations; call resolve() or KernelBuilder::build() before dispatching"
        );
        let range = table
            .keyed_ranges
            .iter()
            .find(|(b, _)| *b == band)
            .map(|(_, r)| r.clone())
            .unwrap_or_else(|| panic!("OverlayDispatcher: undeclared key band '{}'", band.name));
        for entry in &table.keyed[range] {
            if (entry.on_key)(state, ev, tx) {
                return true;
            }
        }
        false
    }

    /// Renders all pre-sorted registered overlays in declared `layer` (bottom-to-top).
    /// Panics if `layer` was not declared or the dispatcher has not been resolved.
    pub fn render_layer(&self, layer: OverlayLayer, state: &S, cx: &mut rune::Cx<S>) -> Vec<rune::El> {
        let Some(table) = self.inner.as_deref() else {
            return Vec::new();
        };
        assert!(
            table.resolved,
            "OverlayDispatcher: unresolved registrations; call resolve() or KernelBuilder::build() before rendering"
        );
        let range = table
            .rendered_ranges
            .iter()
            .find(|(l, _)| *l == layer)
            .map(|(_, r)| r.clone())
            .unwrap_or_else(|| panic!("OverlayDispatcher: undeclared overlay layer '{}'", layer.name));
        let mut out = Vec::new();
        for entry in &table.rendered[range] {
            out.extend((entry.render)(state, cx));
        }
        out
    }

    /// Renders every pre-sorted registered overlay across all declared layers in bottom-to-top order.
    pub fn render_all(&self, state: &S, cx: &mut rune::Cx<S>) -> Vec<rune::El> {
        let Some(table) = self.inner.as_deref() else {
            return Vec::new();
        };
        assert!(
            table.resolved,
            "OverlayDispatcher: unresolved registrations; call resolve() or KernelBuilder::build() before rendering"
        );
        let mut out = Vec::new();
        for entry in &table.rendered {
            out.extend((entry.render)(state, cx));
        }
        out
    }

    /// Dismisses the first open Escape layer in resolved dismissal order.
    pub fn handle_escape(&self, state: &mut S, ev: &rune::KeyEvent, tx: &mut rune::Tx) -> bool {
        let Some(table) = self.inner.as_deref() else {
            return false;
        };
        assert!(
            table.resolved,
            "OverlayDispatcher: unresolved registrations; call resolve() or KernelBuilder::build() before dispatching"
        );
        for layer in &table.escape_resolved {
            if (layer.is_open)(state) {
                (layer.on_escape)(state, ev, tx);
                return true;
            }
        }
        false
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
        let escape_count = self.inner.as_deref().map_or(0, |t| t.pending_escape.len());
        f.debug_struct("OverlayDispatcher")
            .field("len", &self.len())
            .field("escape_layers", &escape_count)
            .finish()
    }
}
