use std::collections::HashMap;
use std::rc::Rc;

use crate::projection_variant::ProjectionVariant;
use crate::shader_group::{ShaderGroup, ShaderGroupPtr, StringIDSetsPair};
use crate::shaders::program_info::{PROGRAMS, program_info};
use crate::shaders::program_parameters::ProgramParameters;
use crate::shaders::shader_program::ShaderProgramPtr;

/// The shader group of each built-in program, by the program's name.
pub struct ShaderRegistry {
    shader_groups: HashMap<&'static str, ShaderGroupPtr>,
}

impl ShaderRegistry {
    /// The groups of the built-in programs, compiled with `parameters`.
    #[must_use]
    pub fn new(parameters: &ProgramParameters) -> Self {
        let shader_groups = PROGRAMS
            .iter()
            .filter_map(|&program| program_info(program))
            .map(|info| (info.name, Rc::new(ShaderGroup::new(info, parameters.clone()))))
            .collect();
        ShaderRegistry { shader_groups }
    }

    /// The group of the program named `name`, such as `FillShader`.
    #[must_use]
    pub fn shader_group(&self, name: &str) -> Option<ShaderGroupPtr> {
        self.shader_groups.get(name).cloned()
    }

    /// The program named `name` with no paint properties read from uniforms,
    /// for `variant`'s projection.
    #[must_use]
    pub fn shader(&self, name: &str, variant: ProjectionVariant) -> Option<ShaderProgramPtr> {
        Some(self.shader_groups.get(name)?.shader(&StringIDSetsPair::default(), variant))
    }
}
