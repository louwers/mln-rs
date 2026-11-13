use std::any::Any;

/// The base of all shader programs across backends, registered with a
/// `ShaderRegistry`. Reach the concrete program with a downcast through
/// `dyn Any`.
pub trait Shader: Any {
    /// Get the type name of this shader
    fn type_name(&self) -> &'static str;
}
