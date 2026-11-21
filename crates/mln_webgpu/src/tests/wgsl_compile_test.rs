// Compiles every built-in program's WGSL as `WebGpu::create_shader` does, in
// both projection variants, with and without the overdraw inspector, with
// every property it declares read per vertex and with every one read from
// its uniform, and makes a pipeline of each with the attributes its program
// declares.

use std::collections::BTreeSet;

use mln_gfx::color_mode::{BlendFunction, Mask};
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::gfx_types::DrawModeType;
use mln_gfx::gpu::{DepthState, FixedState, StencilState, VertexAttribute, VertexBufferLayout, VertexStep};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_gfx::shaders::source::wgsl::Wgsl;
use mln_gfx::stencil_mode::StencilMode;
use mln_gfx::types::DepthFunctionType;

use crate::Device;
use crate::index_table::IndexTable;
use crate::shaders::shader_program::compile;
use crate::shaders::wgsl_sources::PRELUDE;

/// Each program with WGSL: the engine's and the built-in layer types'.
fn programs() -> Vec<(&'static Program, &'static Wgsl)> {
    let programs = mln_render::PROGRAMS.iter().chain(mln_layers::PROGRAMS);
    programs.filter_map(|&program| Some((program, program.sources.wgsl?))).collect()
}

/// The names after the `HAS_UNIFORM_u_` that `wgsl` tests.
fn uniforms(wgsl: &Wgsl) -> BTreeSet<String> {
    const PREFIX: &str = "HAS_UNIFORM_u_";
    let mut names = BTreeSet::new();
    for text in [wgsl.prelude, wgsl.vertex, wgsl.fragment].into_iter().flatten() {
        for (start, _) in text.match_indices(PREFIX) {
            let rest = &text[start + PREFIX.len()..];
            let end = rest.find(|c: char| !c.is_ascii_alphanumeric() && c != '_').unwrap_or(rest.len());
            names.insert(rest[..end].to_owned());
        }
    }
    names
}

/// A variant's defines read a property from its uniform only where the
/// program declares it.
#[test]
fn every_program_declares_the_properties_its_wgsl_reads_from_uniforms() {
    for (program, wgsl) in programs() {
        let declared: BTreeSet<String> = program.properties.iter().map(|property| property.name.to_owned()).collect();
        assert_eq!(uniforms(wgsl), declared, "{}", program.name);
    }
}

/// Every program that draws a layer outputs white for the overdraw
/// inspector, which counts its draws.
#[test]
fn every_layer_program_shows_the_overdraw_inspector() {
    let not_layers = [
        "ClippingMaskProgram",
        "CollisionBoxShader",
        "CollisionCircleShader",
        "GlobeDepthShader",
        "SkyShader",
        "AtmosphereShader",
    ];
    for (program, wgsl) in programs() {
        if !not_layers.contains(&program.name) {
            assert!(
                [wgsl.prelude, wgsl.vertex, wgsl.fragment]
                    .into_iter()
                    .flatten()
                    .any(|text| text.contains("OVERDRAW_INSPECTOR")),
                "{}",
                program.name
            );
        }
    }
}

#[test]
fn every_program_compiles() {
    let device = Device::new(None).expect("device");
    let device = device.device();
    let index_table = IndexTable::new(device);
    let mut failed = Vec::new();
    let parameters = [false, true].map(|overdraw| ProgramParameters::new(1.0, overdraw));
    for (program, wgsl) in programs() {
        for parameters in &parameters {
            for variant in [ProjectionVariant::Mercator, ProjectionVariant::Globe] {
                for uniforms in [&[][..], program.properties] {
                    let key = program.variant_key(parameters, uniforms, variant);
                    match compile(device, index_table.layout(), &key) {
                        Ok(compiled) => {
                            if let Err(error) = compiled.render_pipeline(
                                &layouts(program),
                                &state(),
                                Some(wgpu::TextureFormat::Rgba8Unorm),
                                Some(wgpu::TextureFormat::Depth24PlusStencil8),
                            ) {
                                failed.push(format!("{} ({variant:?}, {:?}): {error}", program.name, key.defines()));
                            }
                            let text = [Some(PRELUDE), wgsl.prelude, wgsl.vertex, wgsl.fragment];
                            assert_eq!(
                                compiled.index_group().is_some(),
                                text.into_iter().flatten().any(|text| text.contains("globalIndex")),
                                "{} binds globalIndex in a group of its own",
                                program.name
                            );
                        }
                        Err(error) => {
                            failed.push(format!("{} ({variant:?}, {:?}): {error}", program.name, key.defines()));
                        }
                    }
                }
            }
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// A pipeline's state that draws triangles with no tests.
fn state() -> FixedState {
    FixedState {
        primitive: DrawModeType::Triangles,
        cull: CullFaceMode::disabled(),
        blend: BlendFunction::Replace,
        color_mask: Mask { r: true, g: true, b: true, a: true },
        depth: DepthState { compare: DepthFunctionType::Always, write: false },
        stencil: StencilState::from(&StencilMode::disabled()),
    }
}

/// A vertex buffer of `program`'s attributes, one after the other in the
/// formats it declares, and one of its instance attributes.
fn layouts(program: &Program) -> Vec<VertexBufferLayout> {
    [(VertexStep::Vertex, program.attributes), (VertexStep::Instance, program.instance_attributes)]
        .into_iter()
        .filter(|(_, attributes)| !attributes.is_empty())
        .map(|(step, attributes)| {
            let mut stride = 0;
            let attributes = attributes
                .iter()
                .map(|attribute| {
                    let offset = stride;
                    stride += attribute.format.stride() as u32;
                    VertexAttribute { id: attribute.id as u32, format: attribute.format, offset }
                })
                .collect();
            VertexBufferLayout { stride, step, attributes }
        })
        .collect()
}
