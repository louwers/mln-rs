//! What a program reads, the same on every backend. A backend compiles a
//! program from the source registered for it.

use std::hash::{Hash, Hasher};
use std::{fmt, ptr};

use mln_util::hash::HashMap;

use crate::gfx_types::AttributeDataType;
use crate::gpu::{GpuError, ShaderKey};
use crate::projection_variant::ProjectionVariant;
use crate::shaders::program_parameters::ProgramParameters;

/// A program: the attributes, textures and paint properties its shader
/// reads.
///
/// A program is the `static` that declares it: two programs are equal only
/// if they are the same `static`, and a backend finds the source it compiles
/// a program from by it.
pub struct Program {
    /// Its name, for logs and labels.
    pub name: &'static str,
    /// Each per-vertex attribute's id and the format a draw without data
    /// for it binds, in the program's order.
    pub attributes: &'static [(usize, AttributeDataType)],
    /// Each per-instance attribute, as `attributes` gives them.
    pub instance_attributes: &'static [(usize, AttributeDataType)],
    /// The ids of the textures it samples.
    pub textures: &'static [usize],
    /// The paint properties it reads: those whose `HAS_UNIFORM_u_<name>`
    /// its source tests, and no others, since a variant defines only these.
    pub properties: &'static [Property],
}

/// A paint property a program reads: from its uniform in a variant that
/// reads it from there, otherwise from the values a layer binds to its
/// attribute.
#[derive(Clone, Copy, Debug)]
pub struct Property {
    /// The name of the uniform, `u_<name>`.
    pub name: &'static str,
    /// The id of the attribute a layer binds the property's values to.
    pub attribute: usize,
}

/// The define that compiles a program's globe variant.
pub const GLOBE_DEFINE: &str = "PROJECTION_GLOBE";

impl Program {
    /// The key of the variant compiled with `parameters` that reads
    /// `uniforms`, properties of the program, from their uniforms, for
    /// `variant`'s projection. It has the parameters' defines
    /// (`DEVICE_PIXEL_RATIO=<ratio>`), a `HAS_UNIFORM_u_<name>` define for
    /// each of `uniforms`, and the globe's define for its variant.
    #[must_use]
    pub fn variant_key<'a>(
        &'static self,
        parameters: &ProgramParameters,
        uniforms: impl IntoIterator<Item = &'a Property>,
        variant: ProjectionVariant,
    ) -> ShaderKey {
        let parameters = parameters
            .defines()
            .iter()
            .map(|(name, value)| if value.is_empty() { name.clone() } else { format!("{name}={value}") });
        let uniforms = uniforms.into_iter().map(|property| format!("HAS_UNIFORM_u_{}", property.name));
        let globe = (variant == ProjectionVariant::Globe).then(|| GLOBE_DEFINE.to_owned());
        ShaderKey::new(self, parameters.chain(uniforms).chain(globe))
    }
}

impl PartialEq for Program {
    fn eq(&self, other: &Program) -> bool {
        ptr::eq(self, other)
    }
}

impl Eq for Program {}

impl Hash for Program {
    fn hash<H: Hasher>(&self, state: &mut H) {
        ptr::hash(self, state);
    }
}

impl fmt::Debug for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name)
    }
}

/// The source of type `S` that a backend compiles each program from.
pub struct Sources<S: 'static> {
    sources: HashMap<&'static Program, &'static S>,
}

impl<S> Sources<S> {
    /// The sources with each of `built_ins` registered.
    #[must_use]
    pub fn new(built_ins: &[(&'static Program, &'static S)]) -> Sources<S> {
        let mut sources = Sources { sources: HashMap::default() };
        for &(program, source) in built_ins {
            sources.register(program, source);
        }
        sources
    }

    /// Registers `source` as `program`'s, in place of any before.
    pub fn register(&mut self, program: &'static Program, source: &'static S) {
        self.sources.insert(program, source);
    }

    /// The source of `program`. Fails with [`GpuError::Unsupported`] if it
    /// has none.
    pub fn get(&self, program: &'static Program) -> Result<&'static S, GpuError> {
        self.sources
            .get(program)
            .copied()
            .ok_or_else(|| GpuError::Unsupported(format!("no source for {}", program.name)))
    }
}
