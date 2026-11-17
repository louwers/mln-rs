//! The renderer's programs, each written once in WESL (`shaders/`).
//!
//! A program is `shaders/programs/<BuiltIn>.wesl`, named after its
//! [`BuiltIn`]; the other modules are shared. Its entry points are `vs_main`
//! and `fs_main` ([`Stage::entry_point`]). A backend gets a program, for its
//! target and a variant, from [`program_of`].
//!
//! # Feature flags
//!
//! A variant has the feature flags of its `ShaderKey`'s defines that have
//! no value (`HAS_UNIFORM_u_<name>`, `PROJECTION_GLOBE`,
//! `OVERDRAW_INSPECTOR`), and a target ([`Target`]) its own: `vulkan`,
//! `d3d12`, `gl`, `metal` or `webgpu`. A define with a value, such as
//! `DEVICE_PIXEL_RATIO`, is no flag: a program reads the value from a
//! uniform.
//!
//! # Linking
//!
//! The build script links each program once for each target, with the
//! target's flag on, the other targets' off and the variant flags left
//! undecided. Each `@if` that tests a variant flag becomes `#if` lines
//! around its declaration, struct member, parameter or statement, and each
//! declaration takes the condition under which an entry point reaches it, so
//! that a variant's lines are what WESL links for the variant. With the text
//! come tables of the program's resources and vertex inputs, each with the
//! conditions under which a variant has it and each stage uses it.
//!
//! At run time a variant is the lines of its flags (`preprocess`), its
//! resources and inputs those of the tables whose conditions hold, and each
//! resource's `@group` and `@binding` its target's ([`Program::resources`]).
//! naga parses and validates the WGSL for every target but WebGPU, which
//! takes it as it is, and writes the backend's language.
//!
//! A crate feature embeds each target's programs: `spirv` (Vulkan), `hlsl`
//! (D3D12), `glsl` (OpenGL), `msl` (Metal), each with naga and its writer,
//! and `wgsl` (WebGPU), without naga.
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
//! draw indexes with `globals::drawable_index()`. Each target binds them
//! in its own convention:
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

mod bindings;
pub mod layout;
mod preprocess;
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
#[cfg(any(test, feature = "naga"))]
pub use naga;
use parking_lot::Mutex;

use crate::bindings::{Declared, DeclaredKind};
pub use crate::layout::DescriptorSetType;
use crate::preprocess::{eval, preprocess};
pub use crate::variants::{MAX_EXHAUSTIVE_FLAGS, flags_of, variants};

include!(concat!(env!("OUT_DIR"), "/built.rs"));

/// A program linked for a target with its variant flags left to decide:
/// the text for the preprocessor, and its resources and vertex inputs, each
/// with the conditions under which a variant has it.
struct Built {
    text: &'static str,
    /// In the order of their macros' indexes.
    resources: &'static [BuiltResource],
    inputs: &'static [BuiltInput],
}

/// A resource of a [`Built`], as the WESL declares it.
struct BuiltResource {
    name: &'static str,
    kind: DeclaredKind,
    group: u32,
    binding: u32,
    /// The bytes of a buffer, of an element of a storage buffer.
    size: u64,
    /// When the vertex stage uses it.
    vertex: &'static str,
    /// When the fragment stage uses it.
    fragment: &'static str,
    /// When a texture is sampled, or never loaded.
    filterable: &'static str,
}

/// A vertex input of a [`Built`].
struct BuiltInput {
    location: u32,
    scalar: ScalarKind,
    components: u32,
    /// When the vertex stage has it.
    cond: &'static str,
}

/// The [`Built`] of the program at `index` in `PROGRAMS` for `target`, if
/// the crate embeds the target's programs.
fn built(index: usize, target: Target) -> Option<&'static Built> {
    let programs: &[Built] = match target {
        Target::Vulkan => VULKAN,
        Target::D3d12 => D3D12,
        Target::Gl => GL,
        Target::Metal => METAL,
        Target::WebGpu => WEBGPU,
    };
    programs.get(index)
}

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

    #[cfg(any(test, feature = "naga"))]
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

/// The group and binding of a resource in a target's module.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ResourceBinding {
    pub group: u32,
    pub binding: u32,
}

/// A resource of a program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resource {
    /// The global variable's name in the module.
    pub name: &'static str,
    pub kind: ResourceKind,
    /// The descriptor set its WESL declares it in.
    pub set: DescriptorSetType,
    /// The renderer's id of the buffer or texture; of a sampler's texture.
    pub id: u32,
    /// Its group and binding in the target's module.
    pub binding: ResourceBinding,
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

/// The kind of the scalars of a vertex input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarKind {
    Float,
    Sint,
    Uint,
}

/// A vertex input of a program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VertexInput {
    /// The input's location: its attribute's id.
    pub location: u32,
    /// The kind of its scalars: an integer input reads integers.
    pub scalar: ScalarKind,
    /// The number of its components, 1 to 4.
    pub components: u32,
}

impl VertexInput {
    /// Whether the input reads integers, unconverted.
    #[must_use]
    pub fn is_integer(&self) -> bool {
        matches!(self.scalar, ScalarKind::Sint | ScalarKind::Uint)
    }
}

/// A program for a target and a variant.
#[derive(Debug)]
pub struct Program {
    /// The program's name, its `BuiltIn`'s.
    pub name: &'static str,
    pub target: Target,
    /// The variant's WGSL.
    pub wgsl: String,
    /// The resources either stage uses.
    pub resources: Vec<Resource>,
    pub inputs: Vec<VertexInput>,
    /// For WebGPU, the group of the draw's index (`ID_GLOBAL_UBO_INDEX`), if
    /// the program reads it.
    pub index_group: Option<u32>,
    /// The module naga parsed and validated: for every target but WebGPU,
    /// which takes the WGSL as it is.
    #[cfg(any(test, feature = "naga"))]
    naga: Option<Validated>,
    /// What the writers of the backends' languages wrote, once each.
    #[cfg(any(test, feature = "naga"))]
    #[cfg_attr(
        not(any(test, feature = "glsl", feature = "hlsl", feature = "msl", feature = "spirv")),
        expect(dead_code, reason = "only the writers read it")
    )]
    written: Written,
}

/// A program's module, parsed and validated by naga.
#[cfg(any(test, feature = "naga"))]
#[derive(Debug)]
pub(crate) struct Validated {
    pub(crate) module: naga::Module,
    pub(crate) info: naga::valid::ModuleInfo,
    /// The index in the module of each stage's entry point.
    entry_points: [usize; 2],
}

/// The outputs of the writers, by stage where they write one.
#[cfg(any(test, feature = "naga"))]
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
    /// The vertex input at `location`, if the program reads it.
    #[must_use]
    pub fn input(&self, location: u32) -> Option<VertexInput> {
        self.inputs.iter().copied().find(|input| input.location == location)
    }
}

#[cfg(any(test, feature = "naga"))]
impl Program {
    /// The module naga validated; none for WebGPU.
    pub(crate) fn validated(&self) -> Result<&Validated, String> {
        self.naga.as_ref().ok_or_else(|| format!("{} for {:?} has no naga module", self.name, self.target))
    }

    /// The index of `stage`'s entry point in the module.
    #[cfg_attr(not(any(test, feature = "msl")), expect(dead_code, reason = "only MSL reads it"))]
    pub(crate) fn entry_point_index(&self, stage: Stage) -> Result<usize, String> {
        Ok(self.validated()?.entry_points[stage as usize])
    }

    /// The resource of the global `handle` of the module, if it is one.
    #[cfg_attr(not(any(test, feature = "glsl")), expect(dead_code, reason = "only GLSL reads it"))]
    pub(crate) fn resource_of(&self, handle: naga::Handle<naga::GlobalVariable>) -> Option<(usize, &Resource)> {
        let name = self.validated().ok()?.module.global_variables[handle].name.as_deref()?;
        self.resources.iter().enumerate().find(|(_, resource)| resource.name == name)
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

/// The program of `key` for `target`, made once a process. Fails for a
/// program without a WESL source, or one that doesn't validate.
pub fn program_of(key: &ShaderKey, target: Target) -> Result<Arc<Program>, String> {
    let program = key.program();
    let module = module_of(program).ok_or_else(|| format!("{program:?} has no WESL source"))?;
    let flags = key.define_values().filter(|(_, value)| value.is_empty()).map(|(name, _)| name);
    self::program(module, target, flags)
}

/// The program `module` for `target` with the feature flags `flags` on,
/// once a process.
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

/// Makes `module`'s variant of `flags` for `target` uncached: its lines of
/// the program linked for the target, and naga's module of them for every
/// target but WebGPU. [`program`] caches it.
pub fn link(module: &'static str, target: Target, flags: &[String]) -> Result<Program, String> {
    let index = PROGRAMS.iter().position(|&(_, m)| m == module).ok_or_else(|| format!("no program {module}"))?;
    let built =
        built(index, target).ok_or_else(|| format!("{module}: the crate doesn't embed {target:?}'s programs"))?;
    let name = module.rsplit("::").next().unwrap_or(module);
    let fail = |error: String| format!("{name} ({target:?}): {error}");
    let on = |flag: &str| flags.iter().any(|f| f == flag);

    let mut used = Vec::new();
    for (index, resource) in built.resources.iter().enumerate() {
        let (vertex, fragment) =
            (eval(resource.vertex, &on).map_err(fail)?, eval(resource.fragment, &on).map_err(fail)?);
        if vertex || fragment {
            let declared =
                Declared::new(resource.name, resource.kind, resource.group, resource.binding).map_err(fail)?;
            used.push((index, resource, declared, vertex, fragment));
        }
    }
    let declared: Vec<Declared> = used.iter().map(|&(_, _, declared, _, _)| declared).collect();
    let (bindings, index_group) = bindings::bindings(&declared, target).map_err(fail)?;
    let mut resources = Vec::with_capacity(used.len());
    for (&(_, resource, declared, vertex, fragment), &binding) in used.iter().zip(&bindings) {
        let kind = match declared.kind {
            DeclaredKind::Texture => {
                ResourceKind::Texture { filterable: eval(resource.filterable, &on).map_err(fail)? }
            }
            kind => kind.into(),
        };
        let size = if declared.kind.is_buffer() { resource.size } else { 0 };
        resources.push(Resource {
            name: resource.name,
            kind,
            set: declared.set,
            id: declared.id,
            binding,
            size,
            vertex,
            fragment,
        });
    }
    let mut inputs = Vec::new();
    for input in built.inputs {
        if eval(input.cond, &on).map_err(fail)? {
            inputs.push(VertexInput { location: input.location, scalar: input.scalar, components: input.components });
        }
    }

    // `__mln_group_<n>` and `__mln_binding_<n>` of the resource at `n`.
    let define = |name: &str| {
        let (group, index) = if let Some(index) = name.strip_prefix("__mln_group_") {
            (true, index)
        } else {
            (false, name.strip_prefix("__mln_binding_")?)
        };
        let index: usize = index.parse().ok()?;
        let at = used.iter().position(|&(i, ..)| i == index)?;
        Some(if group { bindings[at].group } else { bindings[at].binding })
    };
    let wgsl = preprocess(built.text, &on, &define).map_err(fail)?;

    Ok(Program {
        name,
        target,
        #[cfg(any(test, feature = "naga"))]
        naga: (target != Target::WebGpu).then(|| validate(&wgsl)).transpose().map_err(fail)?,
        wgsl,
        resources,
        inputs,
        index_group,
        #[cfg(any(test, feature = "naga"))]
        written: Written::default(),
    })
}

/// Parses and validates `wgsl`, a program.
#[cfg(any(test, feature = "naga"))]
fn validate(wgsl: &str) -> Result<Validated, String> {
    use naga::valid::{Capabilities, ValidationFlags, Validator};

    let module = naga::front::wgsl::parse_str(wgsl).map_err(|error| error.emit_to_string(wgsl))?;
    let info = Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate(&module)
        .map_err(|error| error.emit_to_string(wgsl))?;
    let entry_point = |stage: Stage| {
        module
            .entry_points
            .iter()
            .position(|entry| entry.stage == stage.naga() && entry.name == stage.entry_point())
            .ok_or_else(|| format!("no {stage:?} entry point {}", stage.entry_point()))
    };
    let entry_points = [entry_point(Stage::Vertex)?, entry_point(Stage::Fragment)?];
    Ok(Validated { module, info, entry_points })
}
