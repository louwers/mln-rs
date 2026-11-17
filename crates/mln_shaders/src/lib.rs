//! The renderer's programs, each written once in WESL (`shaders/`), linked
//! into WGSL for a backend and a variant, and checked by naga, whose back
//! ends write each backend's shading language.
//!
//! A program is `shaders/programs/<BuiltIn>.wesl`, named after its
//! [`BuiltIn`]; the other modules are shared. Its entry points are `vs_main`
//! and `fs_main` ([`Stage::entry_point`]). A backend gets a program, linked
//! for it, from [`program_of`].
//!
//! # Feature flags
//!
//! A variant is linked with the feature flags of its `ShaderKey`'s defines
//! that have no value (`HAS_UNIFORM_u_<name>`, `PROJECTION_GLOBE`,
//! `OVERDRAW_INSPECTOR`) and the target's own: `vulkan`, `d3d12`, `gl`,
//! `metal` or `webgpu` ([`Target`]). A define with a value, such as
//! `DEVICE_PIXEL_RATIO`, is no flag: a program reads the value from a
//! uniform.
//!
//! # Resources
//!
//! A program declares each resource with `@group` the descriptor set it is
//! in ([`layout::DescriptorSetType`]: 0 global, 1 the layer's, 2 the
//! drawable's uniform buffers, 3 the drawable's textures and samplers) and
//! `@binding` the renderer's id of it (`mln_gfx::shaders::shader_defines`):
//! a buffer's id, a texture's id, or for a sampler,
//! [`layout::IMAGE_SAMPLER_START`] plus its texture's id. A layer's buffer of
//! an element for each drawable is `var<storage, read>` of an array, which a
//! draw indexes with `globals::drawable_index()`. Each target links the
//! declarations rewritten into its own convention:
//!
//! - Vulkan and D3D12: the set and binding of [`layout`], each texture at the
//!   index of its id; D3D12 then puts a set's bindings in the registers of
//!   its space (`hlsl`). The draw's index is an immediate.
//! - OpenGL: the declarations as written, by which `glsl` reports each
//!   uniform block's buffer id and each sampler's texture id. A storage
//!   buffer is a uniform block of one element, the draw's, and the draw's
//!   index is 0.
//! - Metal: the declarations as written; `msl` puts each buffer in the
//!   buffer slot of its id and each texture and sampler in the slots of the
//!   texture's id. The vertex buffers bind after the program's buffers
//!   (`Program::first_vertex_buffer`).
//! - WebGPU: every buffer at its id in group 0, the textures and samplers at
//!   their bindings in the next group, and the draw's index
//!   (`ID_GLOBAL_UBO_INDEX`) alone in the last ([`Program::index_group`]),
//!   skipping the groups a program doesn't use.
//!
//! A vertex input's `@location` is its attribute's id. An integer input
//! reads integers on every backend.
//!
//! The crate's README says how to add a program.

pub mod layout;
mod link;
mod variants;

#[cfg(any(test, feature = "glsl"))]
pub mod glsl;
#[cfg(any(test, feature = "hlsl"))]
pub mod hlsl;
#[cfg(any(test, feature = "msl"))]
pub mod msl;
#[cfg(any(test, feature = "spirv"))]
pub mod spirv;

#[cfg(test)]
mod tests;

use std::sync::{Arc, LazyLock};

use mln_gfx::gpu::ShaderKey;
use mln_gfx::shaders::shader_source::BuiltIn;
use mln_util::hash::HashMap;
pub use naga;
use naga::valid::{Capabilities, ModuleInfo, ValidationFlags, Validator};
use parking_lot::Mutex;

pub use crate::layout::DescriptorSetType;
use crate::link::{Declared, DeclaredKind};
pub use crate::variants::{MAX_EXHAUSTIVE_FLAGS, flags_of, variants};

include!(concat!(env!("OUT_DIR"), "/registry.rs"));

/// The backend a program is linked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    Vulkan,
    D3d12,
    Gl,
    Metal,
    WebGpu,
}

impl Target {
    pub const ALL: [Target; 5] = [Target::Vulkan, Target::D3d12, Target::Gl, Target::Metal, Target::WebGpu];

    /// The feature flag the target links with.
    #[must_use]
    pub fn feature(self) -> &'static str {
        match self {
            Target::Vulkan => "vulkan",
            Target::D3d12 => "d3d12",
            Target::Gl => "gl",
            Target::Metal => "metal",
            Target::WebGpu => "webgpu",
        }
    }
}

/// A stage of a program.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stage {
    Vertex,
    Fragment,
}

impl Stage {
    pub const ALL: [Stage; 2] = [Stage::Vertex, Stage::Fragment];

    #[must_use]
    pub fn naga(self) -> naga::ShaderStage {
        match self {
            Stage::Vertex => naga::ShaderStage::Vertex,
            Stage::Fragment => naga::ShaderStage::Fragment,
        }
    }

    /// The name of the stage's entry point in every program.
    #[must_use]
    pub fn entry_point(self) -> &'static str {
        match self {
            Stage::Vertex => "vs_main",
            Stage::Fragment => "fs_main",
        }
    }
}

/// What a resource is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceKind {
    UniformBuffer,
    /// A layer's buffer with an element for each drawable: a read-only
    /// storage buffer, or on OpenGL a uniform block of the draw's element.
    StorageBuffer,
    /// A 2D float texture; not `filterable` if the program only loads its
    /// texels, never samples it.
    Texture {
        filterable: bool,
    },
    Sampler,
}

/// A resource of a program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resource {
    /// The global variable's name in the module.
    pub name: String,
    pub handle: naga::Handle<naga::GlobalVariable>,
    pub kind: ResourceKind,
    /// The descriptor set its WESL declares it in.
    pub set: DescriptorSetType,
    /// The renderer's id of the buffer or texture; of a sampler's texture.
    pub id: u32,
    /// Its group and binding in the target's module.
    pub binding: naga::ResourceBinding,
    /// The bytes of a buffer: of a [`ResourceKind::StorageBuffer`], of one
    /// element, the stride of the layer's buffer. 0 for a texture or
    /// sampler.
    pub size: u64,
    /// Whether the vertex stage uses it.
    pub vertex: bool,
    /// Whether the fragment stage uses it.
    pub fragment: bool,
}

impl Resource {
    /// Whether `stage` uses it.
    #[must_use]
    pub fn used_by(&self, stage: Stage) -> bool {
        match stage {
            Stage::Vertex => self.vertex,
            Stage::Fragment => self.fragment,
        }
    }
}

/// A vertex input of a program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VertexInput {
    /// The input's location: its attribute's id.
    pub location: u32,
    /// The kind of its scalars: an integer input reads integers.
    pub scalar: naga::ScalarKind,
    /// The number of its components, 1 to 4.
    pub components: u32,
}

impl VertexInput {
    /// Whether the input reads integers, unconverted.
    #[must_use]
    pub fn is_integer(&self) -> bool {
        matches!(self.scalar, naga::ScalarKind::Sint | naga::ScalarKind::Uint)
    }
}

/// A program linked for a target and a variant, and validated.
#[derive(Debug)]
pub struct Program {
    /// The program's name, its `BuiltIn`'s.
    pub name: &'static str,
    pub target: Target,
    pub module: naga::Module,
    pub info: ModuleInfo,
    /// The WGSL that WESL linked, which `module` is parsed from.
    pub wgsl: String,
    /// The resources either stage uses.
    pub resources: Vec<Resource>,
    pub inputs: Vec<VertexInput>,
    /// For WebGPU, the group of the draw's index (`ID_GLOBAL_UBO_INDEX`), if
    /// the program reads it.
    pub index_group: Option<u32>,
    /// The index in the module of each stage's entry point.
    entry_points: [usize; 2],
    /// What the writers of the backends' languages wrote, once each.
    #[cfg_attr(
        not(any(test, feature = "glsl", feature = "hlsl", feature = "msl", feature = "spirv")),
        expect(dead_code, reason = "only the writers read it")
    )]
    written: Written,
}

/// The outputs of the writers, by stage where they write one.
#[derive(Debug, Default)]
struct Written {
    #[cfg(any(test, feature = "spirv"))]
    spirv: [std::sync::OnceLock<Result<Vec<u32>, String>>; 2],
    /// By stage, and by whether for desktop GLSL.
    #[cfg(any(test, feature = "glsl"))]
    glsl: [[std::sync::OnceLock<Result<glsl::GlslStage, String>>; 2]; 2],
    #[cfg(any(test, feature = "msl"))]
    msl: std::sync::OnceLock<Result<msl::Msl, String>>,
    #[cfg(any(test, feature = "hlsl"))]
    hlsl: [std::sync::OnceLock<Result<hlsl::Hlsl, String>>; 2],
}

impl Program {
    /// The index of `stage`'s entry point in the module.
    #[must_use]
    pub fn entry_point_index(&self, stage: Stage) -> usize {
        self.entry_points[stage as usize]
    }

    /// The resource of the global `handle`, if it is one.
    #[must_use]
    pub fn resource(&self, handle: naga::Handle<naga::GlobalVariable>) -> Option<&Resource> {
        self.resources.iter().find(|resource| resource.handle == handle)
    }

    /// The vertex input at `location`, if the program reads it.
    #[must_use]
    pub fn input(&self, location: u32) -> Option<VertexInput> {
        self.inputs.iter().copied().find(|input| input.location == location)
    }
}

/// The module of `program`'s WESL, if it has one.
#[must_use]
pub fn module_of(program: BuiltIn) -> Option<&'static str> {
    PROGRAMS.iter().find(|&&(built_in, _)| built_in == program).map(|&(_, module)| module)
}

/// Each program that has a WESL source.
pub fn programs() -> impl Iterator<Item = BuiltIn> {
    PROGRAMS.iter().map(|&(program, _)| program)
}

/// The program of `key` for `target`, linked once a process. Fails for a
/// program without a WESL source, or one that doesn't link or validate.
pub fn program_of(key: &ShaderKey, target: Target) -> Result<Arc<Program>, String> {
    let program = key.program();
    let module = module_of(program).ok_or_else(|| format!("{program:?} has no WESL source"))?;
    let flags = key.define_values().filter(|(_, value)| value.is_empty()).map(|(name, _)| name);
    self::program(module, target, flags)
}

/// The program `module` linked for `target` with the feature flags `flags`
/// on, once a process.
pub fn program<'a>(
    module: &'static str,
    target: Target,
    flags: impl IntoIterator<Item = &'a str>,
) -> Result<Arc<Program>, String> {
    type Key = (&'static str, Target, Vec<String>);
    static PROGRAMS: LazyLock<Mutex<HashMap<Key, Arc<Program>>>> = LazyLock::new(Mutex::default);

    let mut flags: Vec<String> = flags.into_iter().map(str::to_owned).collect();
    flags.sort_unstable();
    flags.dedup();
    let key = (module, target, flags);
    if let Some(program) = PROGRAMS.lock().get(&key) {
        return Ok(Arc::clone(program));
    }
    let program = Arc::new(link(module, target, &key.2)?);
    PROGRAMS.lock().insert(key, Arc::clone(&program));
    Ok(program)
}

/// Links, parses and validates `module` uncached; [`program`] caches it.
pub fn link(module: &'static str, target: Target, flags: &[String]) -> Result<Program, String> {
    let linked = link::link(module, target, flags)?;
    let wgsl = linked.wgsl;
    let parsed = naga::front::wgsl::parse_str(&wgsl)
        .map_err(|error| format!("{module} ({target:?}): {}", error.emit_to_string(&wgsl)))?;
    let info = Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate(&parsed)
        .map_err(|error| format!("{module} ({target:?}): {}", error.emit_to_string(&wgsl)))?;
    let resources = resources(&parsed, &info, &linked.declared)?;
    let inputs = inputs(&parsed)?;
    let entry_point = |stage: Stage| {
        parsed
            .entry_points
            .iter()
            .position(|entry| entry.stage == stage.naga() && entry.name == stage.entry_point())
            .ok_or_else(|| format!("{module} has no {stage:?} entry point {}", stage.entry_point()))
    };
    let entry_points = [entry_point(Stage::Vertex)?, entry_point(Stage::Fragment)?];
    Ok(Program {
        name: module.rsplit("::").next().unwrap_or(module),
        target,
        module: parsed,
        info,
        wgsl,
        resources,
        inputs,
        index_group: linked.index_group,
        entry_points,
        written: Written::default(),
    })
}

/// The resources of `module` that either entry point uses.
fn resources(
    module: &naga::Module,
    info: &ModuleInfo,
    declared: &HashMap<String, Declared>,
) -> Result<Vec<Resource>, String> {
    let mut layouter = naga::proc::Layouter::default();
    layouter.update(module.to_ctx()).map_err(|error| format!("{error:?}"))?;
    let used_by = |stage: Stage, handle: naga::Handle<naga::GlobalVariable>| {
        module
            .entry_points
            .iter()
            .position(|entry| entry.stage == stage.naga())
            .is_some_and(|index| !info.get_entry_point(index)[handle].is_empty())
    };
    let (sampled, loaded) = texture_uses(module);
    let mut resources = Vec::new();
    for (handle, global) in module.global_variables.iter() {
        let Some(binding) = global.binding else { continue };
        let name = global.name.clone().unwrap_or_default();
        let resource =
            *declared.get(&name).ok_or_else(|| format!("{name} has a binding the program doesn't declare"))?;
        let (vertex, fragment) = (used_by(Stage::Vertex, handle), used_by(Stage::Fragment, handle));
        if !vertex && !fragment {
            continue;
        }
        // The kind declared: OpenGL's storage buffers are uniform blocks.
        let kind = match resource.kind {
            DeclaredKind::Texture => {
                ResourceKind::Texture { filterable: sampled.contains(&handle) || !loaded.contains(&handle) }
            }
            kind => kind.into(),
        };
        let size = match kind {
            ResourceKind::UniformBuffer | ResourceKind::StorageBuffer => u64::from(layouter[global.ty].size),
            _ => 0,
        };
        resources.push(Resource {
            name,
            handle,
            kind,
            set: resource.set,
            id: resource.id,
            binding,
            size,
            vertex,
            fragment,
        });
    }
    Ok(resources)
}

/// The textures that some function of `module` samples, and those it loads
/// texels of, where it names the global itself.
fn texture_uses(
    module: &naga::Module,
) -> (Vec<naga::Handle<naga::GlobalVariable>>, Vec<naga::Handle<naga::GlobalVariable>>) {
    let (mut sampled, mut loaded) = (Vec::new(), Vec::new());
    let functions = module.functions.iter().map(|(_, function)| function);
    for function in functions.chain(module.entry_points.iter().map(|entry| &entry.function)) {
        for (_, expression) in function.expressions.iter() {
            let (image, uses) = match *expression {
                naga::Expression::ImageSample { image, .. } => (image, &mut sampled),
                naga::Expression::ImageLoad { image, .. } => (image, &mut loaded),
                _ => continue,
            };
            if let naga::Expression::GlobalVariable(global) = function.expressions[image] {
                uses.push(global);
            }
        }
    }
    (sampled, loaded)
}

/// The inputs of `module`'s vertex entry point.
fn inputs(module: &naga::Module) -> Result<Vec<VertexInput>, String> {
    let entry = module
        .entry_points
        .iter()
        .find(|entry| entry.stage == naga::ShaderStage::Vertex)
        .ok_or("no vertex entry point")?;
    let mut inputs = Vec::new();
    let mut add = |binding: &Option<naga::Binding>, ty: naga::Handle<naga::Type>| {
        if let Some(naga::Binding::Location { location, .. }) = *binding {
            let inner = &module.types[ty].inner;
            let scalar = inner.scalar().map_or(naga::ScalarKind::Float, |scalar| scalar.kind);
            let components = match *inner {
                naga::TypeInner::Vector { size, .. } => size as u32,
                _ => 1,
            };
            inputs.push(VertexInput { location, scalar, components });
        }
    };
    for argument in &entry.function.arguments {
        match &module.types[argument.ty].inner {
            naga::TypeInner::Struct { members, .. } => {
                for member in members {
                    add(&member.binding, member.ty);
                }
            }
            _ => add(&argument.binding, argument.ty),
        }
    }
    inputs.sort_by_key(|input| input.location);
    Ok(inputs)
}
