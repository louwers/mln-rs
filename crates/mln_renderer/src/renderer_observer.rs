// Ports include/mln/renderer/renderer_observer.hpp

use mln_tile_id::OverscaledTileID;
pub use mln_util::exception::ExceptionPtr;

use crate::tile::tile::TileOperation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum RenderMode {
    Partial,
    Full,
}

/// `RendererObserver`. All methods have the C++ default (empty) bodies.
///
/// C++ has four `onDidFinishRenderingFrame` overloads that forward to each
/// other; the port keeps the one the renderer calls (with the stats).
// TODO(port): `gfx::RenderingStats` in on_did_finish_rendering_frame,
// `onRegisterShaders`, shader compile callbacks, glyph callbacks,
// `SymbolErrorObserver::onSymbolError`.
pub trait RendererObserver {
    /// Signals that a repaint is required.
    fn on_invalidate(&self) {}

    /// Resource failed to download / parse.
    fn on_resource_error(&self, _error: ExceptionPtr) {}

    /// First frame.
    fn on_will_start_rendering_map(&self) {}

    /// Start of frame, initial is the first frame for this map.
    fn on_will_start_rendering_frame(&self) {}

    /// End of frame, booleans flags that a repaint is required and that
    /// placement changed.
    fn on_did_finish_rendering_frame(&self, _mode: RenderMode, _repaint: bool, _placement_changed: bool) {}

    /// Final frame.
    fn on_did_finish_rendering_map(&self) {}

    /// Style is missing an image. The default calls `done` right away.
    fn on_style_image_missing(&self, _id: &str, done: &mut dyn FnMut()) {
        done();
    }

    fn on_remove_unused_style_images(&self, _ids: &[String]) {}

    /// Tile loading.
    fn on_tile_action(&self, _op: TileOperation, _id: &OverscaledTileID, _source_id: &str) {}

    /// Render layer or drawable failed.
    fn on_render_error(&self, _error: ExceptionPtr) {}
}
