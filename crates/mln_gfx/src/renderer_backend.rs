use std::any::Any;
use std::cell::RefMut;

use crate::gpu::Gpu;

/// What a renderer draws with on a device: the backend's [`Gpu`]. What it
/// draws into is a `FrameTarget` built on it.
pub trait RendererBackend: Any {
    /// The backend's [`Gpu`], which owns the renderer's resources and runs
    /// its frames.
    fn gpu(&self) -> RefMut<'_, dyn Gpu>;

    /// Keeps what the backend's calls need ready across many of them, such
    /// as OpenGL's context current, until the hold drops, as when a panic
    /// unwinds. Holds nest. A backend that needs nothing holds nothing.
    fn hold(&self) -> Hold<'_> {
        Hold::default()
    }
}

impl dyn RendererBackend {
    /// The concrete type behind this trait object, if it is a `T`.
    pub fn downcast_ref<T: RendererBackend>(&self) -> Option<&T> {
        let any: &dyn Any = self;
        any.downcast_ref()
    }
}

/// What a backend keeps ready while a [`RendererBackend::hold`] lasts,
/// released when it drops.
#[derive(Default)]
#[must_use = "the hold ends when dropped"]
pub struct Hold<'a> {
    release: Option<Box<dyn FnOnce() + 'a>>,
}

impl<'a> Hold<'a> {
    /// A hold that `release` ends.
    pub fn new(release: impl FnOnce() + 'a) -> Hold<'a> {
        Hold { release: Some(Box::new(release)) }
    }
}

impl Drop for Hold<'_> {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            release();
        }
    }
}
