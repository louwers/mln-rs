// Ports include/mln/shaders/gl/shader_program_gl.hpp and
// src/mln/shaders/gl/shader_program_gl.cpp
//
// Differences from C++:
// - The program keeps the uniform blocks it has, with the binding point and
//   the sizes of each, for `Context::bind_uniform_blocks`.
// - A sampler reads the texture unit of its texture's id, set once when the
//   program is made, so `get_sampler_location` returns that unit and a draw
//   sets no sampler uniform.
// - An attribute is matched to its table entry by name (C++: by location,
//   which the port's added attribute doesn't follow).

use std::rc::{Rc, Weak};

use glow::HasContext;
use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::shader::Shader;
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_gfx::shaders::shader_defines::MAX_TEXTURE_COUNT_PER_SHADER;
use mln_gfx::shaders::shader_program_base::ShaderProgramBase;
use mln_gfx::vertex_attribute::VertexAttributeArray;
use mln_util::identity::SimpleIdentity;
use mln_util::logging::{Event, Log};

use super::glsl_sources::PRELUDE;
use super::shader_source::ShaderSource;
use crate::context::Context;
use crate::types::ProgramID;

/// A uniform block of a program.
#[derive(Clone, Copy, Debug)]
pub struct UniformBlock {
    /// The block's binding point: the id of its uniform buffer.
    pub id: usize,
    /// `GL_UNIFORM_BLOCK_DATA_SIZE`
    pub size: usize,
    /// `UniformBlockInfo::element_size`
    pub element_size: usize,
}

/// `gl::ShaderProgramGL`
pub struct ShaderProgram {
    shader_program_id: SimpleIdentity,
    context: Weak<Context>,
    program: ProgramID,
    vertex_attributes: VertexAttributeArray,
    instance_attributes: VertexAttributeArray,
    /// The texture unit each texture id's sampler reads.
    sampler_units: [Option<usize>; MAX_TEXTURE_COUNT_PER_SHADER],
    uniform_blocks: Vec<UniformBlock>,
}

/// `mapType(GLenum attrType)`
fn map_type(attribute_type: u32) -> AttributeDataType {
    use AttributeDataType as T;
    match attribute_type {
        glow::FLOAT => T::Float,
        glow::FLOAT_VEC2 => T::Float2,
        glow::FLOAT_VEC3 => T::Float3,
        glow::FLOAT_VEC4 | glow::FLOAT_MAT2 | glow::FLOAT_MAT3 | glow::FLOAT_MAT4 => T::Float4,
        glow::INT => T::Int,
        glow::INT_VEC2 => T::Int2,
        glow::INT_VEC3 => T::Int3,
        glow::INT_VEC4 => T::Int4,
        glow::UNSIGNED_INT => T::UInt,
        glow::UNSIGNED_INT_VEC2 => T::UInt2,
        glow::UNSIGNED_INT_VEC3 => T::UInt3,
        glow::UNSIGNED_INT_VEC4 => T::UInt4,
        _ => T::Invalid,
    }
}

impl ShaderProgram {
    pub const NAME: &'static str = "GenericGLShader";

    /// `ShaderProgramGL::create(context, programParameters, firstAttribName,
    /// uniformBlocksInfo, texturesInfo, attributesInfo, vertexSource,
    /// fragmentSource, additionalDefines)`: compiles and links `source` with
    /// the program's defines and `additional_defines`, or logs why it
    /// couldn't (C++ throws).
    pub fn create(
        context: &Context,
        name: &str,
        program_parameters: &ProgramParameters,
        first_attrib_name: &str,
        source: &ShaderSource,
        additional_defines: &str,
    ) -> Option<ShaderProgram> {
        // Desktop OpenGL compiles GLSL ES 3.00 only with
        // `GL_ARB_ES3_compatibility`, and every 3.3 context the GLSL 3.30
        // that the shaders are as well: what they use of GLSL ES is in it,
        // and it ignores their precision qualifiers.
        let version = if context.is_desktop() { "#version 330 core\n" } else { "#version 300 es\n" };

        let failed = |message: String| {
            Log::error(Event::Shader, message);
            None
        };

        let defines = program_parameters.get_defines_string();
        let vertex_shader = match context.create_shader(
            glow::VERTEX_SHADER,
            &[version, defines, additional_defines, PRELUDE.vertex, source.glsl.vertex],
        ) {
            Ok(shader) => shader,
            Err(log) => return failed(format!("{name}: vertex shader failed to compile: {log}")),
        };
        let fragment_shader = match context.create_shader(
            glow::FRAGMENT_SHADER,
            &[version, defines, additional_defines, PRELUDE.fragment, source.glsl.fragment],
        ) {
            Ok(shader) => shader,
            Err(log) => {
                // SAFETY: the vertex shader is alive and no program has it.
                unsafe { context.gl().delete_shader(vertex_shader) };
                return failed(format!("{name}: fragment shader failed to compile: {log}"));
            }
        };
        let program = match context.create_program(vertex_shader, fragment_shader, first_attrib_name) {
            Ok(program) => program,
            Err(log) => return failed(format!("{name}: program failed to link: {log}")),
        };

        let gl = context.gl();
        let mut uniform_blocks = Vec::with_capacity(source.uniform_blocks.len());
        let mut sampler_units = [None; MAX_TEXTURE_COUNT_PER_SHADER];
        let mut vertex_attributes = VertexAttributeArray::new();
        let mut instance_attributes = VertexAttributeArray::new();
        // SAFETY: the program is alive and linked, and the indexes and
        // locations asked about are the ones it reports.
        unsafe {
            let block_count = gl.get_program_parameter_i32(program, glow::ACTIVE_UNIFORM_BLOCKS).max(0) as u32;
            for index in 0..block_count {
                let block_name = gl.get_active_uniform_block_name(program, index);
                // A block missing from the program is in the table for the
                // variants that read it.
                let Some(info) = source.uniform_blocks.iter().find(|info| info.name == block_name) else {
                    Log::error(Event::Shader, format!("{name}: uniform block {block_name} has no id"));
                    debug_assert!(false, "uniform block without a table entry");
                    continue;
                };
                let size = gl.get_active_uniform_block_parameter_i32(program, index, glow::UNIFORM_BLOCK_DATA_SIZE);
                debug_assert!(size > 0);
                gl.uniform_block_binding(program, index, info.id as u32);
                uniform_blocks.push(UniformBlock {
                    id: info.id,
                    size: size.max(0) as usize,
                    element_size: info.element_size,
                });
            }

            context.state().use_program(gl, Some(program));
            for info in source.textures {
                let location = gl.get_uniform_location(program, info.name);
                debug_assert!(location.is_some(), "sampler {} missing from {name}", info.name);
                if location.is_some()
                    && let Some(unit) = sampler_units.get_mut(info.id)
                {
                    gl.uniform_1_i32(location.as_ref(), info.id as i32);
                    *unit = Some(info.id);
                }
            }

            for index in 0..gl.get_active_attributes(program) {
                let Some(attribute) = gl.get_active_attribute(program, index) else {
                    continue;
                };
                // Built-in attributes, such as gl_VertexID.
                if attribute.name.starts_with("gl_") {
                    continue;
                }
                let Some(location) = gl.get_attrib_location(program, &attribute.name) else {
                    continue;
                };
                let named = |info: &&super::shader_source::AttributeInfo| info.name == attribute.name;
                let (attributes, info) = if let Some(info) = source.attributes.iter().find(named) {
                    (&mut vertex_attributes, info)
                } else if let Some(info) = source.instance_attributes.iter().find(named) {
                    (&mut instance_attributes, info)
                } else {
                    debug_assert!(false, "attribute {} of {name} has no id", attribute.name);
                    continue;
                };
                let element_type = map_type(attribute.atype);
                if element_type != AttributeDataType::Invalid {
                    attributes.set(info.id, location as i32, element_type, attribute.size.max(0) as usize);
                }
            }
        }

        Some(ShaderProgram {
            shader_program_id: SimpleIdentity::new(),
            context: context.weak(),
            program,
            vertex_attributes,
            instance_attributes,
            sampler_units,
            uniform_blocks,
        })
    }

    /// `getGLProgramID()`
    #[must_use]
    pub fn get_gl_program_id(&self) -> ProgramID {
        self.program
    }

    /// The uniform blocks this variant of the program reads.
    #[must_use]
    pub fn uniform_blocks(&self) -> &[UniformBlock] {
        &self.uniform_blocks
    }
}

impl Drop for ShaderProgram {
    fn drop(&mut self) {
        if let Some(context) = self.context.upgrade() {
            context.abandon_program(self.program);
        }
    }
}

impl Shader for ShaderProgram {
    fn type_name(&self) -> &'static str {
        Self::NAME
    }
}

impl ShaderProgramBase for ShaderProgram {
    fn get_id(&self) -> SimpleIdentity {
        self.shader_program_id
    }
    fn get_sampler_location(&self, id: usize) -> Option<usize> {
        self.sampler_units.get(id).copied().flatten()
    }
    fn get_vertex_attributes(&self) -> &VertexAttributeArray {
        &self.vertex_attributes
    }
    fn get_instance_attributes(&self) -> &VertexAttributeArray {
        &self.instance_attributes
    }
}

/// Downcast a shader handle to the GL program (C++ `static_cast`).
pub fn as_gl_shader(shader: &Rc<dyn ShaderProgramBase>) -> Option<&ShaderProgram> {
    shader.downcast_ref::<ShaderProgram>()
}
