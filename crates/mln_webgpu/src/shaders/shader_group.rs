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

use crate::context::Context;
use crate::shaders::shader_program::ShaderProgram;
use crate::shaders::shader_source::{prelude, shader_source};
use crate::shaders::wgsl_preprocessor::preprocess_wgsl;

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
        let vert = source.wgsl.vertex.unwrap_or("");
        let frag = source.wgsl.fragment.unwrap_or("");

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

        // The common prelude goes through the preprocessor with the shader,
        // so its `#ifdef`s see the defines.
        let common_prelude = format!("{}\n", prelude());
        let (mut vertex_source, mut fragment_source) = match source.wgsl.prelude {
            Some(shader_prelude) => {
                (format!("{common_prelude}{shader_prelude}{vert}"), format!("{common_prelude}{shader_prelude}{frag}"))
            }
            None => (format!("{common_prelude}{vert}"), format!("{common_prelude}{frag}")),
        };

        let mut define_set: HashMap<String, bool> = HashMap::new();
        for key in self.program_parameters.get_defines().keys() {
            define_set.entry(key.clone()).or_insert(true);
        }
        for key in additional_defines.keys() {
            define_set.entry(key.clone()).or_insert(true);
        }
        vertex_source = preprocess_wgsl(&vertex_source, &define_set);
        fragment_source = preprocess_wgsl(&fragment_source, &define_set);

        let context = gfx_context.downcast_ref::<Context>().expect("webgpu::Context");
        let mut program =
            ShaderProgram::new(context, &vertex_source, &fragment_source, source.attributes, source.textures, name);
        for attribute in source.instance_attributes {
            program.init_instance_attribute(attribute);
        }
        let shader: ShaderProgramBasePtr = Rc::new(program);
        if !self.register_shader_named(shader.clone(), &shader_name) {
            panic!("Failed to register {shader_name} with shader group!");
        }
        Some(shader)
    }
}
