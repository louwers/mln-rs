use std::any::Any;
use std::rc::Rc;

use mln_util::identity::SimpleIdentity;

use crate::shader::Shader;
use crate::vertex_attribute::VertexAttributeArray;

pub trait ShaderProgramBase: Shader {
    /// The program id assigned at construction.
    fn get_id(&self) -> SimpleIdentity;
    /// Gets the sampler location for a texture id
    fn get_sampler_location(&self, id: usize) -> Option<usize>;
    /// Get the available vertex attributes and their default values
    fn get_vertex_attributes(&self) -> &VertexAttributeArray;
    /// Get the available instance attributes and their default values
    fn get_instance_attributes(&self) -> &VertexAttributeArray;
}

impl dyn ShaderProgramBase {
    /// The concrete type behind this trait object, if it is a `T`.
    pub fn downcast_ref<T: ShaderProgramBase>(&self) -> Option<&T> {
        (self as &dyn Any).downcast_ref()
    }
}

pub type ShaderProgramBasePtr = Rc<dyn ShaderProgramBase>;

/// Pointer equality for shader handles (`shader == value` in C++).
#[must_use]
pub fn same_shader(a: &Option<ShaderProgramBasePtr>, b: &Option<ShaderProgramBasePtr>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => std::ptr::addr_eq(Rc::as_ptr(a), Rc::as_ptr(b)),
        (None, None) => true,
        _ => false,
    }
}
