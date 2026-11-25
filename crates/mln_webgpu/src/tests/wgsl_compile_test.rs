// Compiles every built-in program's WGSL as `WebGpu::create_shader` does, in
// both projection variants, with and without the overdraw inspector, with
// every property it declares read per vertex and with every one read from
// its uniform, and makes a pipeline of each, whose layouts are its
// declaration's.

use std::collections::BTreeSet;

use mln_gfx::color_mode::ColorMode;
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::depth_mode::DepthMode;
use mln_gfx::gfx_types::DrawModeType;
use mln_gfx::gpu::{DepthState, FixedState, StencilState, VertexAttribute, VertexBufferLayout, VertexStep};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_gfx::stencil_mode::StencilMode;

use crate::gpu::VERTEX_STAGE_STORAGE_BUFFERS;
use crate::index_table::IndexTable;
use crate::shaders::shader_program::compile;
use crate::shaders::shader_source::{BUILT_INS, WgslShaderSource, prelude};

/// The names after the `HAS_UNIFORM_u_` that `source`'s WGSL tests.
fn uniforms(source: &WgslShaderSource) -> BTreeSet<String> {
    const PREFIX: &str = "HAS_UNIFORM_u_";
    let mut names = BTreeSet::new();
    for text in [source.prelude, source.vertex, source.fragment].into_iter().flatten() {
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
    for &(program, source) in BUILT_INS {
        let declared: BTreeSet<String> = program.properties.iter().map(|property| property.name.to_owned()).collect();
        assert_eq!(uniforms(source), declared, "{}", program.name);
    }
}

/// Every program that draws a layer outputs white for the overdraw
/// inspector, which counts its draws.
#[test]
fn every_layer_program_shows_the_overdraw_inspector() {
    use mln_gfx::shaders::programs::{
        ATMOSPHERE, CLIPPING_MASK, COLLISION_BOX, COLLISION_CIRCLE, GLOBE_DEPTH, SKY, TERRAIN, TERRAIN_DEPTH,
        TERRAIN_PROBE, VERTEX_TEXEL,
    };
    // The terrain shows its drapes, where the draped layers count their
    // draws.
    let not_layers = [
        &CLIPPING_MASK,
        &COLLISION_BOX,
        &COLLISION_CIRCLE,
        &GLOBE_DEPTH,
        &SKY,
        &ATMOSPHERE,
        &TERRAIN,
        &TERRAIN_DEPTH,
        &VERTEX_TEXEL,
        &TERRAIN_PROBE,
    ];
    for &(program, source) in BUILT_INS {
        if !not_layers.contains(&program) {
            assert!(
                [source.prelude, source.vertex, source.fragment]
                    .into_iter()
                    .flatten()
                    .any(|text| text.contains("OVERDRAW_INSPECTOR")),
                "{}",
                program.name
            );
        }
    }
}

/// A vertex buffer for each step of `program`'s attributes, of all of
/// those that step by it, in their declared formats.
fn vertex_layouts(program: &Program) -> Vec<VertexBufferLayout> {
    [VertexStep::Vertex, VertexStep::Instance]
        .into_iter()
        .filter_map(|step| {
            let mut stride = 0;
            let attributes: Vec<VertexAttribute> = program
                .locations()
                .filter(|&(_, at, _)| at == step)
                .map(|(location, _, attribute)| {
                    let offset = stride;
                    stride += attribute.format.stride() as u32;
                    VertexAttribute { location: location as u32, format: attribute.format, offset }
                })
                .collect();
            (!attributes.is_empty()).then_some(VertexBufferLayout { stride, step, attributes })
        })
        .collect()
}

#[test]
fn every_program_compiles() {
    let Some(device) = super::device() else {
        return;
    };
    let device = device.device();
    let index_table = IndexTable::new(device);
    let mut failed = Vec::new();
    let parameters = [false, true].map(|overdraw| ProgramParameters::new(1.0, overdraw));
    let state = FixedState {
        primitive: DrawModeType::Triangles,
        cull: CullFaceMode::disabled(),
        blend: ColorMode::alpha_blended().blend_function,
        color_mask: ColorMode::alpha_blended().mask,
        depth: DepthState::from(&DepthMode::disabled()),
        stencil: StencilState::from(&StencilMode::disabled()),
    };
    for &(program, source) in BUILT_INS {
        for parameters in &parameters {
            for &variant in ProjectionVariant::of(program) {
                for uniforms in [&[][..], program.properties] {
                    let key = program.variant_key(parameters, uniforms, variant);
                    match compile(device, index_table.layout(), &key, source) {
                        Ok(compiled) => {
                            let text = [Some(prelude()), source.prelude, source.vertex, source.fragment];
                            assert_eq!(
                                compiled.index_group().is_some(),
                                text.into_iter().flatten().any(|text| text.contains("globalIndex")),
                                "{} binds globalIndex in a group of its own",
                                program.name
                            );
                            assert!(
                                compiled.vertex_storage_buffers() <= VERTEX_STAGE_STORAGE_BUFFERS,
                                "{}'s vertex stage reads more than VERTEX_STAGE_STORAGE_BUFFERS storage buffers",
                                program.name
                            );
                            let color = Some(wgpu::TextureFormat::Rgba8Unorm);
                            let depth = Some(wgpu::TextureFormat::Depth24PlusStencil8);
                            if let Err(error) = compiled.render_pipeline(&vertex_layouts(program), &state, color, depth)
                            {
                                failed.push(format!("{} ({variant:?}, {:?}): {error}", program.name, key.defines()));
                            }
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
