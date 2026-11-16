// Manages an ordered collection of elements and their `Arc<Impl>`s. The
// latter is itself stored in an `Arc`. Using immutability at the
// collection level allows us to short-circuit significant portions of the
// RenderStyle update logic via a simple pointer equality check, greatly
// improving performance. A change copies the vector of impls only while a
// snapshot from `get_impls` shares it, so a snapshot never changes.

use std::sync::Arc;

/// An element of a collection: it has an `Impl` type, a shared impl and an
/// id.
pub(crate) trait CollectionElement {
    type Impl: ?Sized;
    fn base_impl(&self) -> Arc<Self::Impl>;
    fn get_id(&self) -> String;
    /// Whether the element's id is `id`, read without copying its impl.
    fn has_id(&self, id: &str) -> bool;
    /// The id of an impl, which orders the impls of a persistent-order
    /// collection.
    fn impl_id(impl_: &Self::Impl) -> &str;
}

impl CollectionElement for dyn crate::source::Source {
    type Impl = dyn crate::source::Impl;
    fn base_impl(&self) -> Arc<Self::Impl> {
        crate::source::Source::base_impl(self)
    }
    fn get_id(&self) -> String {
        crate::source::Source::get_id(self)
    }
    fn has_id(&self, id: &str) -> bool {
        self.base().has_id(id)
    }
    fn impl_id(impl_: &Self::Impl) -> &str {
        &impl_.id
    }
}

pub type ImmutableVector<I> = Arc<Vec<Arc<I>>>;

pub(crate) struct CollectionBase<T: CollectionElement + ?Sized> {
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
    pub(crate) fn size(&self) -> usize {
        self.wrappers.len()
    }

    #[must_use]
    pub(crate) fn get(&self, id: &str) -> Option<&T> {
        let i = self.index(id);
        if i < self.size() { Some(self.wrappers[i].as_ref()) } else { None }
    }

    pub(crate) fn get_wrappers(&self) -> Vec<&T> {
        self.wrappers.iter().map(AsRef::as_ref).collect()
    }

    #[must_use]
    pub(crate) fn get_impls(&self) -> ImmutableVector<T::Impl> {
        Arc::clone(&self.impls)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &T> {
        self.wrappers.iter().map(AsRef::as_ref)
    }

    pub(crate) fn clear(&mut self) {
        // mutate(impls, ...): copy-on-write into a new vector.
        self.impls = Arc::new(Vec::new());
        self.wrappers.clear();
    }

    #[must_use]
    pub(crate) fn index(&self, id: &str) -> usize {
        self.wrappers.iter().position(|e| e.has_id(id)).unwrap_or(self.wrappers.len())
    }

    fn add_at(&mut self, wrapper_index: usize, impl_index: usize, wrapper: Box<T>) -> &mut T {
        debug_assert!(wrapper_index <= self.size());
        debug_assert!(impl_index <= self.size());
        Arc::make_mut(&mut self.impls).insert(impl_index, wrapper.base_impl());

        self.wrappers.insert(wrapper_index, wrapper);
        self.wrappers[wrapper_index].as_mut()
    }

    fn remove_at(&mut self, wrapper_index: usize, impl_index: usize) -> Option<Box<T>> {
        if wrapper_index >= self.size() {
            return None;
        }
        debug_assert!(impl_index < self.size());

        Arc::make_mut(&mut self.impls).remove(impl_index);

        Some(self.wrappers.remove(wrapper_index))
    }
}

/// A collection whose impls are sorted by id.
pub(crate) struct CollectionWithPersistentOrder<T: CollectionElement + ?Sized> {
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
    pub(crate) fn add(&mut self, wrapper: Box<T>) -> &mut T {
        let i = self.impls_index(&wrapper.get_id());
        let size = self.base.size();
        self.base.add_at(size, i, wrapper)
    }

    pub(crate) fn remove(&mut self, id: &str) -> Option<Box<T>> {
        let wrapper_index = self.base.index(id);
        let impl_index = self.impls_index(id);
        self.base.remove_at(wrapper_index, impl_index)
    }

    /// The index of the first impl whose id is not less than `id`.
    fn impls_index(&self, id: &str) -> usize {
        self.base.impls.partition_point(|a| T::impl_id(a) < id)
    }

    /// Whether `impls`, which hold an impl for each element in id order,
    /// are the elements' current impls. Takes O(n log n): it finds each
    /// element's impl by binary search.
    pub(crate) fn are_current(&self, impls: &[Arc<T::Impl>]) -> bool {
        impls.len() == self.base.size()
            && self.base.wrappers.iter().all(|wrapper| {
                let current = wrapper.base_impl();
                let id = T::impl_id(&current);
                let index = impls.partition_point(|impl_| T::impl_id(impl_) < id);
                impls.get(index).is_some_and(|impl_| Arc::ptr_eq(impl_, &current))
            })
    }

    /// The elements' current impls, in id order.
    pub(crate) fn current_impls(&self) -> Vec<Arc<T::Impl>> {
        let mut impls: Vec<Arc<T::Impl>> = self.base.wrappers.iter().map(|wrapper| wrapper.base_impl()).collect();
        impls.sort_unstable_by(|a, b| T::impl_id(a).cmp(T::impl_id(b)));
        impls
    }
}
