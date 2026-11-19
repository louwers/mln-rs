use std::rc::Rc;

use mln_gfx::gpu::ResourceId;

use crate::gfx::buffer::Buffer;
use crate::gfx::indexes::SharedIndexes;
use crate::gfx::release::{Release, ReleaseQueue};
use crate::gfx::resources::SharedGpuTexture;
use crate::gfx::vertices::SharedVertices;

/// A counted handle of what other owners share, which bindings or geometry
/// made of it hold.
#[derive(Clone)]
#[expect(dead_code, reason = "held for what it keeps, not read")]
pub(crate) enum Held {
    Vertices(SharedVertices),
    Indexes(SharedIndexes),
    /// A buffer of default attribute values, which the context shares.
    Buffer(Rc<Buffer>),
    Texture(SharedGpuTexture),
}

/// Bindings or geometry of the context's [`Gpu`](mln_gfx::gpu::Gpu), which
/// go to the context's release queue when this drops. It holds what it was
/// made of that others share, which goes after it; its owner drops it no
/// later than what it was made of that the owner owns.
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
