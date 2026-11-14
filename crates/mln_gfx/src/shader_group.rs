use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::gpu::ShaderKey;
use crate::projection_variant::ProjectionVariant;
use crate::shaders::program_info::ProgramInfo;
use crate::shaders::program_parameters::ProgramParameters;
use crate::shaders::shader_program::{ShaderProgram, ShaderProgramPtr};

/// The names (`a_color`) and attribute ids of the paint properties a layer
/// reads from uniforms rather than vertex attributes.
pub type StringIDSetsPair = (HashSet<String>, HashSet<usize>);

/// The define that compiles a shader's globe variant.
pub const GLOBE_DEFINE: &str = "PROJECTION_GLOBE";

const UNIFORM_PREFIX: &str = "HAS_UNIFORM_u_";

/// The variants of one built-in program: one for each set of paint
/// properties read from uniforms and each projection.
pub struct ShaderGroup {
    info: &'static ProgramInfo,
    parameters: ProgramParameters,
    programs: RefCell<HashMap<ShaderKey, ShaderProgramPtr>>,
}

pub type ShaderGroupPtr = Rc<ShaderGroup>;

impl ShaderGroup {
    #[must_use]
    pub fn new(info: &'static ProgramInfo, parameters: ProgramParameters) -> ShaderGroup {
        ShaderGroup { info, parameters, programs: RefCell::default() }
    }

    /// The variant that reads `properties_as_uniforms` from uniforms, for
    /// `variant`'s projection. Its key has the program parameters' defines
    /// (`DEVICE_PIXEL_RATIO=<ratio>`), a `HAS_UNIFORM_u_*` define for each
    /// property read from a uniform (`a_color` gives `HAS_UNIFORM_u_color`),
    /// and the globe's define for its variant.
    pub fn shader(&self, properties_as_uniforms: &StringIDSetsPair, variant: ProjectionVariant) -> ShaderProgramPtr {
        let parameters = self
            .parameters
            .get_defines()
            .iter()
            .map(|(name, value)| if value.is_empty() { name.clone() } else { format!("{name}={value}") });
        let uniforms = properties_as_uniforms
            .0
            .iter()
            .map(|name| format!("{UNIFORM_PREFIX}{}", name.strip_prefix("a_").unwrap_or(name)));
        let globe = (variant == ProjectionVariant::Globe).then(|| GLOBE_DEFINE.to_owned());
        let key = self.info.key(parameters.chain(uniforms).chain(globe));
        let mut programs = self.programs.borrow_mut();
        let program = programs.entry(key).or_insert_with_key(|key| Rc::new(ShaderProgram::new(key.clone(), self.info)));
        Rc::clone(program)
    }
}
