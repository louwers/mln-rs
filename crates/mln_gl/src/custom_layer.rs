// What a custom layer's host gets of an OpenGL frame (C++: the base
// `style::CustomLayerRenderParameters`, with the context current).

use std::rc::Rc;

use mln_util::size::Size;

use crate::context::Context;
use crate::types::{FramebufferID, Gl};

/// The session's OpenGL context, current, outside the map's framebuffer, for
/// a custom layer's prerender. Draw into framebuffers of your own; the map
/// binds its own again before it draws.
pub struct CustomLayerEncoder {
    pub(crate) context: Rc<Context>,
}

impl CustomLayerEncoder {
    /// The GL functions of the current context.
    #[must_use]
    pub fn gl(&self) -> &Gl {
        self.context.gl()
    }
}

/// The map's framebuffer at a custom layer's place in the style, bound, with
/// the viewport covering it.
///
/// The vertex array bound when the host starts is 0. The host can change the
/// program, vertex array, buffer bindings, framebuffer, viewport, textures and
/// the active unit, uniform buffer bindings, and the scissor, depth, stencil,
/// blend, color mask, cull face and line width state: the map sets what its
/// next draws need again. It leaves any other state as it found it, such as
/// sampler objects, the pixel pack and unpack buffers and parameters, polygon
/// offset and rasterizer discard, and doesn't clear the framebuffer.
pub struct CustomLayerPass {
    pub(crate) context: Rc<Context>,
    /// The framebuffer the map draws into, none for the default one.
    pub framebuffer: Option<FramebufferID>,
    /// Its size in pixels.
    pub size: Size,
}

impl CustomLayerPass {
    /// The GL functions of the current context.
    #[must_use]
    pub fn gl(&self) -> &Gl {
        self.context.gl()
    }
}
