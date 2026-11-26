//! The renderer's release queue: what it let go of on its backend's `Gpu`,
//! which the context destroys in a batch, each resource after what was made
//! of it.

use std::cell::Cell;

use mln_gfx::gpu::{BufferId, BufferUsage, ResourceId, TextureId};

use crate::draw::store::StoreKey;

/// Something the renderer let go of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Release {
    /// A store entry whose last handle dropped, which the context takes out
    /// of its store.
    Entry(StoreKey),
    /// A buffer, of a usage and the bytes it takes, which the rendering
    /// stats count until the context destroys it.
    Buffer(BufferId, BufferUsage, usize),
    /// A texture of the bytes it takes, which the rendering stats count
    /// until the context destroys it.
    Texture(TextureId, usize),
    /// Any other resource of the backend's `Gpu`: bindings, geometry, an
    /// attachment, or one of the context's samplers, pipelines and shaders.
    Resource(ResourceId),
}

/// The queue that handles and owned resources go to when they drop. The
/// context's device owns it, and each handle and owned resource holds it to
/// push itself.
#[derive(Default)]
pub(crate) struct ReleaseQueue(Cell<Vec<Release>>);

impl ReleaseQueue {
    pub(crate) fn push(&self, release: Release) {
        let mut releases = self.0.take();
        releases.push(release);
        self.0.set(releases);
    }

    /// Moves what was released so far into `into`, which is empty, and keeps
    /// its storage for what comes next.
    pub(crate) fn take_into(&self, into: &mut Vec<Release>) {
        debug_assert_eq!(into.len(), 0, "a batch left undrained");
        let mut releases = self.0.take();
        std::mem::swap(&mut releases, into);
        self.0.set(releases);
    }
}

/// When the context destroys `resource` in a batch: bindings and geometry
/// before the buffers and textures they were made of, and the samplers,
/// pipelines and shaders that last for the renderer's life after them.
pub(crate) fn destroy_order(resource: &ResourceId) -> u8 {
    match resource {
        ResourceId::Bindings(_) | ResourceId::Geometry(_) | ResourceId::Readback(_) => 0,
        ResourceId::Buffer(_) | ResourceId::Texture(_) => 1,
        ResourceId::Pipeline(_) => 2,
        ResourceId::Sampler(_) => 3,
        ResourceId::Shader(_) => 4,
    }
}
