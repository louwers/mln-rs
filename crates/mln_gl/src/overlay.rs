// What a window target's overlay draws over each frame before it is
// presented (an app's controls).

use std::sync::Arc;

use mln_gfx::Size;
use mln_gfx::stencil_mode::StencilMode;

use crate::renderer_backend::GlContext;
use crate::types::Gl;

/// What an overlay draws into: the window's framebuffer, bound, through the
/// GL functions of the backend's context.
pub struct OverlayTarget<'a> {
    pub gl: &'a Arc<Gl>,
    /// The size of the framebuffer, in pixels.
    pub size: Size,
}

/// Draws over a frame before it is presented. It may leave any GL state
/// behind but the framebuffer binding: the context forgets what it had set.
pub type Overlay = Box<dyn FnMut(&OverlayTarget<'_>)>;

/// Draws `overlay` into the default framebuffer of `size` pixels, with
/// `context` current.
pub(crate) fn draw(overlay: &mut Overlay, context: &mut GlContext, size: Size) {
    let (gl, state) = context.parts();
    state.bind_framebuffer(gl, None);
    state.set_scissor(gl, None);
    state.set_viewport(gl, (0, 0, size.width as i32, size.height as i32));
    state.set_stencil_mode(gl, &StencilMode::disabled());
    overlay(&OverlayTarget { gl: context.gl_shared(), size });
    context.parts().1.set_dirty();
}
