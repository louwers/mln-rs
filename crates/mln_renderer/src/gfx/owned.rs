use std::rc::Rc;

use mln_gfx::gpu::ResourceId;
use mln_gfx::released::Released;

/// A resource of the context's [`Gpu`](mln_gfx::gpu::Gpu) that goes to the
/// context's release queue when this drops.
pub(crate) struct Owned<T: Copy + Into<ResourceId>> {
    id: T,
    released: Rc<Released>,
}

impl<T: Copy + Into<ResourceId>> Owned<T> {
    pub(crate) fn new(id: T, released: &Rc<Released>) -> Owned<T> {
        Owned { id, released: Rc::clone(released) }
    }

    pub(crate) fn id(&self) -> T {
        self.id
    }
}

impl<T: Copy + Into<ResourceId>> Drop for Owned<T> {
    fn drop(&mut self) {
        self.released.resource(self.id);
    }
}
