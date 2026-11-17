//! The D3D12 shaders: a program's WESL from `mln_shaders`, or else
//! `mln_glsl`'s GLSL, which naga writes as HLSL for the root signature
//! (`hlsl`) and DXC compiles to DXIL (`dxc`), when a program is first used.

mod dxc;
pub(crate) mod hlsl;

use std::fmt::Write as _;
use std::sync::Arc;

use mln_gfx::gpu::{GpuError, ShaderKey};
use mln_glsl::{AttributeInfo, Module, ResourceUse, ShaderSource, Stage, TextureInfo, shader_source};
use mln_shaders::ResourceKind;
use mln_util::hash::HashMap;
use parking_lot::Mutex;

use self::dxc::Dxc;

/// Compiles programs to DXIL.
pub struct ShaderCompiler {
    dxc: Dxc,
}

/// A program compiled to DXIL.
#[derive(Debug)]
pub struct CompiledProgram {
    pub name: &'static str,
    pub vertex: Vec<u8>,
    pub fragment: Vec<u8>,
    /// The resources the stages use, each once.
    pub resources: Vec<ResourceUse>,
    pub attributes: &'static [AttributeInfo],
    pub instance_attributes: &'static [AttributeInfo],
    /// Whether an attribute's location is its id, as in the programs of
    /// `mln_shaders`, rather than its index in the tables.
    pub location_is_id: bool,
    /// The image set's textures: each texture's id and index.
    pub textures: Vec<TextureInfo>,
}

impl ShaderCompiler {
    /// Loads DXC. Without `dxcompiler.dll` on the DLL search path, it fails
    /// with `GpuError::Unsupported`.
    pub fn new() -> Result<ShaderCompiler, GpuError> {
        let dxc = Dxc::load().map_err(|error| GpuError::Unsupported(format!("DXC: {error}")))?;
        Ok(ShaderCompiler { dxc })
    }

    /// The program of `key` with its defines, compiled once a process: it
    /// doesn't depend on the device, and every renderer compiles the same.
    pub fn compile(&self, key: &ShaderKey) -> Result<Arc<CompiledProgram>, GpuError> {
        static PROGRAMS: Mutex<Option<HashMap<ShaderKey, Arc<CompiledProgram>>>> = Mutex::new(None);
        if let Some(program) = PROGRAMS.lock().get_or_insert_default().get(key) {
            return Ok(Arc::clone(program));
        }
        let program = Arc::new(self.compile_uncached(key)?);
        PROGRAMS.lock().get_or_insert_default().insert(key.clone(), Arc::clone(&program));
        Ok(program)
    }

    fn compile_uncached(&self, key: &ShaderKey) -> Result<CompiledProgram, GpuError> {
        let program = key.program();
        let source = shader_source(program).ok_or_else(|| GpuError::Unsupported(format!("no GLSL for {program:?}")))?;
        if let Some(wesl) = mln_shaders::program_of(key, mln_shaders::Target::D3d12) {
            return wesl
                .and_then(|wesl| self.compile_wesl(source, &wesl))
                .map_err(|error| GpuError::Invalid(format!("{}: {error}", source.name)));
        }
        let mut defines = String::new();
        for (name, value) in key.define_values() {
            let _ = writeln!(defines, "#define {name} {value}");
        }
        self.compile_source(source, &defines).map_err(|error| GpuError::Invalid(format!("{}: {error}", source.name)))
    }

    /// Compiles the HLSL `source` of a `stage`, whose entry point is
    /// `entry_point`, to DXIL for shader model 6.0: a host's own shaders, such
    /// as an overlay's, which bind through a root signature of the host's.
    pub fn compile_hlsl(&self, stage: Stage, source: &str, entry_point: &str) -> Result<Vec<u8>, GpuError> {
        let hlsl = hlsl::Hlsl { source: source.to_owned(), entry_point: entry_point.to_owned() };
        self.dxc.compile(stage, &hlsl).map_err(GpuError::Invalid)
    }

    /// Compiles `source` with `defines` (`#define` lines).
    pub(crate) fn compile_source(
        &self,
        source: &'static ShaderSource,
        defines: &str,
    ) -> Result<CompiledProgram, String> {
        let mut resources: Vec<ResourceUse> = Vec::new();
        let mut compile = |stage| -> Result<Vec<u8>, String> {
            let module = Module::parse(stage, &source.compose(stage, defines))?;
            for resource in module.resources()? {
                if !resources.contains(&resource) {
                    resources.push(resource);
                }
            }
            let hlsl = hlsl::write(stage, &module)?;
            self.dxc.compile(stage, &hlsl).map_err(|error| format!("{stage:?}: {error}"))
        };
        let vertex = compile(Stage::Vertex)?;
        let fragment = compile(Stage::Fragment)?;
        Ok(CompiledProgram {
            name: source.name,
            vertex,
            fragment,
            resources,
            attributes: source.attributes,
            instance_attributes: source.instance_attributes,
            location_is_id: false,
            textures: source.textures.to_vec(),
        })
    }

    /// Compiles `wesl`, the WESL of `source`'s program linked for the
    /// variant, whose bindings are the Vulkan layout's with each texture at
    /// the index of its id.
    fn compile_wesl(
        &self,
        source: &'static ShaderSource,
        wesl: &mln_shaders::Program,
    ) -> Result<CompiledProgram, String> {
        let compile = |stage: mln_shaders::Stage| -> Result<Vec<u8>, String> {
            let hlsl = wesl.hlsl(stage)?;
            let stage = match stage {
                mln_shaders::Stage::Vertex => Stage::Vertex,
                mln_shaders::Stage::Fragment => Stage::Fragment,
            };
            self.dxc.compile(stage, hlsl).map_err(|error| format!("{stage:?}: {error}"))
        };
        let vertex = compile(mln_shaders::Stage::Vertex)?;
        let fragment = compile(mln_shaders::Stage::Fragment)?;
        let resources = wesl
            .resources
            .iter()
            .map(|resource| ResourceUse {
                set: resource.binding.group,
                binding: resource.binding.binding,
                size: resource.size,
            })
            .collect();
        let textures = wesl
            .resources
            .iter()
            .filter(|resource| matches!(resource.kind, ResourceKind::Texture { .. }))
            .map(|resource| TextureInfo { index: resource.id as usize, id: resource.id as usize })
            .collect();
        Ok(CompiledProgram {
            name: source.name,
            vertex,
            fragment,
            resources,
            attributes: source.attributes,
            instance_attributes: source.instance_attributes,
            location_is_id: true,
            textures,
        })
    }
}
