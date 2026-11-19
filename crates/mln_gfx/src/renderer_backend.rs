use std::ops::DerefMut;

use crate::gpu::Gpu;

/// What a renderer draws with on a device: the backend's [`Gpu`], which the
/// backend owns and lends. What it draws into is a `FrameTarget` built on
/// it, which takes the same `Gpu` for each call.
pub trait RendererBackend: 'static {
    /// The backend's `Gpu`, which owns the renderer's resources and runs its
    /// frames.
    type Gpu: Gpu;

    /// The lend of [`gpu`](Self::gpu): it derefs to the backend's `Gpu`,
    /// and keeps what the `Gpu`'s calls need ready until it drops, such as
    /// OpenGL's context current.
    type Scope<'a>: DerefMut<Target = Self::Gpu>
    where
        Self: 'a;

    /// Lends the backend's `Gpu` until the returned scope drops. A caller
    /// takes one scope around many calls rather than one for each, since a
    /// backend may make its device ready for each scope.
    fn gpu(&mut self) -> Self::Scope<'_>;
}
