use std::rc::Rc;

use mln_gfx::gpu::ResourceId;

use crate::draw::buffer::Buffer;
use crate::draw::indexes::SharedIndexes;
use crate::draw::release::{Release, ReleaseQueue};
use crate::draw::resources::SharedGpuTexture;
use crate::draw::store::{Shared, VerticesKey};

/// A counted handle of what other owners share, which bindings or geometry
/// made of it hold.
#[derive(Clone)]
#[expect(dead_code, reason = "held for what it keeps, not read")]
pub(crate) enum Held {
    Vertices(Shared<VerticesKey>),
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
