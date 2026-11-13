use std::any::Any;
use std::rc::Rc;

/// The base of all shader programs across backends, registered with a
/// `ShaderRegistry`. Reach the concrete program with
/// `as_any().downcast_ref::<T>()`.
pub trait Shader: Any {
    /// Get the type name of this shader
    fn type_name(&self) -> &'static str;
    fn as_any(&self) -> &dyn Any;
}

pub type ShaderPtr = Rc<dyn Shader>;
