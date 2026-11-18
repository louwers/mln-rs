use std::rc::Rc;

use mln_gfx::gpu::ResourceId;

use crate::gfx::buffer::SharedBuffer;
use crate::gfx::release::{Release, ReleaseQueue};
use crate::gfx::resources::SharedGpuTexture;

/// A handle of a buffer or texture that bindings or geometry were made of.
#[expect(dead_code, reason = "held for what it keeps in the context's store, not read")]
pub(crate) enum Held {
    Buffer(SharedBuffer),
    Texture(SharedGpuTexture),
}

/// Bindings or geometry of the context's [`Gpu`](mln_gfx::gpu::Gpu), which
/// go to the context's release queue when this drops, and the handles of
/// what they were made of, which stay until then: the queue destroys them
/// after the bindings or geometry.
pub(crate) struct Owned<T: Copy + Into<ResourceId>> {
    id: T,
    queue: Rc<ReleaseQueue>,
    _held: Vec<Held>,
}

impl<T: Copy + Into<ResourceId>> Owned<T> {
    /// `id`, made of `held`, which goes to `queue` when this drops.
    pub(crate) fn new(id: T, held: Vec<Held>, queue: &Rc<ReleaseQueue>) -> Owned<T> {
        Owned { id, queue: Rc::clone(queue), _held: held }
    }

    pub(crate) fn id(&self) -> T {
        self.id
    }
}

impl<T: Copy + Into<ResourceId>> Drop for Owned<T> {
    fn drop(&mut self) {
        self.queue.push(Release::Resource(self.id.into()));
    }
}
