// One group type holds the `BuiltIn` program it compiles.

use std::collections::HashMap;
use std::rc::Rc;

use mln_gfx::context::Context as GfxContext;
use mln_gfx::gpu::{GpuError, ShaderKey};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shader_group::{self, GLOBE_DEFINE, ShaderGroupData, StringIDSetsPair};
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_gfx::shaders::shader_program_base::ShaderProgramBasePtr;
use mln_gfx::shaders::shader_source::BuiltIn;
use mln_util::logging::{Event, Log};

use crate::context::Context;
use crate::shaders::shader_program::ShaderProgram;
use crate::shaders::shader_source::{prelude, shader_source};
use crate::shaders::wgsl_preprocessor::preprocess_wgsl;

const UNIFORM_PREFIX: &str = "HAS_UNIFORM_u_";

/// Compiles the program of `key`: the common prelude, the program's own
/// prelude and each stage, preprocessed with the key's defines.
pub(crate) fn compile(
    device: &wgpu::Device,
    index_layout: &wgpu::BindGroupLayout,
    key: &ShaderKey,
) -> Result<ShaderProgram, GpuError> {
    let source = shader_source(key.program())
        .ok_or_else(|| GpuError::Unsupported(format!("no WGSL for {:?}", key.program())))?;
    // The common prelude goes through the preprocessor with the program, so
    // its `#ifdef`s see the defines.
    let shader_prelude = source.wgsl.prelude.unwrap_or("");
    let assemble = |stage: Option<&str>| format!("{}\n{shader_prelude}{}", prelude(), stage.unwrap_or(""));
    // WGSL reads no define's value.
    let defines: HashMap<String, bool> = key.define_values().map(|(name, _)| (name.to_owned(), true)).collect();
    let vertex_source = preprocess_wgsl(&assemble(source.wgsl.vertex), &defines);
    let fragment_source = preprocess_wgsl(&assemble(source.wgsl.fragment), &defines);

    let mut program = ShaderProgram::new(
        device,
        index_layout,
        &vertex_source,
        &fragment_source,
        source.attributes,
        source.textures,
        source.name,
    )?;
    for attribute in source.instance_attributes {
        program.init_instance_attribute(attribute);
    }
    Ok(program)
}

pub(crate) struct ShaderGroup {
    data: ShaderGroupData,
    shader_id: BuiltIn,
    program_parameters: ProgramParameters,
}

impl ShaderGroup {
    pub(crate) fn new(shader_id: BuiltIn, program_parameters: ProgramParameters) -> Self {
        ShaderGroup { data: ShaderGroupData::default(), shader_id, program_parameters }
    }

    /// The program's key: the program parameters' defines, a
    /// `HAS_UNIFORM_u_*` define for each property read from a uniform
    /// (`a_color` gives `HAS_UNIFORM_u_color`), and the globe's define for
    /// its variant.
    fn key(&self, properties_as_uniforms: &StringIDSetsPair, variant: ProjectionVariant) -> ShaderKey {
        let parameters = self.program_parameters.get_defines().keys().cloned();
        let uniforms = properties_as_uniforms
            .0
            .iter()
            .map(|name| format!("{UNIFORM_PREFIX}{}", name.strip_prefix("a_").unwrap_or(name)));
        let globe = (variant == ProjectionVariant::Globe).then(|| GLOBE_DEFINE.to_owned());
        ShaderKey::new(self.shader_id, parameters.chain(uniforms).chain(globe))
    }
}

impl shader_group::ShaderGroup for ShaderGroup {
    fn shader_group_data(&self) -> &ShaderGroupData {
        &self.data
    }

    fn get_or_create_shader(
        &self,
        gfx_context: &dyn GfxContext,
        properties_as_uniforms: &StringIDSetsPair,
        variant: ProjectionVariant,
        _first_attrib_name: &str,
    ) -> Option<ShaderProgramBasePtr> {
        let key = self.key(properties_as_uniforms, variant);
        let shader_name = format!("{:?}#{}", key.program(), key.defines().join(","));
        if let Some(shader) = self.get_typed(&shader_name, ShaderProgram::NAME) {
            return Some(shader);
        }

        let context = gfx_context.downcast_ref::<Context>()?;
        let device = context.get_backend().device();
        let program = compile(device, &context.index_bind_group_layout(), &key)
            .map_err(|error| Log::error(Event::Render, error.to_string()))
            .ok()?;
        let shader: ShaderProgramBasePtr = Rc::new(program);
        if !self.register_shader_named(Rc::clone(&shader), &shader_name) {
            Log::error(Event::Render, format!("WebGPU: another shader is registered as {shader_name}"));
        }
        Some(shader)
    }
}
