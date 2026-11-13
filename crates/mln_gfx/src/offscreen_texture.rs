use std::any::Any;

use mln_util::image::PremultipliedImage;

use crate::command_encoder::CommandEncoder;
use crate::frame::DrawTarget;
use crate::texture2d::Texture2DPtr;

/// A texture that render passes draw into, of the size it was made with:
/// its color texture's size isn't to be changed.
pub trait OffscreenTexture: Any {
    /// What a render pass draws into the texture through.
    fn draw_target(&self) -> &dyn DrawTarget;
    fn is_renderable(&self) -> bool;
    fn read_still_image(&self) -> PremultipliedImage;
    fn get_texture(&self) -> Texture2DPtr;
    /// Submits what `encoder` recorded so far, after the passes that drew
    /// into the texture, so that what samples it later sees them.
    fn submit(&self, encoder: &dyn CommandEncoder);
}
