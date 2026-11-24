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
use mln_gfx::shaders::program::{ENGINE_BUFFERS, Program, ProgramBuffer, ProgramParameters};
use mln_gfx::shaders::source::wgsl::{self, Wgsl};
use mln_gfx::types::DepthFunctionType;

use crate::Device;
use crate::gpu::VERTEX_STAGE_STORAGE_BUFFERS;
use crate::index_table::IndexTable;
use crate::shaders::shader_program::{compile, stage_sources};
use crate::shaders::wgsl_sources::PRELUDE;

/// Each program of the built-in layer types with WGSL, and the
/// conformance cases' own programs.
fn programs() -> Vec<(&'static Program, &'static Wgsl)> {
    let programs = mln_layers::PROGRAMS.iter().chain(&mln_layers::conformance::PROGRAMS).copied();
    programs.filter_map(|program| Some((program, program.sources.wgsl?))).collect()
}

/// The names after the `HAS_UNIFORM_` that `program`'s WGSL, `wgsl`,
/// tests once its properties are written out.
fn uniforms(program: &Program, wgsl: &Wgsl) -> BTreeSet<String> {
    const PREFIX: &str = "HAS_UNIFORM_";
    let mut names = BTreeSet::new();
    let [vertex, fragment] = wgsl::expand(program, wgsl).expect("expands");
    for text in [wgsl.prelude, &vertex, &fragment] {
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
        assert_eq!(uniforms(program, wgsl), declared, "{}", program.name);
    }
}

#[test]
fn every_program_compiles() {
    let device = Device::new(None).expect("device");
    let device = device.device();
    let index_table = IndexTable::new(device);
    let mut failed = Vec::new();
    let parameters = [false, true].map(|overdraw| ProgramParameters { pixel_ratio: 1.0, overdraw });
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
                            let text = [PRELUDE, wgsl.prelude, wgsl.vertex, wgsl.fragment];
                            assert_eq!(
                                compiled.index_group().is_some(),
                                text.into_iter().any(|text| text.contains("drawIndex")),
                                "{} binds drawIndex in a group of its own",
                                program.name
                            );
                            assert!(
                                compiled.vertex_storage_buffers() <= VERTEX_STAGE_STORAGE_BUFFERS,
                                "{}'s vertex stage reads more than VERTEX_STAGE_STORAGE_BUFFERS storage buffers",
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
        stencil: StencilState::DISABLED,
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
                .enumerate()
                .map(|(index, attribute)| {
                    let offset = stride;
                    stride += attribute.format.stride() as u32;
                    VertexAttribute { id: index as u32, format: attribute.format, offset }
                })
                .collect();
            VertexBufferLayout { stride, step, attributes }
        })
        .collect()
}

/// No source declares a block of the engine's or its program's: the
/// backend declares them, from their layouts.
#[test]
fn no_source_declares_a_block() {
    let engine = &ENGINE_BUFFERS;
    for (program, source) in programs() {
        let text = [PRELUDE, source.prelude, source.vertex, source.fragment].concat();
        for block in program.buffers.iter().chain(engine).filter_map(ProgramBuffer::block) {
            assert!(!text.contains(&format!("struct {} {{", block.name)), "{} declares {}", program.name, block.name);
        }
    }
}

/// No source declares a buffer or a texture: the backend declares the
/// engine's buffers and its program's buffers and textures, by their names.
#[test]
fn no_source_declares_a_buffer_or_a_texture() {
    for (program, source) in programs() {
        for text in [source.prelude, source.vertex, source.fragment] {
            assert!(!text.contains("@group("), "{} declares a binding", program.name);
        }
    }
}

/// WGSL lays out every block's struct, padded to its buffer's stride, as
/// its Rust struct is laid out.
#[test]
fn every_block_has_its_layout() {
    use wgpu::naga;

    let parameters = ProgramParameters { pixel_ratio: 1.0, overdraw: false };
    let engine = &ENGINE_BUFFERS;
    for (program, _) in programs() {
        for variant in [ProjectionVariant::Mercator, ProjectionVariant::Globe] {
            let key = program.variant_key(&parameters, program.properties, variant);
            for stage in stage_sources(&key).expect("WGSL").stages {
                let module = naga::front::wgsl::parse_str(&stage).expect("WGSL that parses");
                let mut layouter = naga::proc::Layouter::default();
                layouter.update(module.to_ctx()).expect("a layout");
                for buffer in program.buffers.iter().chain(engine) {
                    let Some(block) = buffer.block() else { continue };
                    let (handle, ty) =
                        module.types.iter().find(|(_, ty)| ty.name.as_deref() == Some(block.name)).expect("the struct");
                    let naga::TypeInner::Struct { members, span } = &ty.inner else {
                        panic!("{} isn't a struct", block.name)
                    };
                    assert_eq!(*span as usize, buffer.size(), "{}: {}", program.name, block.name);
                    assert_eq!(layouter[handle].size as usize, buffer.size(), "{}: {}", program.name, block.name);
                    for (member, declared) in members.iter().zip(block.members) {
                        assert_eq!(member.name.as_deref(), Some(declared.name), "{}", block.name);
                        assert_eq!(member.offset as usize, declared.offset, "{}.{}", block.name, declared.name);
                    }
                }
            }
        }
    }
}
