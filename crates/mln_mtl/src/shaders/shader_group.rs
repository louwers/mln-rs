// The C++ `ShaderGroup<BuiltIn ShaderID>` template is one type here holding
// the `BuiltIn` value.

use std::collections::HashMap;
use std::rc::Rc;

use mln_gfx::context::Context as GfxContext;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shader_group::{self, GLOBE_DEFINE, ShaderGroupData, StringIDSetsPair, get_shader_name, property_hash};
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_gfx::shaders::shader_program_base::ShaderProgramBasePtr;
use mln_gfx::shaders::shader_source::BuiltIn;
use mln_gfx::util_hash::hash_combine;
use mln_util::logging::{Event, Log};

use super::msl_sources::PRELUDE;
use super::shader_source::shader_source;
use crate::context::Context;
use crate::shaders::shader_program::ShaderProgram;

const UNIFORM_PREFIX: &str = "HAS_UNIFORM_u_";

pub struct ShaderGroup {
    data: ShaderGroupData,
    shader_id: BuiltIn,
    program_parameters: ProgramParameters,
}

impl ShaderGroup {
    pub fn new(shader_id: BuiltIn, program_parameters: ProgramParameters) -> Self {
        ShaderGroup { data: ShaderGroupData::default(), shader_id, program_parameters }
    }

    fn add_additional_defines(
        properties_as_uniforms: &StringIDSetsPair,
        additional_defines: &mut HashMap<String, String>,
    ) {
        additional_defines.reserve(properties_as_uniforms.0.len());
        for name in &properties_as_uniforms.0 {
            // We expect the names to be prefixed by "a_", but we need just the base here.
            let base = name.strip_prefix("a_").unwrap_or(name);
            additional_defines.insert(format!("{UNIFORM_PREFIX}{base}"), String::new());
        }
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

        let mut additional_defines = HashMap::new();
        Self::add_additional_defines(properties_as_uniforms, &mut additional_defines);
        if variant == ProjectionVariant::Globe {
            additional_defines.insert(GLOBE_DEFINE.to_owned(), String::new());
        }

        let context = gfx_context.downcast_ref::<Context>()?;
        let shader_source = format!("{PRELUDE}{}{}", source.msl.prelude, source.msl.source);
        let mut shader = context.create_program(
            shader_name.clone(),
            &shader_source,
            source.msl.vertex_main_function,
            source.msl.fragment_main_function,
            &self.program_parameters,
            &additional_defines,
        )?;
        for attribute in source.attributes {
            if !properties_as_uniforms.1.contains(&attribute.id) {
                shader.init_vertex_attribute(attribute);
            }
        }
        for attribute in source.instance_attributes {
            if !properties_as_uniforms.1.contains(&attribute.id) {
                shader.init_instance_attribute(attribute);
            }
        }
        for texture in source.textures {
            shader.init_texture(texture);
        }

        let shader: ShaderProgramBasePtr = Rc::new(shader);
        if !self.register_shader_named(shader.clone(), &shader_name) {
            Log::error(Event::Shader, format!("Failed to register {shader_name} with shader group!"));
            debug_assert!(false);
            return None;
        }
        Some(shader)
    }
}
