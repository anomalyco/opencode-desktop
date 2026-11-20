//! Lightweight Shell Kernel for Rune apps (`rune-desktop`, Seer, and future hosts).
//!
//! Unites four orthogonal decoupling registries so features never add fields to the host `App`
//! struct or edit central switchboards:
//! - [`StateSlots`]: tracked per-window feature state storage indexed by static [`Slot`] handles.
//! - [`OverlayStack`]: explicit state machine for modal/popover/sheet/menu/toast dismissal.
//! - [`LifecycleScheduler`]: 4-phase tick scheduler plus `setup`, `rest`, `visibility`, and `close` hooks.
//! - [`CommandRegistry`]: unified palette and `/slash` command registration.

#![forbid(unsafe_code)]

pub mod commands;
pub mod feature;
pub mod host;
pub mod overlay;
pub mod scheduler;
pub mod slots;

use std::fmt;

pub use commands::{CommandEntry, CommandRegistry, CommandTitle};
pub use feature::Feature;
pub use host::{AppEnv, AppFeedback, AppOverlays};
pub use overlay::{
    EscapeActionFn, EscapeBand, EscapeLayer, EscapePredicateFn, KeyBand, OverlayDismissFn,
    OverlayDispatcher, OverlayEffect, OverlayEntry, OverlayEvent, OverlayKeyFn, OverlayKind,
    OverlayLayer, OverlayRegistration, OverlayRenderFn, OverlayStack, OverlayState,
};
pub use scheduler::{
    CloseFn, LifecycleScheduler, Phase, RestFn, Scheduled, SetupFn, Stage, System, SystemFn,
    VisibilityFn,
};
pub use slots::{Slot, StateSlots};

/// Shell kernel uniting every decoupling registry for host state `S`.
pub struct Kernel<S> {
    pub slots: StateSlots,
    pub overlays: OverlayStack,
    pub overlay_dispatcher: OverlayDispatcher<S>,
    pub scheduler: LifecycleScheduler<S>,
    pub commands: CommandRegistry<S>,
    features: Vec<&'static str>,
}

impl<S> Default for Kernel<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Kernel<S> {
    /// Creates an empty kernel with no features registered.
    pub fn new() -> Self {
        Self {
            slots: StateSlots::new(),
            overlays: OverlayStack::new(),
            overlay_dispatcher: OverlayDispatcher::new(),
            scheduler: LifecycleScheduler::new(),
            commands: CommandRegistry::new(),
            features: Vec::new(),
        }
    }

    /// Starts a [`KernelBuilder`] for assembling features into a [`Kernel`].
    pub fn builder() -> KernelBuilder<S> {
        KernelBuilder::new()
    }

    /// Identifiers of all features registered into this kernel, in registration order.
    pub fn features(&self) -> &[&'static str] {
        &self.features
    }

    /// Returns `true` if a feature with `id` was registered.
    pub fn has_feature(&self, id: &str) -> bool {
        self.features.contains(&id)
    }

    /// Advances the overlay stack state machine and returns effects for the shell to apply.
    pub fn step_overlay(&mut self, event: OverlayEvent) -> Vec<OverlayEffect> {
        overlay::step(&mut self.overlays, event)
    }

    /// Registers a dismiss callback invoked when overlay `id` is dismissed through the kernel.
    pub fn register_overlay_dismiss(&mut self, id: &'static str, on_dismiss: OverlayDismissFn<S>) {
        self.overlay_dispatcher.register(id, on_dismiss);
    }

    /// Steps the topmost overlay with [`OverlayEvent::Escape`], restores focus if recorded, and
    /// invokes any registered dismiss callback. Returns `true` when an overlay was dismissed.
    ///
    /// Hosts that embed [`Kernel<S>`] inside `S` can step [`Self::overlays`] and clone
    /// [`Self::overlay_dispatcher`] (O(1)) before calling [`OverlayDispatcher::apply_effects`].
    pub fn dismiss_top_overlay(&mut self, state: &mut S, tx: &mut rune::Tx) -> bool {
        let effects = self.step_overlay(OverlayEvent::Escape);
        if effects.is_empty() {
            return false;
        }
        self.overlay_dispatcher.apply_effects(&effects, state, tx)
    }
}

impl<S> fmt::Debug for Kernel<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Kernel")
            .field("features", &self.features)
            .field("slots", &self.slots)
            .field("overlays", &self.overlays)
            .field("overlay_dispatcher", &self.overlay_dispatcher)
            .field("scheduler", &self.scheduler)
            .field("commands", &self.commands)
            .finish()
    }
}

/// Builder for assembling features and registries into a [`Kernel`].
pub struct KernelBuilder<S> {
    kernel: Kernel<S>,
}

impl<S> Default for KernelBuilder<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> KernelBuilder<S> {
    /// Creates an empty builder.
    pub fn new() -> Self {
        Self { kernel: Kernel::new() }
    }

    /// Registers a [`Feature`] once (duplicate feature IDs are ignored).
    pub fn with_feature(&mut self, feature: &dyn Feature<S>) -> &mut Self {
        let id = feature.id();
        if self.kernel.features.contains(&id) {
            return self;
        }
        self.kernel.features.push(id);
        feature.register(self);
        self
    }

    /// Declares a named execution stage inside `stage.phase`.
    pub fn add_stage(&mut self, stage: Stage) -> &mut Self {
        self.kernel.scheduler.add_stage(stage);
        self
    }

    /// Declares multiple execution stages in order.
    pub fn add_stages(&mut self, stages: &[Stage]) -> &mut Self {
        self.kernel.scheduler.add_stages(stages);
        self
    }

    /// Registers a per-tick system targeting a declared [`Stage`], returning a [`Scheduled`]
    /// handle for chaining explicit `after` edges.
    pub fn schedule(&mut self, stage: Stage, name: &'static str, run: SystemFn<S>) -> Scheduled<'_, S> {
        self.kernel.scheduler.schedule(stage, name, run)
    }

    /// Registers a pre-built [`System`] targeting a declared [`Stage`].
    pub fn schedule_system(&mut self, system: System<S>) -> &mut Self {
        self.kernel.scheduler.schedule_system(system);
        self
    }

    /// Registers a one-time window setup hook.
    pub fn add_setup(&mut self, name: &'static str, f: SetupFn<S>) -> &mut Self {
        self.kernel.scheduler.add_setup(name, f);
        self
    }

    /// Registers an idle/rest eviction hook.
    pub fn add_rest(&mut self, name: &'static str, f: RestFn<S>) -> &mut Self {
        self.kernel.scheduler.add_rest(name, f);
        self
    }

    /// Registers a window visibility hook.
    pub fn add_visibility(&mut self, name: &'static str, f: VisibilityFn<S>) -> &mut Self {
        self.kernel.scheduler.add_visibility(name, f);
        self
    }

    /// Registers a window close hook.
    pub fn add_close(&mut self, name: &'static str, f: CloseFn<S>) -> &mut Self {
        self.kernel.scheduler.add_close(name, f);
        self
    }

    /// Registers a palette or slash command.
    pub fn add_command(&mut self, entry: CommandEntry<S>) -> &mut Self {
        self.kernel.commands.register(entry);
        self
    }

    /// Declares visual stacking layers rendered by the host in bottom-to-top order.
    pub fn add_overlay_layers(&mut self, layers: &[OverlayLayer]) -> &mut Self {
        self.kernel.overlay_dispatcher.add_layers(layers);
        self
    }

    /// Declares key-dispatch bands in execution order (first band runs first).
    pub fn add_overlay_key_bands(&mut self, bands: &[KeyBand]) -> &mut Self {
        self.kernel.overlay_dispatcher.add_key_bands(bands);
        self
    }

    /// Declares page-Escape dismissal bands in dismissal order (first band dismisses first).
    pub fn add_escape_bands(&mut self, bands: &[EscapeBand]) -> &mut Self {
        self.kernel.overlay_dispatcher.add_escape_bands(bands);
        self
    }

    /// Registers a full overlay descriptor (render, key dispatch, Escape dismiss, layer).
    pub fn register_overlay(&mut self, reg: OverlayRegistration<S>) -> &mut Self {
        self.kernel.overlay_dispatcher.register_overlay(reg);
        self
    }

    /// Registers a dismissible page Escape layer targeting a declared [`EscapeBand`].
    pub fn register_escape_layer(&mut self, layer: EscapeLayer<S>) -> &mut Self {
        self.kernel.overlay_dispatcher.register_escape_layer(layer);
        self
    }

    /// Registers a dismiss callback invoked when overlay `id` is dismissed through the kernel.
    pub fn register_overlay_dismiss(&mut self, id: &'static str, on_dismiss: OverlayDismissFn<S>) -> &mut Self {
        self.kernel.register_overlay_dismiss(id, on_dismiss);
        self
    }

    /// Registers a tracked state slot seeded with `T::default()` before the window starts.
    pub fn register_slot<T: Default + 'static>(&mut self, slot: &'static Slot<T>) -> &mut Self {
        self.kernel.slots.insert(slot, T::default());
        self
    }

    /// Registers a tracked state slot seeded with `val` before the window starts.
    pub fn register_slot_with<T: 'static>(&mut self, slot: &'static Slot<T>, val: T) -> &mut Self {
        self.kernel.slots.insert(slot, val);
        self
    }

    /// Finalizes and validates the tick schedule and overlay dispatcher and returns the assembled [`Kernel`].
    pub fn build(mut self) -> Kernel<S> {
        self.kernel.scheduler.resolve();
        self.kernel.overlay_dispatcher.resolve();
        self.kernel
    }
}
