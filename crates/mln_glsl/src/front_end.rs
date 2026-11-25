// naga's GLSL front end takes GLSL 4.50 for Vulkan, less what the
// programs' GLSL avoids: combined image samplers, `bool` members of buffers, precision
// qualifiers and samplers as function parameters.

use naga::valid::{Capabilities, ModuleInfo, ValidationFlags, Validator};

use crate::prelude::{PRELUDE_FRAGMENT, PRELUDE_VERTEX};

/// The GLSL version every source is compiled as.
const GLSL_VERSION: u32 = 450;

/// The stages of a program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Vertex,
    Fragment,
}

impl Stage {
    #[must_use]
    pub fn naga(self) -> naga::ShaderStage {
        match self {
            Stage::Vertex => naga::ShaderStage::Vertex,
            Stage::Fragment => naga::ShaderStage::Fragment,
        }
    }

    /// The prelude of every shader of the stage.
    fn prelude(self) -> &'static str {
        match self {
            Stage::Vertex => PRELUDE_VERTEX,
            Stage::Fragment => PRELUDE_FRAGMENT,
        }
    }
}

/// The source of a stage: the version, the program's defines (`#define`
/// lines), the engine's declarations, the stage's prelude, which reads them,
/// and the program's declarations and source.
pub(crate) fn compose(stage: Stage, defines: &str, engine: &str, declarations: &str, source: &str) -> String {
    format!("#version {GLSL_VERSION}\n{defines}\n{engine}{}\n{declarations}{source}", stage.prelude())
}

/// A resource that a stage's entry point uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceUse {
    pub set: u32,
    pub binding: u32,
    /// The size of a uniform or storage buffer's block: of its first element,
    /// for a block that ends in an array of any length. 0 for a texture or
    /// sampler.
    pub size: u64,
}

/// A stage that naga parsed and validated, with the entry point `main`.
#[derive(Debug)]
pub struct Module {
    pub module: naga::Module,
    pub info: ModuleInfo,
}

impl Module {
    /// Parses and validates GLSL, such as a [`compose`](crate::compose)d stage.
    pub fn parse(stage: Stage, source: &str) -> Result<Module, String> {
        let module = parse(stage, source)?;
        // Not the uniformity of control flow: it is WGSL's rule for
        // derivatives, which GLSL doesn't have.
        let flags = ValidationFlags::all() & !ValidationFlags::CONTROL_FLOW_UNIFORMITY;
        let info =
            Validator::new(flags, Capabilities::all()).validate(&module).map_err(|error| format!("{error:?}"))?;
        Ok(Module { module, info })
    }

    /// The resources the entry point uses.
    pub fn resources(&self) -> Result<Vec<ResourceUse>, String> {
        let uses = self.info.get_entry_point(0);
        let mut layouter = naga::proc::Layouter::default();
        layouter.update(self.module.to_ctx()).map_err(|error| format!("{error:?}"))?;
        Ok(self
            .module
            .global_variables
            .iter()
            .filter(|&(handle, _)| !uses[handle].is_empty())
            .filter_map(|(_, global)| {
                let size = match global.space {
                    naga::AddressSpace::Uniform | naga::AddressSpace::Storage { .. } => {
                        u64::from(layouter[global.ty].size)
                    }
                    _ => 0,
                };
                global.binding.as_ref().map(|binding| ResourceUse {
                    set: binding.group,
                    binding: binding.binding,
                    size,
                })
            })
            .collect())
    }
}

/// Parses a source into a naga module, unvalidated.
pub(crate) fn parse(stage: Stage, source: &str) -> Result<naga::Module, String> {
    let mut frontend = naga::front::glsl::Frontend::default();
    frontend
        .parse(&naga::front::glsl::Options::from(stage.naga()), source)
        .map_err(|errors| errors.emit_to_string(source))
}
