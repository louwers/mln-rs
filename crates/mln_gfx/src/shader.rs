// Ports include/mln/gfx/shader.hpp

use std::any::Any;
use std::rc::Rc;

/// `gfx::Shader`: base of all programs across backends; registered with a
/// `ShaderRegistry`. `to<T>()` becomes a downcast through `dyn Any`.
pub trait Shader: Any {
    /// Get the type name of this shader
    fn type_name(&self) -> &'static str;
}

pub type ShaderPtr = Rc<dyn Shader>;
