// The C++ `ShaderGroupGL<BuiltIn ShaderID>` template is one type here
// holding the `BuiltIn` value.

use std::rc::Rc;

use mln_gfx::context::Context as GfxContext;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shader_group::{self, GLOBE_DEFINE, ShaderGroupData, StringIDSetsPair, get_shader_name, property_hash};
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_gfx::shaders::shader_program_base::ShaderProgramBasePtr;
use mln_gfx::shaders::shader_source::BuiltIn;
use mln_gfx::util_hash::hash_combine;
use mln_util::logging::{Event, Log};

use super::shader_source::shader_source;
use crate::context::Context;
use crate::shaders::shader_program::ShaderProgram;

pub struct ShaderGroup {
    data: ShaderGroupData,
    shader_id: BuiltIn,
    program_parameters: ProgramParameters,
}

impl ShaderGroup {
    pub fn new(shader_id: BuiltIn, program_parameters: ProgramParameters) -> Self {
        ShaderGroup { data: ShaderGroupData::default(), shader_id, program_parameters }
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
        first_attrib_name: &str,
    ) -> Option<ShaderProgramBasePtr> {
        let source = shader_source(self.shader_id)?;
        let name = source.name;

        let mut seed = 0u64;
        hash_combine(&mut seed, property_hash(properties_as_uniforms));
        hash_combine(&mut seed, self.program_parameters.get_defines_hash());
        hash_combine(&mut seed, variant as u64);
        let shader_name = get_shader_name(name, seed);

        if let Some(shader) = self.get_typed(&shader_name, ShaderProgram::NAME) {
            return Some(shader);
        }

        // No match, we need to create the shader.
        let mut property_names: Vec<&str> = properties_as_uniforms.0.iter().map(String::as_str).collect();
        property_names.sort_unstable();
        let mut additional_defines = String::with_capacity(property_names.len() * 48);
        for property_name in property_names {
            // We expect the names to be prefixed by "a_", but we need just
            // the base here.
            let base = property_name.strip_prefix("a_").unwrap_or(property_name);
            additional_defines.push_str("#define HAS_UNIFORM_u_");
            additional_defines.push_str(base);
            additional_defines.push('\n');
        }
        if variant == ProjectionVariant::Globe {
            additional_defines.push_str("#define ");
            additional_defines.push_str(GLOBE_DEFINE);
            additional_defines.push('\n');
        }

        let context = gfx_context.downcast_ref::<Context>()?;
        let shader: ShaderProgramBasePtr = Rc::new(ShaderProgram::create(
            context,
            &shader_name,
            &self.program_parameters,
            first_attrib_name,
            source,
            &additional_defines,
        )?);
        if !self.register_shader_named(shader.clone(), &shader_name) {
            Log::error(Event::Shader, format!("Failed to register {shader_name} with shader group!"));
            debug_assert!(false);
            return None;
        }
        Some(shader)
    }
}
