// Ports include/mln/renderer/renderer_frontend.hpp

use std::sync::Arc;

use crate::renderer_event::RendererEvents;
use crate::update_parameters::UpdateParameters;

/// The RenderFrontend is the bridge between the Map and platform used to
/// update and observe the Renderer.
///
/// It hides any threading specifics and always replies on the original
/// thread.
///
/// The methods take `&self`: the Map keeps a handle to its frontend (C++
/// keeps a reference) while the platform also uses it, so implementations
/// use interior mutability.
pub trait RendererFrontend {
    /// Must synchronously clean up the Renderer if set.
    fn reset(&self);

    /// Gives the renderer the map's queue for its events.
    fn set_event_queue(&self, events: RendererEvents);

    /// Coalescing updates is up to the implementer.
    fn update(&self, parameters: Arc<UpdateParameters>);

    // TODO(port): `getThreadPool()` (the TaggedScheduler the style and
    // render sources use), with the thread pool.
}
