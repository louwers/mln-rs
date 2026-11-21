//! Every variant of the engine's programs compiles on each backend this
//! platform has: in both projections, with and without the overdraw
//! inspector.

use mln_gfx::RendererBackend;
use mln_gfx::gpu::{Gpu, ShaderKey};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::program::ProgramParameters;

use crate::shaders::programs::PROGRAMS;

/// The key of each variant of each engine program.
fn keys() -> Vec<ShaderKey> {
    let mut keys = Vec::new();
    for program in PROGRAMS {
        for overdraw in [false, true] {
            for variant in [ProjectionVariant::Mercator, ProjectionVariant::Globe] {
                let parameters = ProgramParameters { pixel_ratio: 1.0, overdraw };
                keys.push(program.variant_key(&parameters, [], variant));
            }
        }
    }
    keys
}

/// Compiles every variant through `gpu`.
fn compile_all(gpu: &mut (impl Gpu + ?Sized)) {
    for key in keys() {
        if let Err(error) = gpu.create_shader(&key) {
            panic!("{:?} ({:?}): {error}", key.program(), key.define_values().collect::<Vec<_>>());
        }
    }
}

#[test]
fn every_engine_program_compiles_on_webgpu() {
    let device = mln_webgpu::Device::new(None).expect("device");
    compile_all(&mut *mln_webgpu::RendererBackend::new(&device).gpu());
}

#[cfg(target_vendor = "apple")]
#[test]
fn every_engine_program_compiles_on_metal() {
    let device = mln_mtl::Device::new(None).expect("device");
    compile_all(&mut *mln_mtl::RendererBackend::new(&device).gpu());
}

/// On GL, where the platform has a device.
#[test]
fn every_engine_program_compiles_on_gl() {
    match mln_gl::Device::with_options(mln_gl::DeviceOptions::default()) {
        Ok(device) => compile_all(&mut *mln_gl::RendererBackend::new(&device).expect("backend").gpu()),
        Err(error) => eprintln!("skipped: no GL device: {error}"),
    }
}

/// Through the GLSL that the Vulkan and D3D12 backends parse.
#[cfg(not(any(target_vendor = "apple", target_family = "wasm")))]
#[test]
fn every_engine_program_parses_as_glsl() {
    use mln_glsl::variants::variants;
    use mln_glsl::{Module, Stage, compose};
    for program in PROGRAMS {
        let source = program.sources.glsl.expect("GLSL");
        for variant in variants(program, true) {
            for stage in [Stage::Vertex, Stage::Fragment] {
                let composed = compose(program, source, stage, &variant.defines).expect("composes");
                if let Err(error) = Module::parse(stage, &composed) {
                    panic!("{program:?} ({}, {stage:?}): {error}", variant.name);
                }
            }
        }
    }
}
