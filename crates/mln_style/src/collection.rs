// Ports src/mln/style/collection.hpp.
//
// Manages an ordered collection of elements and their `Arc<Impl>`s. The
// latter is itself stored in an `Arc`. Using immutability at the
// collection level allows us to short-circuit significant portions of the
// RenderStyle update logic via a simple pointer equality check, greatly
// improving performance.

use std::sync::Arc;

/// What C++ requires of an element type: an `Impl` type, an
/// `Immutable<Impl> baseImpl` member and `getID()`.
pub trait CollectionElement {
    type Impl: ?Sized;
    fn base_impl(&self) -> Arc<Self::Impl>;
    fn get_id(&self) -> String;
    /// `impl->id` (used to order the impls of a persistent-order collection).
    fn impl_id(impl_: &Self::Impl) -> &str;
}

impl CollectionElement for dyn crate::layer::Layer {
    type Impl = dyn crate::layer::Impl;
    fn base_impl(&self) -> Arc<Self::Impl> {
        crate::layer::Layer::base_impl(self)
    }
    fn get_id(&self) -> String {
        crate::layer::Layer::get_id(self)
    }
    fn impl_id(impl_: &Self::Impl) -> &str {
        &impl_.id
    }
}

impl CollectionElement for dyn crate::source::Source {
    type Impl = dyn crate::source::Impl;
    fn base_impl(&self) -> Arc<Self::Impl> {
        crate::source::Source::base_impl(self)
    }
    fn get_id(&self) -> String {
        crate::source::Source::get_id(self)
    }
    fn impl_id(impl_: &Self::Impl) -> &str {
        &impl_.id
    }
}

pub type ImmutableVector<I> = Arc<Vec<Arc<I>>>;

/// `CollectionBase<T>`.
pub struct CollectionBase<T: CollectionElement + ?Sized> {
    wrappers: Vec<Box<T>>,
    impls: ImmutableVector<T::Impl>,
}

impl<T: CollectionElement + ?Sized> Default for CollectionBase<T> {
    fn default() -> Self {
        CollectionBase { wrappers: Vec::new(), impls: Arc::new(Vec::new()) }
    }
}

impl<T: CollectionElement + ?Sized> CollectionBase<T> {
    #[must_use]
    pub fn size(&self) -> usize {
        self.wrappers.len()
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<&T> {
        let i = self.index(id);
        if i < self.size() { Some(self.wrappers[i].as_ref()) } else { None }
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut T> {
        let i = self.index(id);
        if i < self.size() { Some(self.wrappers[i].as_mut()) } else { None }
    }

    pub fn get_wrappers(&self) -> Vec<&T> {
        self.wrappers.iter().map(std::convert::AsRef::as_ref).collect()
    }

    pub fn get_wrappers_mut(&mut self) -> Vec<&mut T> {
        self.wrappers.iter_mut().map(std::convert::AsMut::as_mut).collect()
    }

    #[must_use]
    pub fn get_impls(&self) -> ImmutableVector<T::Impl> {
        self.impls.clone()
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.wrappers.iter().map(std::convert::AsRef::as_ref)
    }

    pub fn clear(&mut self) {
        // mutate(impls, ...): copy-on-write into a new vector.
        self.impls = Arc::new(Vec::new());
        self.wrappers.clear();
    }

    #[must_use]
    pub fn index(&self, id: &str) -> usize {
        self.wrappers.iter().position(|e| e.get_id() == id).unwrap_or(self.wrappers.len())
    }

    fn add_at(&mut self, wrapper_index: usize, impl_index: usize, wrapper: Box<T>) -> &mut T {
        debug_assert!(wrapper_index <= self.size());
        debug_assert!(impl_index <= self.size());
        let mut impls = (*self.impls).clone();
        impls.insert(impl_index, wrapper.base_impl());
        self.impls = Arc::new(impls);

        self.wrappers.insert(wrapper_index, wrapper);
        self.wrappers[wrapper_index].as_mut()
    }

    fn remove_at(&mut self, wrapper_index: usize, impl_index: usize) -> Option<Box<T>> {
        if wrapper_index >= self.size() {
            return None;
        }
        debug_assert!(impl_index < self.size());

        let mut impls = (*self.impls).clone();
        impls.remove(impl_index);
        self.impls = Arc::new(impls);

        Some(self.wrappers.remove(wrapper_index))
    }

    /// Must be called whenever an element of the collection is internally
    /// mutated.
    fn update_at(&mut self, impl_index: usize, base_impl: Arc<T::Impl>) {
        if impl_index >= self.size() {
            debug_assert!(false);
            return;
        }
        let mut impls = (*self.impls).clone();
        impls[impl_index] = base_impl;
        self.impls = Arc::new(impls);
    }
}

/// `Collection<T, false>`: impls in wrapper order.
pub struct Collection<T: CollectionElement + ?Sized> {
    base: CollectionBase<T>,
}

impl<T: CollectionElement + ?Sized> Default for Collection<T> {
    fn default() -> Self {
        Collection { base: CollectionBase::default() }
    }
}

impl<T: CollectionElement + ?Sized> std::ops::Deref for Collection<T> {
    type Target = CollectionBase<T>;
    fn deref(&self) -> &CollectionBase<T> {
        &self.base
    }
}

impl<T: CollectionElement + ?Sized> std::ops::DerefMut for Collection<T> {
    fn deref_mut(&mut self) -> &mut CollectionBase<T> {
        &mut self.base
    }
}

impl<T: CollectionElement + ?Sized> Collection<T> {
    pub fn add(&mut self, wrapper: Box<T>, before: Option<&str>) -> &mut T {
        let idx = match before {
            Some(before) => self.base.index(before),
            None => self.base.size(),
        };
        self.base.add_at(idx, idx, wrapper)
    }

    pub fn remove(&mut self, id: &str) -> Option<Box<T>> {
        let idx = self.base.index(id);
        self.base.remove_at(idx, idx)
    }

    /// `update(const T& wrapper)` (by the wrapper's id).
    pub fn update(&mut self, id: &str) {
        let idx = self.base.index(id);
        if idx < self.base.size() {
            let base_impl = self.base.wrappers[idx].base_impl();
            self.base.update_at(idx, base_impl);
        } else {
            debug_assert!(false);
        }
    }
}

/// `Collection<T, true>` (`CollectionWithPersistentOrder<T>`): impls sorted by id.
pub struct CollectionWithPersistentOrder<T: CollectionElement + ?Sized> {
    base: CollectionBase<T>,
}

impl<T: CollectionElement + ?Sized> Default for CollectionWithPersistentOrder<T> {
    fn default() -> Self {
        CollectionWithPersistentOrder { base: CollectionBase::default() }
    }
}

impl<T: CollectionElement + ?Sized> std::ops::Deref for CollectionWithPersistentOrder<T> {
    type Target = CollectionBase<T>;
    fn deref(&self) -> &CollectionBase<T> {
        &self.base
    }
}

impl<T: CollectionElement + ?Sized> std::ops::DerefMut for CollectionWithPersistentOrder<T> {
    fn deref_mut(&mut self) -> &mut CollectionBase<T> {
        &mut self.base
    }
}

impl<T: CollectionElement + ?Sized> CollectionWithPersistentOrder<T> {
    pub fn add(&mut self, wrapper: Box<T>) -> &mut T {
        let i = self.impls_index(&wrapper.get_id());
        let size = self.base.size();
        self.base.add_at(size, i, wrapper)
    }

    pub fn remove(&mut self, id: &str) -> Option<Box<T>> {
        let wrapper_index = self.base.index(id);
        let impl_index = self.impls_index(id);
        self.base.remove_at(wrapper_index, impl_index)
    }

    pub fn update(&mut self, id: &str) {
        let impl_index = self.impls_index(id);
        let idx = self.base.index(id);
        if idx < self.base.size() {
            let base_impl = self.base.wrappers[idx].base_impl();
            self.base.update_at(impl_index, base_impl);
        } else {
            debug_assert!(false);
        }
    }

    /// `std::lower_bound` by `impl->id`.
    fn impls_index(&self, id: &str) -> usize {
        self.base.impls.partition_point(|a| T::impl_id(a) < id)
    }
}
