// C++ names them by `GLuint`; glow by a type per kind of object, an integer
// natively and a key to the browser's object on WebGL2.

use glow::HasContext;

/// The GL functions of one context.
pub type Gl = glow::Context;

pub type ProgramID = <Gl as HasContext>::Program;
pub type ShaderID = <Gl as HasContext>::Shader;
pub type BufferID = <Gl as HasContext>::Buffer;
pub type TextureID = <Gl as HasContext>::Texture;
pub type VertexArrayID = <Gl as HasContext>::VertexArray;
pub type FramebufferID = <Gl as HasContext>::Framebuffer;
pub type RenderbufferID = <Gl as HasContext>::Renderbuffer;
pub type UniformLocation = <Gl as HasContext>::UniformLocation;
