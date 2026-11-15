use std::rc::Rc;

use crate::gpu::{ShaderKey, VertexStep};
use crate::shaders::program_info::ProgramInfo;
use crate::vertex_attribute::VertexAttributeArray;

/// A built-in program with its defines: the key a pipeline compiles it from,
/// and what it reads.
pub struct ShaderProgram {
    key: ShaderKey,
    info: &'static ProgramInfo,
    attributes: VertexAttributeArray,
    instance_attributes: VertexAttributeArray,
}

pub type ShaderProgramPtr = Rc<ShaderProgram>;

/// Its attributes by id, each at its place in the program's list and of the
/// format a draw without data for it binds.
fn attribute_array(attributes: &[(usize, crate::gfx_types::AttributeDataType)]) -> VertexAttributeArray {
    let mut array = VertexAttributeArray::new();
    for (index, &(id, format)) in attributes.iter().enumerate() {
        array.set(id, index as i32, format, 0);
    }
    array
}

impl ShaderProgram {
    #[must_use]
    pub fn new(key: ShaderKey, info: &'static ProgramInfo) -> ShaderProgram {
        ShaderProgram {
            key,
            info,
            attributes: attribute_array(info.attributes),
            instance_attributes: attribute_array(info.instance_attributes),
        }
    }

    #[must_use]
    pub fn key(&self) -> &ShaderKey {
        &self.key
    }

    #[must_use]
    pub fn name(&self) -> &'static str {
        self.info.name
    }

    /// The per-vertex attributes, with the formats of their defaults.
    #[must_use]
    pub fn vertex_attributes(&self) -> &VertexAttributeArray {
        &self.attributes
    }

    /// The per-instance attributes, with the formats of their defaults.
    #[must_use]
    pub fn instance_attributes(&self) -> &VertexAttributeArray {
        &self.instance_attributes
    }

    /// The id of the attribute at `index` of the program's per-vertex or
    /// per-instance attributes.
    #[must_use]
    pub fn attribute_id(&self, step: VertexStep, index: usize) -> Option<usize> {
        let attributes = match step {
            VertexStep::Vertex => self.info.attributes,
            VertexStep::Instance => self.info.instance_attributes,
        };
        attributes.get(index).map(|&(id, _)| id)
    }

    /// Whether the program samples the texture with texture id `id`.
    #[must_use]
    pub fn samples(&self, id: usize) -> bool {
        self.info.textures.contains(&id)
    }

    /// The texture ids of the textures the program samples.
    #[must_use]
    pub fn textures(&self) -> &'static [usize] {
        self.info.textures
    }
}

/// Whether `a` and `b` are the same program handle, or both none.
#[must_use]
pub fn same_shader(a: &Option<ShaderProgramPtr>, b: &Option<ShaderProgramPtr>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Rc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}
