use std::rc::Rc;

use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::gpu::{ShaderKey, VertexStep};
use mln_gfx::shaders::program_info::ProgramInfo;

/// A built-in program with its defines: the key a pipeline compiles it from,
/// and what it reads.
pub(crate) struct ShaderProgram {
    key: ShaderKey,
    info: &'static ProgramInfo,
}

pub(crate) type ShaderProgramPtr = Rc<ShaderProgram>;

impl ShaderProgram {
    #[must_use]
    pub(crate) fn new(key: ShaderKey, info: &'static ProgramInfo) -> ShaderProgram {
        ShaderProgram { key, info }
    }

    #[must_use]
    pub(crate) fn key(&self) -> &ShaderKey {
        &self.key
    }

    #[must_use]
    pub(crate) fn name(&self) -> &'static str {
        self.info.name
    }

    /// The per-vertex attributes, in the program's order, each its id and
    /// the format a draw without data for it binds.
    #[must_use]
    pub(crate) fn vertex_attributes(&self) -> &'static [(usize, AttributeDataType)] {
        self.info.attributes
    }

    /// The per-instance attributes, as
    /// [`vertex_attributes`](Self::vertex_attributes) gives them.
    #[must_use]
    pub(crate) fn instance_attributes(&self) -> &'static [(usize, AttributeDataType)] {
        self.info.instance_attributes
    }

    /// The id of the attribute at `index` of the program's per-vertex or
    /// per-instance attributes.
    #[must_use]
    pub(crate) fn attribute_id(&self, step: VertexStep, index: usize) -> Option<usize> {
        let attributes = match step {
            VertexStep::Vertex => self.info.attributes,
            VertexStep::Instance => self.info.instance_attributes,
        };
        attributes.get(index).map(|&(id, _)| id)
    }

    /// Whether the program samples the texture with texture id `id`.
    #[must_use]
    pub(crate) fn samples(&self, id: usize) -> bool {
        self.info.textures.contains(&id)
    }

    /// The texture ids of the textures the program samples.
    #[must_use]
    pub(crate) fn textures(&self) -> &'static [usize] {
        self.info.textures
    }
}
