//! Per-window tracked state slots so features store their own state without adding fields to the
//! host application struct.
//!
//! Every slot wraps its value in [`rune::Tracked`], so reading a slot inside a `rune::region` or
//! `rune::tracked_reads` subscribes to that slot's cell and mutating it invalidates only regions
//! that read it.
//!
//! Slots are addressed by static [`Slot<T>`] handles resolved once per process into a dense `Vec`
//! index: zero `TypeId` hashing on access, safe across multiple windows, and usable from plain
//! `fn(&mut App) -> &mut T` lenses.

use std::any::{Any, type_name};
use std::fmt;
use std::marker::PhantomData;
use std::ops::{Index, IndexMut};
use std::sync::atomic::{AtomicUsize, Ordering};

use rune::Tracked;

const UNASSIGNED: usize = usize::MAX;
static NEXT_SLOT: AtomicUsize = AtomicUsize::new(0);

/// Typed static handle identifying a per-window [`rune::Tracked`] slot in [`StateSlots`].
///
/// Declare one `static` per slot in the feature's adapter (or feature crate):
///
/// ```ignore
/// static MY_STATE: Slot<MyState> = Slot::new();
/// ```
///
/// Seed it once in `Feature::register` via `builder.register_slot(&MY_STATE)` (or
/// `builder.register_slot_with(&MY_STATE, ...)`). Its dense index is resolved once per process
/// and cached in an atomic `usize`, so reads and writes index directly into a `Vec` with zero
/// hashing and work inside plain `fn(&mut App) -> &mut T` lenses.
pub struct Slot<T: 'static> {
    index: AtomicUsize,
    // `fn(T) -> T` is invariant in `T` and unconditionally `Send + Sync`, even when `T` is not
    // `Send` or `Sync` (for example UI states holding `Rc` or `RefCell`).
    _marker: PhantomData<fn(T) -> T>,
}

impl<T: 'static> Slot<T> {
    /// Creates a new slot handle. Store in a `static` item so `&'static Slot<T>` can be passed to
    /// [`StateSlots`] and [`crate::KernelBuilder`].
    pub const fn new() -> Self {
        Self { index: AtomicUsize::new(UNASSIGNED), _marker: PhantomData }
    }

    /// Returns this handle's dense slot index, assigning one once on first use.
    #[inline]
    pub fn index(&self) -> usize {
        let idx = self.index.load(Ordering::Relaxed);
        if idx != UNASSIGNED {
            return idx;
        }
        self.assign()
    }

    #[cold]
    fn assign(&self) -> usize {
        let candidate = NEXT_SLOT.fetch_add(1, Ordering::Relaxed);
        match self.index.compare_exchange(
            UNASSIGNED,
            candidate,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => candidate,
            Err(existing) => existing,
        }
    }
}

impl<T: 'static> Default for Slot<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: 'static> fmt::Debug for Slot<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let idx = self.index.load(Ordering::Relaxed);
        f.debug_struct("Slot")
            .field("type", &type_name::<T>())
            .field("index", &(idx != UNASSIGNED).then_some(idx))
            .finish()
    }
}

#[cold]
#[inline(never)]
fn missing_slot<T>(idx: usize) -> ! {
    panic!(
        "StateSlots: slot `{}` (index {idx}) is not registered; register it in Feature::register via builder.register_slot(&SLOT)",
        type_name::<T>()
    )
}

#[inline]
fn downcast_tracked<T: 'static>(entry: &dyn Any) -> &Tracked<T> {
    match entry.downcast_ref::<Tracked<T>>() {
        Some(cell) => cell,
        None => unreachable!("Slot<{}> index always maps to Tracked<T>", type_name::<T>()),
    }
}

#[inline]
fn downcast_tracked_mut<T: 'static>(entry: &mut dyn Any) -> &mut Tracked<T> {
    match entry.downcast_mut::<Tracked<T>>() {
        Some(cell) => cell,
        None => unreachable!("Slot<{}> index always maps to Tracked<T>", type_name::<T>()),
    }
}

/// Per-window heterogeneous store of [`rune::Tracked`] feature states indexed by static
/// [`Slot<T>`] handles.
///
/// Every stored value lives inside a `rune::Tracked<T>` cell so `rune::region` and
/// `rune::tracked_reads` subscribe on read and invalidate on write identically to a `Tracked<T>`
/// field on the host application struct.
pub struct StateSlots {
    entries: Vec<Option<Box<dyn Any>>>,
    count: usize,
    /// Tracks absent <-> present slot transitions so even a `try_get` on an unregistered lazy slot
    /// inside a `rune::region` invalidates once that slot is inserted.
    presence: Tracked<()>,
}

impl Default for StateSlots {
    fn default() -> Self {
        Self::new()
    }
}

impl StateSlots {
    /// Creates an empty slot table.
    pub fn new() -> Self {
        Self { entries: Vec::new(), count: 0, presence: Tracked::new(()) }
    }

    /// Inserts or replaces the tracked value in `slot`.
    ///
    /// If `slot` is already populated, writes through its existing [`Tracked`] cell in place
    /// (preserving its cell id and bumping its version). If vacant, allocates a new [`Tracked`]
    /// cell and notifies presence subscribers.
    pub fn insert<T: 'static>(&mut self, slot: &'static Slot<T>, value: T) {
        let idx = slot.index();
        if idx >= self.entries.len() {
            self.entries.resize_with(idx + 1, || None);
        }
        if let Some(existing) = self.entries[idx].as_deref_mut() {
            **downcast_tracked_mut::<T>(existing) = value;
        } else {
            self.entries[idx] = Some(Box::new(Tracked::new(value)));
            self.count += 1;
            Tracked::touch(&self.presence);
        }
    }

    /// Returns a shared reference to the value in `slot`, recording a [`Tracked`] read in any
    /// active `rune::region` or `rune::tracked_reads`.
    ///
    /// Panics if `slot` has not been registered in this `StateSlots`.
    #[inline]
    pub fn get<T: 'static>(&self, slot: &'static Slot<T>) -> &T {
        self.tracked(slot)
    }

    /// Returns a mutable reference to the value in `slot`, bumping its [`Tracked`] cell version.
    ///
    /// Panics if `slot` has not been registered in this `StateSlots`.
    #[inline]
    pub fn get_mut<T: 'static>(&mut self, slot: &'static Slot<T>) -> &mut T {
        let idx = slot.index();
        match self.entries.get_mut(idx).and_then(Option::as_deref_mut) {
            Some(entry) => downcast_tracked_mut::<T>(entry),
            None => missing_slot::<T>(idx),
        }
    }

    /// Returns a shared reference to `slot` if present (recording a [`Tracked`] read), or `None`
    /// if absent (subscribing to slot presence so a later insert still invalidates the region).
    #[inline]
    pub fn try_get<T: 'static>(&self, slot: &'static Slot<T>) -> Option<&T> {
        match self.try_tracked(slot) {
            Some(cell) => Some(&**cell),
            None => {
                let _ = &*self.presence;
                None
            }
        }
    }

    /// Returns a mutable reference to `slot` if present, bumping its [`Tracked`] cell version.
    #[inline]
    pub fn try_get_mut<T: 'static>(&mut self, slot: &'static Slot<T>) -> Option<&mut T> {
        let idx = slot.index();
        self.entries
            .get_mut(idx)
            .and_then(Option::as_deref_mut)
            .map(|entry| &mut **downcast_tracked_mut::<T>(entry))
    }

    /// Returns a mutable reference to `slot`, inserting `T::default()` if absent, and bumps its
    /// [`Tracked`] cell version.
    #[inline]
    pub fn get_or_default<T: Default + 'static>(&mut self, slot: &'static Slot<T>) -> &mut T {
        let idx = slot.index();
        if idx >= self.entries.len() {
            self.entries.resize_with(idx + 1, || None);
        }
        if self.entries[idx].is_none() {
            self.entries[idx] = Some(Box::new(Tracked::new(T::default())));
            self.count += 1;
            Tracked::touch(&self.presence);
        }
        match self.entries[idx].as_deref_mut() {
            Some(entry) => downcast_tracked_mut::<T>(entry),
            None => unreachable!("slot populated immediately above"),
        }
    }

    /// Returns a shared reference to `slot` without recording a [`Tracked`] read (matches
    /// [`Tracked::untracked`]).
    ///
    /// Panics if `slot` has not been registered.
    #[inline]
    pub fn untracked<T: 'static>(&self, slot: &'static Slot<T>) -> &T {
        Tracked::untracked(self.tracked(slot))
    }

    /// Non-panicking [`Self::untracked`]: returns `None` without recording any read when absent.
    #[inline]
    pub fn try_untracked<T: 'static>(&self, slot: &'static Slot<T>) -> Option<&T> {
        self.try_tracked(slot).map(Tracked::untracked)
    }

    /// Marks `slot` changed without borrowing it mutably (for interior mutability; matches
    /// [`Tracked::touch`]).
    ///
    /// Panics if `slot` has not been registered.
    #[inline]
    pub fn touch<T: 'static>(&self, slot: &'static Slot<T>) {
        Tracked::touch(self.tracked(slot));
    }

    /// Returns the write count of `slot`'s [`Tracked`] cell (matches [`Tracked::version`]).
    ///
    /// Panics if `slot` has not been registered.
    #[inline]
    pub fn version<T: 'static>(&self, slot: &'static Slot<T>) -> u64 {
        Tracked::version(self.tracked(slot))
    }

    /// Returns the cell id of `slot`'s [`Tracked`] cell (matches [`Tracked::id`]).
    ///
    /// Panics if `slot` has not been registered.
    #[inline]
    pub fn cell_id<T: 'static>(&self, slot: &'static Slot<T>) -> u32 {
        Tracked::id(self.tracked(slot))
    }

    /// Returns the underlying [`Tracked`] cell for `slot`.
    ///
    /// Panics if `slot` has not been registered.
    #[inline]
    pub fn tracked<T: 'static>(&self, slot: &'static Slot<T>) -> &Tracked<T> {
        let idx = slot.index();
        match self.entries.get(idx).and_then(Option::as_deref) {
            Some(entry) => downcast_tracked::<T>(entry),
            None => missing_slot::<T>(idx),
        }
    }

    /// Returns the underlying [`Tracked`] cell for `slot`, if present.
    #[inline]
    pub fn try_tracked<T: 'static>(&self, slot: &'static Slot<T>) -> Option<&Tracked<T>> {
        let idx = slot.index();
        self.entries.get(idx).and_then(Option::as_deref).map(downcast_tracked::<T>)
    }

    /// Borrows two distinct registered slots mutably at the same time, bumping both cells'
    /// [`Tracked`] versions.
    ///
    /// Panics if `a` and `b` refer to the same slot index or either slot is not registered.
    pub fn get_two_mut<A: 'static, B: 'static>(
        &mut self,
        a: &'static Slot<A>,
        b: &'static Slot<B>,
    ) -> (&mut A, &mut B) {
        let ia = a.index();
        let ib = b.index();
        assert!(ia != ib, "StateSlots::get_two_mut called with identical slot index {ia}");
        if ia < ib {
            if ib >= self.entries.len() {
                missing_slot::<B>(ib);
            }
            let (left, right) = self.entries.split_at_mut(ib);
            let ra = match left.get_mut(ia).and_then(Option::as_deref_mut) {
                Some(entry) => &mut **downcast_tracked_mut::<A>(entry),
                None => missing_slot::<A>(ia),
            };
            let rb = match right[0].as_deref_mut() {
                Some(entry) => &mut **downcast_tracked_mut::<B>(entry),
                None => missing_slot::<B>(ib),
            };
            (ra, rb)
        } else {
            if ia >= self.entries.len() {
                missing_slot::<A>(ia);
            }
            let (left, right) = self.entries.split_at_mut(ia);
            let rb = match left.get_mut(ib).and_then(Option::as_deref_mut) {
                Some(entry) => &mut **downcast_tracked_mut::<B>(entry),
                None => missing_slot::<B>(ib),
            };
            let ra = match right[0].as_deref_mut() {
                Some(entry) => &mut **downcast_tracked_mut::<A>(entry),
                None => missing_slot::<A>(ia),
            };
            (ra, rb)
        }
    }

    /// Removes and drops the value in `slot` (bumping its [`Tracked`] cell on drop and notifying
    /// presence subscribers). Returns `true` if a value was present.
    pub fn remove<T: 'static>(&mut self, slot: &'static Slot<T>) -> bool {
        let idx = slot.index();
        if let Some(entry) = self.entries.get_mut(idx)
            && entry.take().is_some()
        {
            self.count -= 1;
            Tracked::touch(&self.presence);
            return true;
        }
        false
    }

    /// Returns `true` if `slot` is currently populated, subscribing to slot presence.
    #[inline]
    pub fn contains<T: 'static>(&self, slot: &'static Slot<T>) -> bool {
        let _ = &*self.presence;
        self.entries.get(slot.index()).is_some_and(Option::is_some)
    }

    /// Number of occupied slots.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Returns `true` when no slots are stored.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Drops all stored slots.
    pub fn clear(&mut self) {
        if self.count > 0 {
            self.entries.clear();
            self.count = 0;
            Tracked::touch(&self.presence);
        }
    }
}

impl<T: 'static> Index<&'static Slot<T>> for StateSlots {
    type Output = T;

    #[inline]
    fn index(&self, slot: &'static Slot<T>) -> &T {
        self.get(slot)
    }
}

impl<T: 'static> IndexMut<&'static Slot<T>> for StateSlots {
    #[inline]
    fn index_mut(&mut self, slot: &'static Slot<T>) -> &mut T {
        self.get_mut(slot)
    }
}

impl fmt::Debug for StateSlots {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StateSlots").field("len", &self.count).finish()
    }
}
