//! Feature registration contract and native surface provider registry.

use std::collections::HashMap;

use crate::KernelBuilder;

/// A self-contained feature module that registers its systems, commands, slots, and surfaces into
/// a [`KernelBuilder`] without editing central switchboards.
pub trait Feature<S> {
    /// Stable identifier for this feature (used for deduplication and diagnostics).
    fn id(&self) -> &'static str;

    /// Registers this feature's hooks, commands, initial slots, and surfaces.
    fn register(&self, builder: &mut KernelBuilder<S>);
}

/// Built-in and extensible surface categories hosted inside a Rune shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SurfaceKind {
    Terminal,
    Browser,
    Office,
    CodeView,
    Diff,
    Preview,
    Custom(&'static str),
}

impl SurfaceKind {
    /// Canonical string name for this surface kind.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::Browser => "browser",
            Self::Office => "office",
            Self::CodeView => "code-view",
            Self::Diff => "diff",
            Self::Preview => "preview",
            Self::Custom(name) => name,
        }
    }
}

/// Metadata describing a registered native surface provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurfaceDescriptor {
    pub id: &'static str,
    pub kind: SurfaceKind,
    pub title: &'static str,
    /// Optional lowercase file extensions (without leading dot) handled by this surface.
    pub extensions: &'static [&'static str],
    /// Whether a window may host multiple simultaneous instances of this surface.
    pub multi_instance: bool,
}

impl SurfaceDescriptor {
    /// Creates a descriptor with no extension bindings and single-instance default.
    pub const fn new(id: &'static str, kind: SurfaceKind, title: &'static str) -> Self {
        Self { id, kind, title, extensions: &[], multi_instance: false }
    }

    /// Declares file extensions handled by this surface provider.
    pub const fn with_extensions(mut self, extensions: &'static [&'static str]) -> Self {
        self.extensions = extensions;
        self
    }

    /// Declares whether multiple instances can exist simultaneously per window.
    pub const fn with_multi_instance(mut self, multi_instance: bool) -> Self {
        self.multi_instance = multi_instance;
        self
    }

    /// Returns `true` if this surface claims file extension `ext` (case-insensitive, optional dot).
    pub fn handles_extension(&self, ext: &str) -> bool {
        let needle = ext.strip_prefix('.').unwrap_or(ext);
        self.extensions.iter().any(|candidate| candidate.eq_ignore_ascii_case(needle))
    }
}

/// Registry of native surface providers (`terminal`, `browser`, `office`, `code-view`, ...).
#[derive(Clone, Debug, Default)]
pub struct SurfaceRegistry {
    entries: Vec<SurfaceDescriptor>,
    by_id: HashMap<&'static str, usize>,
}

impl SurfaceRegistry {
    /// Creates an empty surface registry.
    pub fn new() -> Self {
        Self { entries: Vec::new(), by_id: HashMap::new() }
    }

    /// Registers a surface descriptor, replacing any existing descriptor with the same `id`.
    pub fn register(&mut self, descriptor: SurfaceDescriptor) {
        if let Some(&idx) = self.by_id.get(descriptor.id) {
            self.entries[idx] = descriptor;
            return;
        }
        let idx = self.entries.len();
        self.by_id.insert(descriptor.id, idx);
        self.entries.push(descriptor);
    }

    /// Removes a surface descriptor by id.
    pub fn unregister(&mut self, id: &str) -> Option<SurfaceDescriptor> {
        let idx = self.by_id.remove(id)?;
        let removed = self.entries.remove(idx);
        self.by_id.clear();
        for (i, entry) in self.entries.iter().enumerate() {
            self.by_id.insert(entry.id, i);
        }
        Some(removed)
    }

    /// Looks up a surface descriptor by string id.
    pub fn get(&self, id: &str) -> Option<&SurfaceDescriptor> {
        let &idx = self.by_id.get(id)?;
        self.entries.get(idx)
    }

    /// Iterates all registered surfaces matching `kind`.
    pub fn by_kind(&self, kind: SurfaceKind) -> impl Iterator<Item = &SurfaceDescriptor> {
        self.entries.iter().filter(move |entry| entry.kind == kind)
    }

    /// Finds the first registered surface claiming file extension `ext`.
    pub fn for_extension(&self, ext: &str) -> Option<&SurfaceDescriptor> {
        self.entries.iter().find(|entry| entry.handles_extension(ext))
    }

    /// Iterates all registered surface descriptors in registration order.
    pub fn iter(&self) -> std::slice::Iter<'_, SurfaceDescriptor> {
        self.entries.iter()
    }

    /// Number of registered surfaces.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` when no surfaces are registered.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
