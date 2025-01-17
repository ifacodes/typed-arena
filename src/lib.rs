//! A contiguous growable container which assigns and returns IDs to values when
//! they are added to it.
//!
//! These IDs can then be used to access their corresponding values at any time,
//! like an index, except that they remain valid even if other items in the arena
//! are removed or if the arena is sorted.
//!
//! A big advantage of this collection over something like a [`HashMap`](std::collections::HashMap)
//! is that, since the values are stored in contiguous memory, you can access this
//! slice [directly](Arena::as_slice) and get all the benefits that you would from
//! having an array or a [`Vec`], such as parallel iterators with [`rayon`](https://crates.io/crates/rayon).
//!
//! # Examples
//!
//! ```
//! use arena::Arena;
//!
//! // create an arena and add 3 values to it
//! let mut arena = Arena::new();
//! let a = arena.insert('A');
//! let b = arena.insert('B');
//! let c = arena.insert('C');
//!
//! // we can access the slice of values directly
//! assert_eq!(arena.as_slice(), &['A', 'B', 'C']);
//!
//! // or we can use the returned IDs to access them
//! assert_eq!(arena.get(a), Some(&'A'));
//! assert_eq!(arena.get(b), Some(&'B'));
//! assert_eq!(arena.get(c), Some(&'C'));
//!
//! // remove a value from the middle
//! arena.remove(b);
//!
//! // the slice now only has the remaining values
//! assert_eq!(arena.as_slice(), &['A', 'C']);
//!
//! // even though `C` changed position, its ID is still valid
//! assert_eq!(arena.get(a), Some(&'A'));
//! assert_eq!(arena.get(b), None);
//! assert_eq!(arena.get(c), Some(&'C'));
//!
//! // IDs are copyable so they can be passed around easily
//! let some_id = c;
//! assert_eq!(arena.get(some_id), Some(&'C'));
//! ```
//!
//! # Iteration
//!
//! Because arena implements [`Deref<Target = [T]>`](Arena::deref), you can iterate over
//! the values in the contiguous slice directly:
//!
//! ```
//! # use arena::Arena;
//! let mut arena = Arena::from(['A', 'B', 'C']);
//!
//! let mut iter = arena.iter();
//! assert_eq!(iter.next(), Some(&'A'));
//! assert_eq!(iter.next(), Some(&'B'));
//! assert_eq!(iter.next(), Some(&'C'));
//! assert_eq!(iter.next(), None);
//! ```
//!
//! Alternatively, you can iterate over ID/value pairs:
//!
//! ```
//! # use arena::Arena;
//! let mut arena = Arena::new();
//! let a = arena.insert('A');
//! let b = arena.insert('B');
//! let c = arena.insert('C');
//!
//! let mut pairs = arena.pairs();
//! assert_eq!(pairs.next(), Some((a, &'A')));
//! assert_eq!(pairs.next(), Some((b, &'B')));
//! assert_eq!(pairs.next(), Some((c, &'C')));
//! assert_eq!(pairs.next(), None);
//! ```
//!
//! Or iterate over just the IDs:
//!
//! ```
//! # use arena::Arena;
//! # let mut arena = Arena::new();
//! # let a = arena.insert('A');
//! # let b = arena.insert('B');
//! # let c = arena.insert('C');
//! let mut ids = arena.ids();
//! assert_eq!(ids.next(), Some(a));
//! assert_eq!(ids.next(), Some(b));
//! assert_eq!(ids.next(), Some(c));
//! assert_eq!(ids.next(), None);
//! ```
//!
//! # Performance
//!
//! Lookups by ID do a few checks, so they are slower than `Vec<T>` indexing, but like
//! a vector they do not take longer even when the collection grows. To provide this
//! ability, though, adding and removing from the arena has more overhead as well.
//!
//! To keep removal fast, the arena uses a "pop & swap" method to remove values, meaning
//! the last value will get moved into the removed value's position. The ID of that value
//! will then get remapped to prevent it from being invalidated. Because of this, you
//! should never assume the values or IDs in an arena remain in the order you added them.

use std::cmp::Ordering;
use std::marker::PhantomData;
use std::ops::{Deref, Index, IndexMut};
#[cfg(feature = "uuid")]
use uuid::Uuid;

/// A contiguous growable container which assigns and returns IDs to values when they are
/// added to it.
#[derive(Debug, Clone)]
pub struct Arena<T> {
    values: Vec<T>,
    slots: Vec<Slot>,
    next_uid: u64,
    first_free: Option<usize>,
    #[cfg(feature = "uuid")]
    uuid: Uuid,
}

impl<T> Arena<T> {
    /// Constructs a new, empty `Arena<T>`.
    ///
    /// # Examples
    ///
    /// ```
    /// # #![allow(unused_mut)]
    /// # use arena::Arena;
    /// let mut arena: Arena<String> = Arena::new();
    /// ```
    #[cfg(not(feature = "uuid"))]
    pub const fn new() -> Self {
        Self {
            values: Vec::new(),
            slots: Vec::new(),
            next_uid: 1,
            first_free: None,
        }
    }

    #[cfg(feature = "uuid")]
    pub fn new() -> Self {
        Self {
            values: Vec::new(),
            slots: Vec::new(),
            next_uid: 1,
            first_free: None,
            uuid: Uuid::new_v4(),
        }
    }

    /// Constructs a new, empty `Arena<T>` with at least the specified capacity.
    ///
    /// # Examples
    ///
    /// ```
    /// # #![allow(unused_mut)]
    /// # use arena::Arena;
    /// let mut arena: Arena<String> = Arena::with_capacity(1000);
    /// ```
    #[inline]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            values: Vec::with_capacity(capacity),
            slots: Vec::with_capacity(capacity),
            next_uid: 1,
            first_free: None,
            #[cfg(feature = "uuid")]
            uuid: Uuid::new_v4(),
        }
    }

    /// Returns `true` if the arena contains no elements.
    ///
    /// # Examples
    ///
    /// ```
    /// # use arena::Arena;
    /// let mut arena = Arena::new();
    /// assert!(arena.is_empty());
    ///
    /// arena.insert('A');
    /// assert!(!arena.is_empty());
    /// ```
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Returns the amount of slots the arena is using to map IDs.
    ///
    /// # Examples
    ///
    /// ```
    /// # use arena::Arena;
    /// let mut arena = Arena::from(['A', 'B', 'C']);
    ///
    /// assert_eq!(arena.len(), 3);
    /// assert_eq!(arena.slot_count(), 3);
    ///
    /// arena.clear();
    ///
    /// assert_eq!(arena.len(), 0);
    /// assert_eq!(arena.slot_count(), 3);
    /// ```
    #[inline]
    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// Returns the amount of empty slots the arena has. New values added to
    /// the arena will make use of these slots instead of creating new ones.
    ///
    /// # Examples
    ///
    /// ```
    /// # use arena::Arena;
    /// let mut arena = Arena::from(['A', 'B', 'C']);
    ///
    /// assert_eq!(arena.slot_count(), 3);
    /// assert_eq!(arena.free_slot_count(), 0);
    ///
    /// let _ = arena.pop();
    ///
    /// assert_eq!(arena.slot_count(), 3);
    /// assert_eq!(arena.free_slot_count(), 1);
    /// ```
    #[inline]
    pub fn free_slot_count(&self) -> usize {
        self.slot_count() - self.len()
    }

    /// Extracts a slice containing all the arena's values.
    ///
    /// # Examples
    ///
    /// ```
    /// # use arena::Arena;
    /// let mut arena = Arena::from(['A', 'B', 'C']);
    ///
    /// assert_eq!(arena.as_slice(), &['A', 'B', 'C']);
    ///
    /// let _ = arena.pop();
    ///
    /// assert_eq!(arena.as_slice(), &['A', 'B']);
    /// ```
    #[inline]
    pub fn as_slice(&self) -> &[T] {
        self.values.as_slice()
    }

    /// Extracts a mutable slice containing all the arena's values.
    ///
    /// # Examples
    ///
    /// ```
    /// # use arena::Arena;
    /// let mut arena: Arena<i32> = Arena::from([1, 2, 3, 4, 5]);
    ///
    /// assert_eq!(arena.as_mut_slice(), &[1, 2, 3, 4, 5]);
    ///
    /// for num in arena.as_mut_slice() {
    ///     *num += 1;
    /// }
    ///
    /// assert_eq!(arena.as_mut_slice(), &[2, 3, 4, 5, 6]);
    ///
    /// ```
    ///
    /// # Warning
    ///
    /// Re-arranging the values in this mutable slice will invalidate the IDs given
    /// when they were added to the arena. It is recommended only to use this slice
    /// for modifying the values in place, either in sequence or in parallel (for
    /// example, with the `rayon` library).
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        self.values.as_mut_slice()
    }

    /// Returns an unsafe mutable pointer to the arena's value buffer, or a dangling
    /// raw pointer valid for zero sized reads if the arena didn't allocate.
    #[inline]
    pub fn as_mut_ptr(&mut self) -> *mut T {
        self.values.as_mut_ptr()
    }

    #[cfg(feature = "uuid")]
    pub fn match_id(&self, id: &ArenaId<T>) -> bool {
        id.uuid == self.uuid
    }

    /// Returns a reference to the value assigned with the ID, or `None` if the
    /// value is not in the arena.
    ///
    /// # Examples
    ///
    /// ```
    /// # use arena::Arena;
    /// let mut arena = Arena::new();
    /// let a = arena.insert('A');
    /// let b = arena.insert('B');
    /// let c = arena.insert('C');
    ///
    /// assert_eq!(arena.get(a), Some(&'A'));
    /// assert_eq!(arena.get(b), Some(&'B'));
    /// assert_eq!(arena.get(c), Some(&'C'));
    ///
    /// arena.remove(b);
    ///
    /// assert_eq!(arena.get(a), Some(&'A'));
    /// assert_eq!(arena.get(b), None);
    /// assert_eq!(arena.get(c), Some(&'C'));
    /// ```
    #[inline]
    pub fn get(&self, id: ArenaId<T>) -> Option<&T> {
        #[cfg(feature = "uuid")]
        if !self.match_id(&id) {
            return None;
        }
        match &self.slots.get(id.idx)?.state {
            State::Used { uid, value } if *uid == id.uid => Some(&self.values[*value]),
            _ => None,
        }
    }

    /// Returns a mutable reference to the value assigned with the ID, or `None`
    /// if the value is not in the arena.
    ///
    /// # Examples
    ///
    /// ```
    /// # use arena::Arena;
    /// let mut arena = Arena::new();
    /// let a = arena.insert('A');
    /// let b = arena.insert('B');
    ///
    /// assert_eq!(arena.as_slice(), &['A', 'B']);
    ///
    /// if let Some(a_val) = arena.get_mut(a) {
    ///     *a_val = 'B';
    /// }
    ///
    /// if let Some(b_val) = arena.get_mut(b) {
    ///     *b_val = 'A';
    /// }
    ///
    /// assert_eq!(arena.as_slice(), &['B', 'A']);
    /// ```
    #[inline]
    pub fn get_mut(&mut self, id: ArenaId<T>) -> Option<&mut T> {
        #[cfg(feature = "uuid")]
        if !self.match_id(&id) {
            return None;
        }
        match &self.slots.get(id.idx)?.state {
            State::Used { uid, value } if *uid == id.uid => Some(&mut self.values[*value]),
            _ => None,
        }
    }

    /// Returns a pair of mutable references correspding to the pair of
    /// supplied IDs.
    ///
    /// # Examples
    ///
    /// ```
    /// # use arena::Arena;
    /// let mut arena = Arena::new();
    /// let a = arena.insert('A');
    /// let b = arena.insert('B');
    ///
    /// assert_eq!(arena.as_slice(), &['A', 'B']);
    ///
    /// match arena.get2_mut(a, b) {
    ///     (Some(val_a), Some(val_b)) => {
    ///         *val_a = 'X';
    ///         *val_b = 'Y';
    ///     }
    ///     _ => panic!()
    /// }
    ///
    /// assert_eq!(arena.as_slice(), &['X', 'Y']);
    ///
    /// ```
    pub fn get2_mut(&mut self, a: ArenaId<T>, b: ArenaId<T>) -> (Option<&mut T>, Option<&mut T>) {
        #[cfg(feature = "uuid")]
        if !self.match_id(&a) || !self.match_id(&b) {
            return (None, None);
        }
        match (self.index_of(a), self.index_of(b)) {
            (Some(a), Some(b)) => {
                assert_ne!(a, b);
                let (lower, upper) = self.values.split_at_mut(a.max(b));
                let (a, b) = if a < b {
                    (&mut lower[a], &mut upper[0])
                } else {
                    (&mut lower[0], &mut upper[b])
                };
                (Some(a), Some(b))
            }
            (Some(a), None) => (Some(&mut self.values[a]), None),
            (None, Some(b)) => (None, Some(&mut self.values[b])),
            (None, None) => (None, None),
        }
    }

    /// Returns true if the arena contains a value assigned with the ID.
    ///
    /// # Examples
    ///
    /// ```
    /// # use arena::Arena;
    /// let mut arena = Arena::new();
    /// let a = arena.insert('A');
    /// let b = arena.insert('B');
    /// let c = arena.insert('C');
    ///
    /// assert!(arena.contains(a));
    /// assert!(arena.contains(b));
    /// assert!(arena.contains(c));
    ///
    /// arena.remove(a);
    ///
    /// assert!(!arena.contains(a));
    /// assert!(arena.contains(b));
    /// assert!(arena.contains(c));
    /// ```
    #[inline]
    pub fn contains(&self, id: ArenaId<T>) -> bool {
        #[cfg(feature = "uuid")]
        if !self.match_id(&id) {
            return false;
        }
        self.get(id).is_some()
    }

    /// Returns the ID assigned to the value at the corresponding index, or
    /// `None` if the index is out of bounds.
    ///
    /// # Examples
    ///
    /// ```
    /// # use arena::Arena;
    /// let mut arena = Arena::new();
    /// let a = arena.insert('A');
    /// let b = arena.insert('B');
    /// let c = arena.insert('C');
    ///
    /// assert_eq!(arena.id_at(0), Some(a));
    /// assert_eq!(arena.id_at(1), Some(b));
    /// assert_eq!(arena.id_at(2), Some(c));
    ///
    /// println!("{:#?}", arena);
    ///
    /// arena.remove(b);
    ///
    /// println!("{:#?}", arena);
    ///
    /// assert_eq!(arena.id_at(0), Some(a));
    /// assert_eq!(arena.id_at(1), Some(c));
    /// assert_eq!(arena.id_at(2), None);
    ///
    /// ```
    #[inline]
    pub fn id_at(&self, index: usize) -> Option<ArenaId<T>> {
        if index >= self.len() {
            return None;
        }
        let idx = self.slots.get(index)?.value_slot;
        match &self.slots[idx].state {
            State::Used { uid, value } if *value == index => Some(ArenaId::<T> {
                #[cfg(feature = "uuid")]
                uuid: self.uuid,
                uid: *uid,
                idx,
                _ty: PhantomData,
            }),
            _ => None,
        }
    }

    /// Returns the index of the value corresponding to the ID if it is in the arena.
    ///
    /// # Examples
    ///
    /// ```
    /// # use arena::Arena;
    /// let mut arena = Arena::new();
    /// let a = arena.insert('A');
    /// let b = arena.insert('B');
    /// let c = arena.insert('C');
    /// let d = arena.insert('D');
    ///
    /// assert_eq!(arena.as_slice(), &['A', 'B', 'C', 'D']);
    /// assert_eq!(arena.index_of(a), Some(0));
    /// assert_eq!(arena.index_of(b), Some(1));
    /// assert_eq!(arena.index_of(c), Some(2));
    /// assert_eq!(arena.index_of(d), Some(3));
    ///
    /// // remove `B` from the arena
    /// arena.remove_at(1);
    ///
    /// // now `D` has moved into the hole created by `B`
    /// assert_eq!(arena.as_slice(), &['A', 'D', 'C']);
    /// assert_eq!(arena.index_of(d), Some(1));
    ///
    /// ```
    #[inline]
    pub fn index_of(&self, id: ArenaId<T>) -> Option<usize> {
        #[cfg(feature = "uuid")]
        if !self.match_id(&id) {
            return None;
        }
        match &self.slots.get(id.idx)?.state {
            State::Used { uid, value } if *uid == id.uid => Some(*value),
            _ => None,
        }
    }

    /// Inserts a value in the arena, returning an ID that can be used to
    /// access the value at a later time, even if the values were re-arranged.
    ///
    /// # Examples
    ///
    /// ```
    /// # use arena::Arena;
    /// let mut arena = Arena::new();
    /// let a = arena.insert('A');
    /// let b = arena.insert('B');
    ///
    /// assert_ne!(a, b);
    /// assert_eq!(arena.get(a), Some(&'A'));
    /// assert_eq!(arena.get(b), Some(&'B'));
    /// ```
    #[inline]
    pub fn insert(&mut self, value: T) -> ArenaId<T> {
        self.insert_with(|_| value)
    }

    /// Inserts a value, created by the provided function, to the arena. The
    /// function is passed the ID assigned to the value, which is useful if
    /// the values themselves want to store the IDs on construction.
    ///
    /// # Examples
    ///
    /// ```
    /// # use arena::{Arena, ArenaId};
    /// #[derive(Debug)]
    /// struct Person {
    ///     id: ArenaId<Person>,
    ///     name: &'static str,
    /// }
    ///
    /// let mut arena = Arena::new();
    ///
    /// let foo = arena.insert_with(|id| Person {
    ///     id,
    ///     name: "Foo",
    /// });
    ///
    /// let bar = arena.insert_with(|id| Person {
    ///     id,
    ///     name: "Bar",
    /// });
    ///
    /// assert_eq!(arena[foo].id, foo);
    /// assert_eq!(arena[foo].name, "Foo");
    ///
    /// assert_eq!(arena[bar].id, bar);
    /// assert_eq!(arena[bar].name, "Bar");
    /// ```
    pub fn insert_with<F>(&mut self, create: F) -> ArenaId<T>
    where
        F: FnOnce(ArenaId<T>) -> T,
    {
        let value = self.values.len();
        let idx = match self.first_free.take() {
            Some(idx) => {
                match &self.slots[idx].state {
                    State::Free { next_free } => {
                        self.first_free = *next_free;
                    }
                    _ => unreachable!(),
                }
                self.slots[idx].state = State::Used {
                    uid: self.next_uid,
                    value,
                };
                idx
            }
            None => {
                let idx = self.slots.len();
                self.slots.push(Slot {
                    value_slot: 0,
                    state: State::Used {
                        uid: self.next_uid,
                        value,
                    },
                });
                idx
            }
        };
        self.slots[value].value_slot = idx;
        let id = ArenaId::<T> {
            #[cfg(feature = "uuid")]
            uuid: self.uuid,
            uid: self.next_uid,
            idx,
            _ty: PhantomData,
        };
        self.next_uid += 1;
        self.values.push(create(id));
        id
    }

    /// Removes the value from the arena assigned to the ID. If the value existed
    /// in the arena, it will be returned.
    ///
    /// # Examples
    ///
    /// ```
    /// # use arena::Arena;
    /// let mut arena = Arena::new();
    /// let foo = arena.insert("foo");
    ///
    /// assert_eq!(arena.remove(foo), Some("foo"));
    /// assert_eq!(arena.remove(foo), None);
    ///
    /// ```
    pub fn remove(&mut self, id: ArenaId<T>) -> Option<T> {
        #[cfg(feature = "uuid")]
        if !self.match_id(&id) {
            return None;
        }
        // get the position of the removed value
        let removed_val = match &self.slots[id.idx].state {
            State::Used { uid, value } if *uid == id.uid => *value,
            _ => return None,
        };

        // free up the slot of the removed value
        self.slots[id.idx].state = State::Free {
            next_free: self.first_free.replace(id.idx),
        };

        // check if the removed value is the last in the list
        let last_val = self.values.len() - 1;
        if removed_val < last_val {
            // if not, move the last value into the removed value's slot
            let last_slot = self.slots[last_val].value_slot;
            self.slots[removed_val].value_slot = last_slot;
            match &mut self.slots[last_slot].state {
                State::Used { uid, value } => *value = removed_val,
                _ => unreachable!(),
            }

            // then also move the value into the removed value's position
            Some(self.values.swap_remove(removed_val))
        } else {
            self.pop()
        }
    }

    /// Removes the value at the specified index and returns it.
    ///
    /// # Examples
    ///
    /// ```
    /// # use arena::Arena;
    /// let mut arena = Arena::from(['A', 'B', 'C']);
    ///
    /// assert_eq!(arena.remove_at(5), None);
    /// assert_eq!(arena.remove_at(1), Some('B'));
    /// assert_eq!(arena.remove_at(1), Some('C'));
    /// assert_eq!(arena.remove_at(1), None);
    /// assert_eq!(arena.remove_at(0), Some('A'));
    /// assert_eq!(arena.remove_at(0), None);
    /// ```
    pub fn remove_at(&mut self, index: usize) -> Option<T> {
        self.remove(self.id_at(index)?)
    }

    /// Pops a value off the end of the arena and returns it.
    ///
    /// # Examples
    ///
    /// ```
    /// # use arena::Arena;
    /// let mut arena = Arena::from(['A', 'B', 'C']);
    ///
    /// assert_eq!(arena.pop(), Some('C'));
    /// assert_eq!(arena.pop(), Some('B'));
    /// assert_eq!(arena.pop(), Some('A'));
    /// assert_eq!(arena.pop(), None);
    /// ```
    #[inline]
    pub fn pop(&mut self) -> Option<T> {
        let value = self.values.pop()?;
        let slot = self.slots[self.values.len()].value_slot;
        self.slots[slot].state = State::Free {
            next_free: self.first_free.replace(slot),
        };
        Some(value)
    }

    fn clear_opt(&mut self, clear_slots: bool) {
        if clear_slots {
            self.slots.clear();
            self.first_free = None;
        } else {
            for i in 0..self.values.len() {
                let slot = self.slots[i].value_slot;
                self.slots[slot].state = State::Free {
                    next_free: self.first_free.replace(slot),
                };
            }
        }

        self.values.clear();
    }

    /// Clears all values from the arena. This will free up all the slots,
    /// which will be reused for any values added after this call.
    ///
    /// # Examples
    ///
    /// ```
    /// # use arena::Arena;
    /// let mut arena = Arena::from(['A', 'B', 'C']);
    ///
    /// assert_eq!(arena.len(), 3);
    /// assert_eq!(arena.slot_count(), 3);
    ///
    /// arena.clear();
    ///
    /// assert_eq!(arena.len(), 0);
    /// assert_eq!(arena.slot_count(), 3);
    /// ```
    pub fn clear(&mut self) {
        self.clear_opt(false);
    }

    /// Clears all values and slots from the arena.
    ///
    /// # Examples
    ///
    /// ```
    /// # use arena::Arena;
    /// let mut arena = Arena::from(['A', 'B', 'C']);
    ///
    /// assert_eq!(arena.len(), 3);
    /// assert_eq!(arena.slot_count(), 3);
    ///
    /// arena.clear_all();
    ///
    /// assert_eq!(arena.len(), 0);
    /// assert_eq!(arena.slot_count(), 0);
    /// ```
    pub fn clear_all(&mut self) {
        self.clear_opt(true);
    }

    /// Swaps the position of the two values corresponding to the provided IDs without
    /// invalidating them.
    ///
    /// # Examples
    ///
    /// ```
    /// # use arena::Arena;
    /// let mut arena = Arena::new();
    /// let a = arena.insert('A');
    /// let b = arena.insert('B');
    ///
    /// assert_eq!(arena.as_slice(), &['A', 'B']);
    /// assert_eq!(arena[a], 'A');
    /// assert_eq!(arena[b], 'B');
    ///
    /// arena.swap_positions(a, b);
    ///
    /// assert_eq!(arena.as_slice(), &['B', 'A']);
    /// assert_eq!(arena[a], 'A');
    /// assert_eq!(arena[b], 'B');
    /// ```
    #[inline]
    pub fn swap_positions(&mut self, i: ArenaId<T>, j: ArenaId<T>) -> bool {
        #[cfg(feature = "uuid")]
        if !self.match_id(&i) || !self.match_id(&j) {
            return false;
        }
        if let Some(i) = self.index_of(i) {
            if let Some(j) = self.index_of(j) {
                self.swap(i, j);
                return true;
            }
        }
        false
    }

    /// Swaps values from the two positions in the arena without invalidating their IDS.
    ///
    /// # Examples
    ///
    /// ```
    /// # use arena::Arena;
    /// let mut arena = Arena::new();
    /// let a = arena.insert('A');
    /// let b = arena.insert('B');
    ///
    /// assert_eq!(arena.as_slice(), &['A', 'B']);
    /// assert_eq!(arena[a], 'A');
    /// assert_eq!(arena[b], 'B');
    ///
    /// arena.swap(0, 1);
    ///
    /// assert_eq!(arena.as_slice(), &['B', 'A']);
    /// assert_eq!(arena[a], 'A');
    /// assert_eq!(arena[b], 'B');
    /// ```
    #[inline]
    pub fn swap(&mut self, i: usize, j: usize) {
        assert!(i < self.len());
        assert!(j < self.len());

        if i == j {
            return;
        }

        self.values.swap(i, j);
        let slot_i = self.slots[i].value_slot;
        let slot_j = self.slots[j].value_slot;
        match &mut self.slots[slot_i] {
            Slot {
                value_slot,
                state: State::Used { value, .. },
            } => {
                *value_slot = slot_j;
                *value = j;
            }
            _ => unreachable!(),
        };
        match &mut self.slots[slot_j] {
            Slot {
                value_slot,
                state: State::Used { value, .. },
            } => {
                *value_slot = slot_i;
                *value = i;
            }
            _ => unreachable!(),
        };
    }

    fn quicksort<F: FnMut(&T, &T) -> Ordering>(
        &mut self,
        low: usize,
        high: usize,
        compare: &mut F,
    ) {
        if low + 1 >= high.wrapping_add(1) {
            return;
        }
        let p = {
            let (mut i, mut j) = (low, low);
            while i <= high {
                if compare(&self.values[i], &self.values[high]) == Ordering::Greater {
                    i += 1;
                } else {
                    self.swap(i, j);
                    i += 1;
                    j += 1;
                }
            }
            j - 1
        };
        self.quicksort(low, p.wrapping_sub(1), compare);
        self.quicksort(p + 1, high, compare);
    }

    /// Sorts the values in the arena, using the provided function, without
    /// invalidating their IDs.
    ///
    /// # Examples
    ///
    /// ```
    /// # use arena::Arena;
    /// let mut arena = Arena::new();
    /// let c = arena.insert('C');
    /// let a = arena.insert('A');
    /// let b = arena.insert('B');
    ///
    /// assert_eq!(arena.as_slice(), &['C', 'A', 'B']);
    /// assert_eq!(arena[a], 'A');
    /// assert_eq!(arena[b], 'B');
    /// assert_eq!(arena[c], 'C');
    ///
    /// arena.sort_by(|a, b| a.cmp(b));
    ///
    /// assert_eq!(arena.as_slice(), &['A', 'B', 'C']);
    /// assert_eq!(arena[a], 'A');
    /// assert_eq!(arena[b], 'B');
    /// assert_eq!(arena[c], 'C');
    /// ```
    #[inline]
    pub fn sort_by<F: FnMut(&T, &T) -> Ordering>(&mut self, mut compare: F) {
        if self.len() > 1 {
            self.quicksort(0, self.len() - 1, &mut compare);
        }
    }

    /// Returns the arena as a simple vector of its values.
    ///
    /// This simply discards the rest of the arena and just returns the vector
    /// that the arena was already using to store the values.
    ///
    /// # Examples
    ///
    /// ```
    /// # use arena::Arena;
    /// let mut arena = Arena::from(['A', 'B', 'C']);
    /// arena.insert('D');
    ///
    /// let vec = arena.to_vec();
    /// assert_eq!(&vec, &['A', 'B', 'C', 'D']);
    /// ```
    pub fn to_vec(self) -> Vec<T> {
        self.values
    }

    /// Returns an iterator that allows modifying each value.
    ///
    /// The iterator yields all items from start to end.
    ///
    /// # Examples
    ///
    /// ```
    /// # use arena::Arena;
    /// let mut arena = Arena::from([1, 2, 3]);
    ///
    /// for val in arena.iter_mut() {
    ///     *val *= 10;
    /// }
    ///
    /// assert_eq!(arena.as_slice(), &[10, 20, 30]);
    /// ```
    #[inline]
    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, T> {
        self.values.iter_mut()
    }

    /// Returns an iterator over all ID/value pairs in the arena.
    ///
    /// # Examples
    ///
    /// ```
    /// # use arena::Arena;
    /// let mut arena = Arena::new();
    /// let a = arena.insert('A');
    /// let b = arena.insert('B');
    /// let c = arena.insert('C');
    ///
    /// let mut pairs = arena.pairs();
    /// assert_eq!(pairs.next(), Some((a, &'A')));
    /// assert_eq!(pairs.next(), Some((b, &'B')));
    /// assert_eq!(pairs.next(), Some((c, &'C')));
    /// assert_eq!(pairs.next(), None);
    /// ```
    #[inline]
    pub fn pairs(&self) -> Pairs<'_, T> {
        Pairs {
            iter: self.values.iter().enumerate(),
            slots: &self.slots,
            #[cfg(feature = "uuid")]
            uuid: self.uuid,
        }
    }

    /// Returns a mutable iterator over all ID/value pairs in the arena.
    ///
    /// # Examples
    ///
    /// ```
    /// # use arena::Arena;
    /// let mut arena = Arena::new();
    /// let a = arena.insert('A');
    /// let b = arena.insert('B');
    /// let c = arena.insert('C');
    ///
    /// assert_eq!(arena.as_slice(), &['A', 'B', 'C']);
    ///
    /// for (id, val) in arena.pairs_mut() {
    ///     if id == a {
    ///         assert_eq!(*val, 'A');
    ///     } else if id == b {
    ///         assert_eq!(*val, 'B');
    ///     } else if id == c {
    ///         assert_eq!(*val, 'C');
    ///     } else {
    ///         unreachable!()
    ///     }
    /// }
    /// ```
    #[inline]
    pub fn pairs_mut(&mut self) -> PairsMut<'_, T> {
        PairsMut {
            iter: self.values.iter_mut().enumerate(),
            slots: &self.slots,
            #[cfg(feature = "uuid")]
            uuid: self.uuid,
        }
    }

    /// Returns an iterator over all IDs in the arena.
    ///
    /// # Examples
    ///
    /// ```
    /// # use arena::Arena;
    /// let mut arena = Arena::new();
    /// let a = arena.insert('A');
    /// let b = arena.insert('B');
    /// let c = arena.insert('C');
    ///
    /// let mut ids = arena.ids();
    /// assert_eq!(ids.next(), Some(a));
    /// assert_eq!(ids.next(), Some(b));
    /// assert_eq!(ids.next(), Some(c));
    /// assert_eq!(ids.next(), None);
    /// ```
    #[inline]
    pub fn ids(&self) -> Ids<'_, T> {
        Ids {
            iter: self.slots[..self.len()].iter().enumerate(),
            _ty: PhantomData,
            #[cfg(feature = "uuid")]
            uuid: self.uuid,
        }
    }
}

impl<T: Clone> Arena<T> {
    /// Adds all values from the slice to the arena.
    #[inline]
    pub fn extend_from_slice(&mut self, slice: &[T]) {
        self.values.reserve(slice.len());
        self.extend(slice.iter().cloned());
    }
}

impl<T: Ord> Arena<T> {
    /// Sorts the values in the arena, without invalidating their IDs.
    ///
    /// # Examples
    ///
    /// ```
    /// # use arena::Arena;
    /// let mut arena = Arena::new();
    /// let c = arena.insert('C');
    /// let a = arena.insert('A');
    /// let b = arena.insert('B');
    ///
    /// assert_eq!(arena.as_slice(), &['C', 'A', 'B']);
    /// assert_eq!(arena[a], 'A');
    /// assert_eq!(arena[b], 'B');
    /// assert_eq!(arena[c], 'C');
    ///
    /// arena.sort();
    ///
    /// assert_eq!(arena.as_slice(), &['A', 'B', 'C']);
    /// assert_eq!(arena[a], 'A');
    /// assert_eq!(arena[b], 'B');
    /// assert_eq!(arena[c], 'C');
    #[inline]
    pub fn sort(&mut self) {
        self.sort_by(|a, b| a.cmp(b));
    }
}

impl<T> Default for Arena<T> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Deref for Arena<T> {
    type Target = [T];

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.values.as_slice()
    }
}

impl<T> Index<ArenaId<T>> for Arena<T> {
    type Output = T;

    #[inline]
    fn index(&self, index: ArenaId<T>) -> &Self::Output {
        self.get(index).unwrap()
    }
}

impl<T> IndexMut<ArenaId<T>> for Arena<T> {
    #[inline]
    fn index_mut(&mut self, index: ArenaId<T>) -> &mut Self::Output {
        self.get_mut(index).unwrap()
    }
}

impl<T> Extend<T> for Arena<T> {
    #[inline]
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        for val in iter {
            self.insert(val);
        }
    }
}

impl<'a, T: Clone + 'a> Extend<&'a T> for Arena<T> {
    #[inline]
    fn extend<I: IntoIterator<Item = &'a T>>(&mut self, iter: I) {
        self.extend(iter.into_iter().cloned())
    }
}

impl<T> From<Vec<T>> for Arena<T> {
    fn from(values: Vec<T>) -> Self {
        let mut slots = Vec::new();
        let mut uid = 0;
        for i in 0..values.len() {
            slots.push(Slot {
                value_slot: i,
                state: State::Used { uid: uid, value: i },
            });
            uid += 1;
        }
        Self {
            values,
            slots,
            first_free: None,
            next_uid: uid,
            #[cfg(feature = "uuid")]
            uuid: Uuid::new_v4(),
        }
    }
}

impl<'a, T: Clone + 'a> From<&'a [T]> for Arena<T> {
    #[inline]
    fn from(values: &'a [T]) -> Self {
        Self::from_iter(values.iter().cloned())
    }
}

impl<'a, T: Clone + 'a> From<&'a mut [T]> for Arena<T> {
    #[inline]
    fn from(values: &'a mut [T]) -> Self {
        Self::from_iter(values.iter().cloned())
    }
}

impl<T, const N: usize> From<[T; N]> for Arena<T> {
    #[inline]
    fn from(values: [T; N]) -> Self {
        Self::from(Vec::from(values))
    }
}

impl<T> IntoIterator for Arena<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.values.into_iter()
    }
}

impl<T> FromIterator<T> for Arena<T> {
    #[inline]
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut arena = Arena::new();
        arena.extend(iter.into_iter());
        arena
    }
}

#[derive(Debug, Clone)]
struct Slot {
    value_slot: usize,
    state: State,
}

#[derive(Debug, Clone)]
enum State {
    Used { uid: u64, value: usize },
    Free { next_free: Option<usize> },
}

/// An ID assigned to a value when it was added to an arena.
///
/// Unlike an index, this ID will remain a valid handle to the value even
/// if other values are removed from the arena and the value vector gets
/// re-ordered.
///
/// They implement `Copy` and so can be passed around freely.
#[derive(Debug)]
pub struct ArenaId<T> {
    #[cfg(feature = "uuid")]
    uuid: Uuid,
    uid: u64,
    idx: usize,
    _ty: PhantomData<fn() -> T>,
}

// This sucks, but the following need to be implemented manually due to [derive] not currently handling PhantomData well.
// See: https://github.com/rust-lang/rust/issues/26925
impl<T> Clone for ArenaId<T> {
    #[inline]
    fn clone(&self) -> Self {
        Self {
            #[cfg(feature = "uuid")]
            uuid: Uuid::new_v4(),
            uid: self.uid,
            idx: self.idx,
            _ty: PhantomData,
        }
    }
}

impl<T> Copy for ArenaId<T> {}

impl<T> PartialEq for ArenaId<T> {
    #[cfg(feature = "uuid")]
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.uuid == other.uuid && self.uid == other.uid && self.idx == other.idx
    }

    #[cfg(not(feature = "uuid"))]
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.uid == other.uid && self.idx == other.idx
    }
}

impl<T> Eq for ArenaId<T> {}

impl<T> std::hash::Hash for ArenaId<T> {
    #[inline]
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        #[cfg(feature = "uuid")]
        self.uuid.hash(state);
        self.uid.hash(state);
        self.idx.hash(state);
    }
}

impl<T> PartialOrd for ArenaId<T> {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for ArenaId<T> {
    #[cfg(feature = "uuid")]
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        (self.uuid, self.uid, self.idx).cmp(&(other.uuid, other.uid, other.idx))
    }
    #[cfg(not(feature = "uuid"))]
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        (self.uid, self.idx).cmp(&(other.uid, other.idx))
    }
}

/// Iterator over an arena's ID/value pairs.
///
/// This struct is created by the [`pairs`](Arena::pairs) method on [`Arena`].
pub struct Pairs<'a, T> {
    iter: std::iter::Enumerate<std::slice::Iter<'a, T>>,
    slots: &'a [Slot],
    #[cfg(feature = "uuid")]
    uuid: Uuid,
}

impl<'a, T> Iterator for Pairs<'a, T> {
    type Item = (ArenaId<T>, &'a T);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let (idx, val) = self.iter.next()?;
        let idx = self.slots[idx].value_slot;
        match &self.slots[idx].state {
            State::Used { uid, .. } => Some((
                ArenaId::<T> {
                    #[cfg(feature = "uuid")]
                    uuid: self.uuid,
                    uid: *uid,
                    idx,
                    _ty: PhantomData,
                },
                val,
            )),
            _ => unreachable!(),
        }
    }
}

/// Mutable iterator over an arena's ID/value pairs.
///
/// This struct is created by the [`pairs_mut`](Arena::pairs_mut) method on [`Arena`].
pub struct PairsMut<'a, T> {
    iter: std::iter::Enumerate<std::slice::IterMut<'a, T>>,
    slots: &'a [Slot],
    #[cfg(feature = "uuid")]
    uuid: Uuid,
}

impl<'a, T> Iterator for PairsMut<'a, T> {
    type Item = (ArenaId<T>, &'a mut T);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let (idx, val) = self.iter.next()?;
        let idx = self.slots[idx].value_slot;
        match &self.slots[idx].state {
            State::Used { uid, .. } => Some((
                ArenaId::<T> {
                    #[cfg(feature = "uuid")]
                    uuid: self.uuid,
                    uid: *uid,
                    idx,
                    _ty: PhantomData,
                },
                val,
            )),
            _ => unreachable!(),
        }
    }
}

/// Iterator over an arena's IDs.
///
/// This struct is created by the [`ids`](Arena::ids) method on [`Arena`].
pub struct Ids<'a, T> {
    iter: std::iter::Enumerate<std::slice::Iter<'a, Slot>>,
    _ty: PhantomData<T>,
    #[cfg(feature = "uuid")]
    uuid: Uuid,
}

impl<'a, T> Iterator for Ids<'a, T> {
    type Item = ArenaId<T>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let (idx, slot) = self.iter.next()?;
        match &slot.state {
            State::Used { uid, .. } => Some(ArenaId::<T> {
                #[cfg(feature = "uuid")]
                uuid: self.uuid,
                uid: *uid,
                idx,
                _ty: PhantomData,
            }),
            _ => None,
        }
    }
}

#[cfg(feature = "serde")]
mod ser {
    use crate::State;
    use serde::de::Visitor;
    use serde::ser::SerializeStruct;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::fmt::Formatter;

    impl<T: Serialize> Serialize for crate::Arena<T> {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            let mut s = serializer.serialize_struct("Arena", 2)?;
            s.serialize_field("next_uid", &self.next_uid)?;

            let entries: Vec<Entry<'_, T>> = self
                .pairs()
                .map(|(id, val)| Entry {
                    uid: id.uid,
                    idx: id.idx,
                    val,
                })
                .collect();
            s.serialize_field("entries", &entries)?;

            s.end()
        }
    }

    impl<'de, T: Deserialize<'de>> Deserialize<'de> for crate::Arena<T> {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            let mut de: DeArena<T> = DeArena::deserialize(deserializer)?;

            de.entries.sort_by(|a, b| a.idx.cmp(&b.idx));

            let mut slots = Vec::new();
            let mut next_value_slot = 0;
            let mut next_value = 0;

            let mut first_free = None;
            for e in &de.entries {
                // push free slots until we reach the entry's index
                while slots.len() < e.idx {
                    slots.push(crate::Slot {
                        value_slot: de.entries[next_value_slot].idx,
                        state: crate::State::Free {
                            next_free: first_free.replace(slots.len()),
                        },
                    });
                    next_value_slot += 1;
                }

                // insert the entry
                slots.push(crate::Slot {
                    value_slot: de.entries[next_value_slot].idx,
                    state: crate::State::Used {
                        uid: e.uid,
                        value: next_value,
                    },
                });

                next_value_slot += 1;
                next_value += 1;
            }

            let values = de.entries.into_iter().map(|e| e.val).collect();

            Ok(Self {
                next_uid: de.next_uid,
                slots,
                values,
                first_free,
            })
        }
    }

    #[derive(Serialize)]
    struct Entry<'a, T> {
        uid: u64,
        idx: usize,
        val: &'a T,
    }

    #[derive(Deserialize)]
    struct DeEntry<T> {
        uid: u64,
        idx: usize,
        val: T,
    }

    #[derive(Deserialize)]
    struct DeArena<T> {
        next_uid: u64,
        entries: Vec<DeEntry<T>>,
    }
}

#[test]
fn rain_test() {
    let mut arena = Arena::new();
    let a = arena.insert("a");
    let b = arena.insert("b");
    let c = arena.insert("c");
    let d = arena.insert("d");
    let e = arena.insert("e");

    arena.remove(b);
    arena.remove(a);

    let f = arena.insert("f");
    let g = arena.insert("g");

    assert_eq!(*arena.get(c).unwrap(), "c");
    assert_eq!(*arena.get(d).unwrap(), "d");
    assert_eq!(*arena.get(e).unwrap(), "e");
    assert_eq!(*arena.get(f).unwrap(), "f");
    assert_eq!(*arena.get(g).unwrap(), "g");

    arena.remove(f);

    assert_eq!(*arena.get(c).unwrap(), "c");
    assert_eq!(*arena.get(d).unwrap(), "d");
    assert_eq!(*arena.get(g).unwrap(), "g");
    assert_eq!(*arena.get(e).unwrap(), "e");
}
