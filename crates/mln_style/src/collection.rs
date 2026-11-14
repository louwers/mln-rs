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

impl CollectionElement for dyn crate::layer::Layer {
    type Impl = dyn crate::layer::Impl;
    fn base_impl(&self) -> Arc<Self::Impl> {
        crate::layer::Layer::base_impl(self)
    }
    fn get_id(&self) -> String {
        crate::layer::Layer::get_id(self)
    }
    fn has_id(&self, id: &str) -> bool {
        crate::layer::Layer::base_impl_ref(self).id == id
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

    pub(crate) fn get_mut(&mut self, id: &str) -> Option<&mut T> {
        let i = self.index(id);
        if i < self.size() { Some(self.wrappers[i].as_mut()) } else { None }
    }

    pub(crate) fn get_wrappers(&self) -> Vec<&T> {
        self.wrappers.iter().map(AsRef::as_ref).collect()
    }

    pub(crate) fn get_wrappers_mut(&mut self) -> Vec<&mut T> {
        self.wrappers.iter_mut().map(AsMut::as_mut).collect()
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

    /// Must be called whenever an element of the collection is internally
    /// mutated.
    fn update_at(&mut self, impl_index: usize, base_impl: Arc<T::Impl>) {
        if impl_index >= self.size() {
            debug_assert!(false);
            return;
        }
        Arc::make_mut(&mut self.impls)[impl_index] = base_impl;
    }
}

/// A collection whose impls are in wrapper order.
pub(crate) struct Collection<T: CollectionElement + ?Sized> {
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
    pub(crate) fn add(&mut self, wrapper: Box<T>, before: Option<&str>) -> &mut T {
        let idx = match before {
            Some(before) => self.base.index(before),
            None => self.base.size(),
        };
        self.base.add_at(idx, idx, wrapper)
    }

    /// Adds `wrapper` as `add` does, unless an element has its id: then
    /// returns it back. Looks through the collection once.
    pub(crate) fn add_unique(&mut self, wrapper: Box<T>, before: Option<&str>) -> Result<&mut T, Box<T>> {
        let id = wrapper.get_id();
        let size = self.base.size();
        let mut idx = size;
        for (i, element) in self.base.wrappers.iter().enumerate() {
            if element.has_id(&id) {
                return Err(wrapper);
            }
            if idx == size && before.is_some_and(|before| element.has_id(before)) {
                idx = i;
            }
        }
        Ok(self.base.add_at(idx, idx, wrapper))
    }

    /// Adds `wrappers` in order above every element. Their ids must differ
    /// from each other and from the collection's.
    pub(crate) fn extend(&mut self, wrappers: impl IntoIterator<Item = Box<T>>) {
        let impls = Arc::make_mut(&mut self.base.impls);
        for wrapper in wrappers {
            impls.push(wrapper.base_impl());
            self.base.wrappers.push(wrapper);
        }
    }

    pub(crate) fn remove(&mut self, id: &str) -> Option<Box<T>> {
        let idx = self.base.index(id);
        self.base.remove_at(idx, idx)
    }

    /// `update(const T& wrapper)` (by the wrapper's id).
    pub(crate) fn update(&mut self, id: &str) {
        let idx = self.base.index(id);
        if idx < self.base.size() {
            let base_impl = self.base.wrappers[idx].base_impl();
            self.base.update_at(idx, base_impl);
        } else {
            debug_assert!(false);
        }
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
}
