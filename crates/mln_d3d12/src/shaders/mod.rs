//! The D3D12 shaders: the HLSL that `mln_shaders` writes of a program, for
//! the root signature (`hlsl`), which DXC compiles to DXIL (`dxc`) when the
//! program is first used.

mod dxc;
pub(crate) mod hlsl;

use std::sync::Arc;

use mln_gfx::gpu::{GpuError, ShaderKey};
use mln_shaders::{Resource, Stage};
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
    /// The resources either stage uses, at their sets and bindings of the
    /// Vulkan layout (`mln_shaders::layout`).
    pub resources: Vec<Resource>,
    /// The attributes the vertex stage reads, by id: each at the location
    /// of its id.
    pub attributes: Vec<u32>,
}

impl ShaderCompiler {
    /// Loads DXC. Without `dxcompiler.dll` on the DLL search path, it fails
    /// with `GpuError::Unsupported`.
    pub fn new() -> Result<ShaderCompiler, GpuError> {
        let dxc = Dxc::load().map_err(|error| GpuError::Unsupported(format!("DXC: {error}")))?;
        Ok(ShaderCompiler { dxc })
    }

    /// The program of `key`, compiled once a process: it doesn't depend on
    /// the device, and every renderer compiles the same.
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
        let program = mln_shaders::program_of(key, mln_shaders::Target::D3d12)
            .map_err(|error| GpuError::Invalid(format!("{:?}: {error}", key.program())))?;
        let name = program.name;
        let compile = |stage: Stage| -> Result<Vec<u8>, GpuError> {
            let hlsl = program.hlsl(stage).map_err(GpuError::Invalid)?;
            self.dxc.compile(stage, hlsl).map_err(|error| GpuError::Invalid(format!("{name} {stage:?}: {error}")))
        };
        Ok(CompiledProgram {
            name,
            vertex: compile(Stage::Vertex)?,
            fragment: compile(Stage::Fragment)?,
            resources: program.resources.clone(),
            attributes: program.inputs.iter().map(|input| input.location).collect(),
        })
    }

    /// Compiles the HLSL `source` of a `stage`, whose entry point is
    /// `entry_point`, to DXIL for shader model 6.0: a host's own shaders, such
    /// as an overlay's, which bind through a root signature of the host's.
    pub fn compile_hlsl(&self, stage: Stage, source: &str, entry_point: &str) -> Result<Vec<u8>, GpuError> {
        let hlsl = hlsl::Hlsl { source: source.to_owned(), entry_point: entry_point.to_owned() };
        self.dxc.compile(stage, &hlsl).map_err(GpuError::Invalid)
    }
}
