//! The D3D12 shaders: `mln_glsl`'s GLSL, which naga writes as HLSL for the
//! root signature (`hlsl`) and DXC compiles to DXIL (`dxc`), when a program
//! is first used.

mod dxc;
pub(crate) mod hlsl;

use std::fmt::Write as _;
use std::sync::Arc;

use mln_gfx::gpu::{GpuError, ShaderKey};
use mln_gfx::shaders::program::Program;
use mln_glsl::{Module, ResourceUse, ShaderSource, Stage};
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
    /// The program it is compiled from.
    pub program: &'static Program,
    pub vertex: Vec<u8>,
    pub fragment: Vec<u8>,
    /// The resources the stages use, each once.
    pub resources: Vec<ResourceUse>,
    /// Whether the vertex stage reads the vertex index, which includes the
    /// base vertex in naga's special constants.
    pub reads_vertex_index: bool,
}

impl ShaderCompiler {
    /// Loads DXC. Without `dxcompiler.dll` on the DLL search path, it fails
    /// with `GpuError::Unsupported`.
    pub fn new() -> Result<ShaderCompiler, GpuError> {
        let dxc = Dxc::load().map_err(|error| GpuError::Unsupported(format!("DXC: {error}")))?;
        Ok(ShaderCompiler { dxc })
    }

    /// The program of `key` with its defines, compiled from `source` once a
    /// process: it doesn't depend on the device, and every renderer compiles
    /// the same.
    pub fn compile(&self, key: &ShaderKey, source: &'static ShaderSource) -> Result<Arc<CompiledProgram>, GpuError> {
        static PROGRAMS: Mutex<Option<HashMap<ShaderKey, Arc<CompiledProgram>>>> = Mutex::new(None);
        if let Some(program) = PROGRAMS.lock().get_or_insert_default().get(key) {
            return Ok(Arc::clone(program));
        }
        let program = Arc::new(self.compile_uncached(key, source)?);
        PROGRAMS.lock().get_or_insert_default().insert(key.clone(), Arc::clone(&program));
        Ok(program)
    }

    fn compile_uncached(&self, key: &ShaderKey, source: &'static ShaderSource) -> Result<CompiledProgram, GpuError> {
        let program = key.program();
        let mut defines = String::new();
        for (name, value) in key.define_values() {
            let _ = writeln!(defines, "#define {name} {value}");
        }
        self.compile_source(program, source, &defines)
            .map_err(|error| GpuError::Invalid(format!("{}: {error}", program.name)))
    }

    /// Compiles the HLSL `source` of a `stage`, whose entry point is
    /// `entry_point`, to DXIL for shader model 6.0: a host's own shaders, such
    /// as an overlay's, which bind through a root signature of the host's.
    pub fn compile_hlsl(&self, stage: Stage, source: &str, entry_point: &str) -> Result<Vec<u8>, GpuError> {
        let hlsl = hlsl::Hlsl { source: source.to_owned(), entry_point: entry_point.to_owned() };
        self.dxc.compile(stage, &hlsl).map_err(GpuError::Invalid)
    }

    /// Compiles `program` from `source` with `defines` (`#define` lines).
    pub(crate) fn compile_source(
        &self,
        program: &'static Program,
        source: &'static ShaderSource,
        defines: &str,
    ) -> Result<CompiledProgram, String> {
        let mut resources: Vec<ResourceUse> = Vec::new();
        let mut reads_vertex_index = false;
        let mut compile = |stage| -> Result<Vec<u8>, String> {
            let module = Module::parse(stage, &source.compose(program, stage, defines)?)?;
            reads_vertex_index |= module.module.entry_points.iter().any(|entry_point| {
                entry_point.function.arguments.iter().any(|argument| {
                    matches!(argument.binding, Some(naga::Binding::BuiltIn(naga::BuiltIn::VertexIndex)))
                })
            });
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
        Ok(CompiledProgram { name: program.name, program, vertex, fragment, resources, reads_vertex_index })
    }
}
