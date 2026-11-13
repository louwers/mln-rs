// C++ names them by `GLuint`; glow by a type per kind of object, an integer
// natively and a key to the browser's object on WebGL2.

use glow::HasContext;

/// The GL functions of one context.
pub(crate) type Gl = glow::Context;

pub(crate) type ProgramID = <Gl as HasContext>::Program;
pub(crate) type ShaderID = <Gl as HasContext>::Shader;
pub(crate) type BufferID = <Gl as HasContext>::Buffer;
pub(crate) type TextureID = <Gl as HasContext>::Texture;
pub(crate) type VertexArrayID = <Gl as HasContext>::VertexArray;
pub(crate) type FramebufferID = <Gl as HasContext>::Framebuffer;
pub(crate) type RenderbufferID = <Gl as HasContext>::Renderbuffer;
pub(crate) type UniformLocation = <Gl as HasContext>::UniformLocation;
