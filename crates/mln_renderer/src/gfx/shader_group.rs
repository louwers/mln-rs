use std::cell::RefCell;
use std::rc::Rc;

use mln_gfx::gpu::ShaderKey;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::program_info::ProgramInfo;
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_util::hash::{HashMap, HashSet};

use crate::gfx::shader_program::{ShaderProgram, ShaderProgramPtr};

/// The names (`a_color`) and attribute ids of the paint properties a layer
/// reads from uniforms rather than vertex attributes.
pub(crate) type StringIDSetsPair = (HashSet<String>, HashSet<usize>);

/// The variants of one built-in program: one for each set of paint
/// properties read from uniforms and each projection.
pub(crate) struct ShaderGroup {
    info: &'static ProgramInfo,
    parameters: ProgramParameters,
    programs: RefCell<HashMap<ShaderKey, ShaderProgramPtr>>,
}

pub(crate) type ShaderGroupPtr = Rc<ShaderGroup>;

impl ShaderGroup {
    #[must_use]
    pub(crate) fn new(info: &'static ProgramInfo, parameters: ProgramParameters) -> ShaderGroup {
        ShaderGroup { info, parameters, programs: RefCell::default() }
    }

    /// The variant that reads `properties_as_uniforms` from uniforms, for
    /// `variant`'s projection, of the key
    /// [`ProgramInfo::variant_key`] gives it.
    pub(crate) fn shader(
        &self,
        properties_as_uniforms: &StringIDSetsPair,
        variant: ProjectionVariant,
    ) -> ShaderProgramPtr {
        let uniforms = properties_as_uniforms.0.iter().map(String::as_str);
        let key = self.info.variant_key(&self.parameters, uniforms, variant);
        let mut programs = self.programs.borrow_mut();
        let program = programs.entry(key).or_insert_with_key(|key| Rc::new(ShaderProgram::new(key.clone(), self.info)));
        Rc::clone(program)
    }
}
