//! Parses and validates every program's GLSL with naga, as the backends
//! compose it: `#version 450`, the `ProgramParameters` defines, a
//! `HAS_UNIFORM_u_<name>` define per property passed as a uniform,
//! `PROJECTION_GLOBE` for the globe variant, then the engine's declarations,
//! the stage's prelude, the program's declarations and the stage's source.
//! Also checks that the buffers,
//! textures, samplers and vertex inputs the GLSL declares are the ones the
//! descriptor set layouts and the program's declaration describe, that
//! the buffers have the sizes of what the renderer binds there, and that the
//! stages of a shader agree on what passes between them, in both projection
//! variants.

use std::collections::{BTreeMap, BTreeSet};
use std::mem::size_of;

use mln_builtins::shaders::{every_program, projection_variants, stands};
use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::shaders::bindings::{
    FIRST_UNIFORM_ID, FRAGMENT_DRAW_ID, MAX_ATTRIBUTES, PAINT_PARAMS_ID, PROJECTION_ID, VERTEX_DRAW_ID,
};
use mln_gfx::shaders::layer_ubo::{GlobalPaintParamsUBO, ProjectionUBO};
use mln_gfx::shaders::program::{Attribute, BufferKind, PAINT_PARAMS, Program, Variants};
use mln_gfx::shaders::source::GlslSource;
use naga::{AddressSpace, ArraySize, Binding, Interpolation, Module, ScalarKind, StorageAccess, TypeInner};

use crate::front_end::parse;
use crate::layout::{
    DRAWABLE_UNIFORM_COUNT, DRAWABLE_VERTEX_COUNT, DescriptorSetType, GLOBAL_UNIFORM_COUNT, IMAGE_SAMPLER_START,
    IMAGE_TEXTURE_COUNT, LAYER_STORAGE_COUNT, LAYER_UNIFORM_COUNT, UBO_INDEX_PUSH_CONSTANT_SIZE,
    drawable_uniform_binding, drawable_vertex_binding, global_uniform_binding, image_sampler_binding,
    image_texture_binding, layer_storage_binding, layer_uniform_binding,
};
use crate::variants::variants;
use crate::{Stage, compose, prelude};

/// Every program the renderer draws with that has GLSL, with its GLSL.
fn programs() -> impl Iterator<Item = (&'static Program, &'static GlslSource)> {
    every_program().filter_map(|program| Some((program, program.sources.glsl?)))
}

const STAGES: [Stage; 2] = [Stage::Vertex, Stage::Fragment];

/// A variant's defines read a property from its uniform only where the
/// program declares it.
#[test]
fn every_program_declares_the_properties_its_glsl_reads_from_uniforms() {
    const PREFIX: &str = "HAS_UNIFORM_u_";
    for (program, source) in programs() {
        let mut tested = BTreeSet::new();
        for text in [source.vertex, source.fragment] {
            for (start, _) in text.match_indices(PREFIX) {
                let rest = &text[start + PREFIX.len()..];
                let end = rest.find(|c: char| !c.is_ascii_alphanumeric() && c != '_').unwrap_or(rest.len());
                tested.insert(&rest[..end]);
            }
        }
        let declared: BTreeSet<&str> = program.properties.iter().map(|property| property.name).collect();
        assert_eq!(tested, declared, "{}", program.name);
    }
}

/// The module of a stage of `source` with `defines`, unvalidated.
fn module(program: &Program, source: &GlslSource, stage: Stage, defines: &str) -> Module {
    compose(source, program, stage, defines)
        .and_then(|composed| parse(stage, &composed))
        .unwrap_or_else(|error| panic!("{} ({stage:?}):\n{error}", program.name))
}

// Every stage parses and validates, with every property per vertex, every
// property as a uniform, and each property alone as a uniform, in both
// projections.
#[test]
fn every_stage_validates() {
    let mut failures = Vec::new();
    let mut validated = 0;
    for (program, source) in programs() {
        for variant in variants(program, projection_variants(program), true) {
            for stage in STAGES {
                match compose(source, program, stage, &variant.defines)
                    .and_then(|composed| crate::Module::parse(stage, &composed))
                {
                    Ok(_) => validated += 1,
                    Err(error) => failures.push(format!("{} ({}, {stage:?}):\n{error}", program.name, variant.name)),
                }
            }
        }
    }
    assert!(failures.is_empty(), "{} of {} stages failed:\n{}", failures.len(), validated + failures.len(), {
        failures.join("\n\n")
    });
}

/// A buffer the renderer binds for a shader: the set it is in, with its id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Buffer {
    Global(usize),
    LayerStorage(usize),
    LayerUniform(usize),
    DrawableUniform(usize),
    /// A vertex buffer that the shader reads as a storage buffer.
    DrawableVertex(usize),
}

impl Buffer {
    /// The set, the binding in it, and whether it is a storage buffer.
    fn slot(self) -> (DescriptorSetType, u32, bool) {
        match self {
            Buffer::Global(id) => (DescriptorSetType::Global, global_uniform_binding(id).unwrap(), false),
            Buffer::LayerStorage(id) => (DescriptorSetType::Layer, layer_storage_binding(id).unwrap(), true),
            Buffer::LayerUniform(id) => (DescriptorSetType::Layer, layer_uniform_binding(id).unwrap(), false),
            Buffer::DrawableUniform(id) => {
                (DescriptorSetType::DrawableUniform, drawable_uniform_binding(id).unwrap(), false)
            }
            Buffer::DrawableVertex(id) => {
                (DescriptorSetType::DrawableUniform, drawable_vertex_binding(id).unwrap(), true)
            }
        }
    }
}

/// The buffers of each shader, as its program declares them, with the size
/// of what the renderer binds: a uniform block, or an element of a storage
/// buffer (the union of the layer's blocks where it has several, a word of a
/// vertex buffer).
fn buffers(program: &Program) -> Vec<(Buffer, usize)> {
    let paint_params = program.paint_params.map(|_| &PAINT_PARAMS);
    let buffers = paint_params.into_iter().chain(program.projection_buffer()).chain(program.buffers);
    buffers
        .map(|buffer| {
            let at = match buffer.kind {
                BufferKind::Uniform(_) if buffer.id == PAINT_PARAMS.id => Buffer::Global(buffer.id),
                BufferKind::Uniform(_) => Buffer::LayerUniform(buffer.id),
                BufferKind::DrawUniform(_) => Buffer::DrawableUniform(buffer.id),
                BufferKind::Storage { .. } => Buffer::LayerStorage(buffer.id),
                BufferKind::Vertices(_) => Buffer::DrawableVertex(buffer.id),
            };
            let size = buffer.stride().or(buffer.block().map(|block| block.size)).unwrap_or(size_of::<u32>());
            (at, size)
        })
        .collect()
}

/// A resource a module declares in a descriptor set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Resource {
    /// A uniform buffer, with the size of its struct.
    Uniform(u32),
    /// A read-only storage buffer holding an array, with the array's stride.
    Storage(u32),
    Texture,
    Sampler,
}

/// What a module declares for its pipeline layout.
struct Resources {
    /// The resources of the descriptor sets, by (set, binding).
    bindings: BTreeMap<(u32, u32), Resource>,
    /// The size of the push-constant block, if there is one.
    push_constants: Option<u32>,
}

/// The resources of `module`, or what the descriptor sets can't hold.
fn resources(module: &Module) -> Result<Resources, String> {
    let mut bindings = BTreeMap::new();
    let mut push_constants = None;
    for (_, global) in module.global_variables.iter() {
        let name = global.name.as_deref().unwrap_or("?");
        let inner = &module.types[global.ty].inner;
        let resource = match (global.space, inner) {
            (AddressSpace::Private | AddressSpace::Function, _) => continue,
            (AddressSpace::Immediate, TypeInner::Struct { span, .. }) => {
                if push_constants.replace(*span).is_some() {
                    return Err(format!("{name}: a second push-constant block"));
                }
                continue;
            }
            (AddressSpace::Uniform, TypeInner::Struct { span, .. }) => Resource::Uniform(*span),
            (AddressSpace::Storage { access }, TypeInner::Struct { members, .. }) => {
                if access != StorageAccess::LOAD {
                    return Err(format!("{name}: a storage buffer that isn't read-only"));
                }
                let [member] = members.as_slice() else {
                    return Err(format!("{name}: a storage buffer of {} members", members.len()));
                };
                match module.types[member.ty].inner {
                    TypeInner::Array { size: ArraySize::Dynamic, stride, .. } if member.offset == 0 => {
                        Resource::Storage(stride)
                    }
                    _ => return Err(format!("{name}: a storage buffer that isn't one unsized array")),
                }
            }
            (AddressSpace::Handle, TypeInner::Image { .. }) => Resource::Texture,
            (AddressSpace::Handle, TypeInner::Sampler { .. }) => Resource::Sampler,
            (space, _) => return Err(format!("{name}: unexpected in {space:?}")),
        };
        let Some(binding) = &global.binding else {
            return Err(format!("{name}: no set and binding"));
        };
        if bindings.insert((binding.group, binding.binding), resource).is_some() {
            return Err(format!("{name}: set {} binding {} is declared twice", binding.group, binding.binding));
        }
    }
    Ok(Resources { bindings, push_constants })
}

/// What is wrong with a resource of a module, checked against the bindings
/// the descriptor set layouts have.
fn check_layout(set: u32, binding: u32, resource: Resource) -> Result<(), String> {
    let binding = binding as usize;
    let in_layout = match (set, resource) {
        (0, Resource::Uniform(_)) => binding < GLOBAL_UNIFORM_COUNT,
        (1, Resource::Storage(_)) => binding < LAYER_STORAGE_COUNT,
        (1, Resource::Uniform(_)) => {
            (LAYER_STORAGE_COUNT..LAYER_STORAGE_COUNT + LAYER_UNIFORM_COUNT).contains(&binding)
        }
        (2, Resource::Uniform(_)) => binding < DRAWABLE_UNIFORM_COUNT,
        (2, Resource::Storage(_)) => {
            (DRAWABLE_UNIFORM_COUNT..DRAWABLE_UNIFORM_COUNT + DRAWABLE_VERTEX_COUNT).contains(&binding)
        }
        (3, Resource::Texture) => binding < IMAGE_TEXTURE_COUNT,
        (3, Resource::Sampler) => (IMAGE_SAMPLER_START..IMAGE_SAMPLER_START + IMAGE_TEXTURE_COUNT).contains(&binding),
        _ => false,
    };
    if in_layout { Ok(()) } else { Err(format!("set {set} binding {binding}: no {resource:?} in the layout")) }
}

// Every resource a stage declares has a binding of its kind in the
// descriptor set layouts, the global set holds the paint parameters alone,
// the stages declare the program's textures between them, each with its
// sampler but those whose texels they read, and the push constants are the
// general pipeline
// layout's index or, for the clipping mask, its matrices.
#[test]
fn resources_are_in_the_descriptor_set_layouts() {
    assert_eq!(DescriptorSetType::Global.set_index(), 0);
    assert_eq!(DescriptorSetType::Layer.set_index(), 1);
    assert_eq!(DescriptorSetType::DrawableUniform.set_index(), 2);
    assert_eq!(DescriptorSetType::DrawableImage.set_index(), 3);

    let mut failures = Vec::new();
    for (program, source) in programs() {
        let table: BTreeSet<u32> = program
            .textures
            .iter()
            .enumerate()
            .filter(|(_, texture)| !texture.is_unused())
            .map(|(slot, _)| image_texture_binding(slot))
            .collect();
        for variant in variants(program, projection_variants(program), false) {
            let mut declared = BTreeSet::new();
            for stage in STAGES {
                let mut fail = |message: String| {
                    failures.push(format!("{} ({}, {stage:?}): {message}", program.name, variant.name));
                };
                let Resources { bindings, push_constants } =
                    match resources(&module(program, source, stage, &variant.defines)) {
                        Ok(resources) => resources,
                        Err(message) => {
                            fail(message);
                            continue;
                        }
                    };
                for (&(set, binding), &resource) in &bindings {
                    if let Err(message) = check_layout(set, binding, resource) {
                        fail(message);
                    }
                    if set == 0 && global_uniform_binding(PAINT_PARAMS_ID) != Some(binding) {
                        fail(format!("set 0 binding {binding}: not the paint parameters"));
                    }
                }

                let image_set = DescriptorSetType::DrawableImage.set_index();
                let bindings_of = |kind: Resource| -> BTreeSet<u32> {
                    bindings
                        .iter()
                        .filter(|&(&(set, _), &resource)| set == image_set && resource == kind)
                        .map(|(&(_, binding), _)| binding)
                        .collect()
                };
                let (textures, samplers) = (bindings_of(Resource::Texture), bindings_of(Resource::Sampler));
                if !textures.is_subset(&table) {
                    fail(format!("textures at {textures:?}, the table has {table:?}"));
                }
                let sampled: BTreeSet<u32> = textures
                    .iter()
                    .filter(|&&binding| program.textures[binding as usize].sampled())
                    .map(|&binding| image_sampler_binding(binding as usize))
                    .collect();
                if samplers != sampled {
                    fail(format!("samplers at {samplers:?}, not {sampled:?}"));
                }
                declared.extend(textures);

                let expected_push_constants = push_constants.map(|_| UBO_INDEX_PUSH_CONSTANT_SIZE);
                if push_constants != expected_push_constants {
                    fail(format!("push constants of {push_constants:?} bytes, not {expected_push_constants:?}"));
                }
            }
            // A variant that doesn't stand on terrain reads none of the
            // ground's depth.
            let depth = program.textures.iter().position(|texture| texture.name == "ground_depth");
            let ground: Vec<u32> = depth.into_iter().map(image_texture_binding).collect();
            let expected: BTreeSet<u32> = if variant.projection.stands() || !stands(program) {
                table.clone()
            } else {
                table.iter().copied().filter(|binding| !ground.contains(binding)).collect()
            };
            if declared != expected {
                failures.push(format!(
                    "{} ({}): textures at {declared:?}, the table has {table:?}",
                    program.name, variant.name
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// The stages that the program says read each of its buffers and textures, and
// the paint parameters, read it in some variant: a stage declares only what
// it reads.
#[test]
fn every_stage_reads_what_it_declares() {
    let mut failures = Vec::new();
    for (program, source) in programs() {
        for stage in STAGES {
            let (mut declared, mut read) = (BTreeSet::new(), BTreeSet::new());
            for variant in variants(program, projection_variants(program), true) {
                let module = compose(source, program, stage, &variant.defines)
                    .and_then(|composed| crate::Module::parse(stage, &composed))
                    .unwrap_or_else(|error| panic!("{} ({stage:?}): {error}", program.name));
                let resources = resources(&module.module).unwrap_or_else(|message| panic!("{message}"));
                declared.extend(resources.bindings.into_keys());
                read.extend(module.resources().unwrap().iter().map(|resource| (resource.set, resource.binding)));
            }
            let unread: Vec<(u32, u32)> = declared.difference(&read).copied().collect();
            if !unread.is_empty() {
                failures.push(format!("{} ({stage:?}): declares {unread:?} and reads none of them", program.name));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// Every buffer a stage declares is one the renderer binds for the shader, at
// the binding its id has, as a uniform or a storage buffer as the id is, and
// with the size of the renderer's struct: the GLSL blocks are std140, as the
// renderer's `#[repr(C)]` structs are laid out. Every buffer the renderer
// binds is declared by a stage.
#[test]
fn buffers_are_the_renderers() {
    assert_eq!(size_of::<ProjectionUBO>(), 208);
    assert_eq!(size_of::<GlobalPaintParamsUBO>(), 48);

    let mut failures = Vec::new();
    for (program, source) in programs() {
        let expected: BTreeMap<(u32, u32), (Buffer, Resource)> = buffers(program)
            .into_iter()
            .map(|(buffer, size)| {
                let (set, binding, storage) = buffer.slot();
                let size = size as u32;
                let resource = if storage { Resource::Storage(size) } else { Resource::Uniform(size) };
                ((set.set_index(), binding), (buffer, resource))
            })
            .collect();
        assert_eq!(expected.len(), buffers(program).len(), "{}: two buffers at a binding", program.name);

        let mut declared = BTreeSet::new();
        for variant in variants(program, projection_variants(program), false) {
            for stage in STAGES {
                let at = format!("{} ({}, {stage:?})", program.name, variant.name);
                let resources = resources(&module(program, source, stage, &variant.defines))
                    .unwrap_or_else(|message| panic!("{at}: {message}"));
                for (slot, resource) in resources.bindings {
                    if !matches!(resource, Resource::Uniform(_) | Resource::Storage(_)) {
                        continue;
                    }
                    declared.insert(slot);
                    match expected.get(&slot) {
                        Some(&(_, expected)) if expected == resource => {}
                        Some((buffer, expected)) => {
                            failures.push(format!("{at}: {buffer:?} is {resource:?}, the renderer binds {expected:?}"));
                        }
                        None => failures.push(format!("{at}: {resource:?} at {slot:?} is no buffer of the shader")),
                    }
                }
            }
        }
        for (slot, (buffer, _)) in &expected {
            if !declared.contains(slot) {
                failures.push(format!("{}: no stage declares {buffer:?}", program.name));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The scalar kind and component count of the GLSL type that reads
/// `data_type`: the integer formats as integers of their signedness.
fn input_type(data_type: AttributeDataType) -> (ScalarKind, u32) {
    match data_type {
        AttributeDataType::Short2 => (ScalarKind::Sint, 2),
        AttributeDataType::Short4 => (ScalarKind::Sint, 4),
        AttributeDataType::UShort2 => (ScalarKind::Uint, 2),
        AttributeDataType::UByte4 | AttributeDataType::UShort4 => (ScalarKind::Uint, 4),
        AttributeDataType::Float => (ScalarKind::Float, 1),
        AttributeDataType::Float2 => (ScalarKind::Float, 2),
        AttributeDataType::Float3 => (ScalarKind::Float, 3),
        AttributeDataType::Float4 => (ScalarKind::Float, 4),
        other => panic!("no input type for {other:?}"),
    }
}

/// What passes through a location of a stage's interface: its scalar kind
/// and component count and, between the stages, whether it is flat.
type Varying = (ScalarKind, u32, bool);

fn varying(module: &Module, ty: naga::Handle<naga::Type>, binding: Option<&Binding>) -> Option<(u32, Varying)> {
    let Some(&Binding::Location { location, interpolation, .. }) = binding else { return None };
    let (kind, components) = match module.types[ty].inner {
        TypeInner::Scalar(scalar) => (scalar.kind, 1),
        TypeInner::Vector { size, scalar } => (scalar.kind, size as u32),
        ref other => panic!("location {location}: {other:?}"),
    };
    Some((location, (kind, components, interpolation == Some(Interpolation::Flat))))
}

/// The inputs of a stage by location.
fn stage_inputs(module: &Module) -> BTreeMap<u32, Varying> {
    let [entry_point] = module.entry_points.as_slice() else {
        panic!("{} entry points", module.entry_points.len());
    };
    let mut inputs = BTreeMap::new();
    for argument in &entry_point.function.arguments {
        if let Some((location, input)) = varying(module, argument.ty, argument.binding.as_ref()) {
            assert!(inputs.insert(location, input).is_none(), "location {location} is declared twice");
        }
    }
    inputs
}

/// The outputs of a stage by location.
fn stage_outputs(module: &Module) -> BTreeMap<u32, Varying> {
    let [entry_point] = module.entry_points.as_slice() else {
        panic!("{} entry points", module.entry_points.len());
    };
    let mut outputs = BTreeMap::new();
    let Some(result) = &entry_point.function.result else { return outputs };
    let members: Vec<(naga::Handle<naga::Type>, Option<&Binding>)> = match &module.types[result.ty].inner {
        TypeInner::Struct { members, .. } => members.iter().map(|m| (m.ty, m.binding.as_ref())).collect(),
        _ => vec![(result.ty, result.binding.as_ref())],
    };
    for (ty, binding) in members {
        if let Some((location, output)) = varying(module, ty, binding) {
            assert!(outputs.insert(location, output).is_none(), "location {location} is declared twice");
        }
    }
    outputs
}

// The inputs of a vertex stage are the program's vertex and instance
// attributes, at their locations (`Program::location`) and in the types
// that read their formats, or, for a property's attribute, which may come as
// one value instead of two, in types of as many components or more. With a
// property as a uniform, the stage reads a subset of them.
#[test]
fn vertex_inputs_are_the_programs_attributes() {
    let mut failures = Vec::new();
    for (program, source) in programs() {
        let reads = |attribute: &Attribute, (kind, components): (ScalarKind, u32)| {
            let (declared_kind, declared) = input_type(attribute.format);
            let property = program.properties.iter().any(|property| property.name == attribute.name);
            declared_kind == kind && (declared == components || property && declared < components)
        };
        let locations: BTreeSet<u32> = program.locations().map(|(location, ..)| location as u32).collect();
        assert_eq!(locations.len(), program.locations().count(), "{}: attribute locations repeat", program.name);
        for variant in variants(program, projection_variants(program), true) {
            let unread =
                |attribute: &Attribute| attribute.variants == Variants::Standing && !variant.projection.stands();
            let table: BTreeMap<u32, &Attribute> = program
                .locations()
                .filter(|&(_, _, attribute)| !unread(attribute))
                .map(|(location, _, attribute)| (location as u32, attribute))
                .collect();
            let inputs: BTreeMap<u32, (ScalarKind, u32)> =
                stage_inputs(&module(program, source, Stage::Vertex, &variant.defines))
                    .into_iter()
                    .map(|(location, (kind, components, _))| (location, (kind, components)))
                    .collect();
            let all = !variant.per_vertex || inputs.keys().eq(table.keys());
            let matches = all
                && inputs.iter().all(|(location, &input)| table.get(location).is_some_and(|attr| reads(attr, input)));
            if !matches {
                failures
                    .push(format!("{} ({}): inputs {inputs:?}, the program has {table:?}", program.name, variant.name));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// A fill extrusion wall reads its next outline vertex from the buffer of its
// outline vertex, a vertex further on: the two have the same formats.
#[test]
fn a_walls_next_outline_vertex_has_the_formats_of_its_outline_vertex() {
    let instanced: Vec<_> = programs()
        .filter(|(program, _)| program.name.starts_with("FillExtrusion") && !program.instance_attributes.is_empty())
        .collect();
    assert_eq!(instanced.len(), 2);
    for (program, _) in instanced {
        let format = |name: &str| {
            let mut found = program.instance_attributes.iter().filter(|attr| attr.name == name);
            let attr = found.next().unwrap_or_else(|| panic!("{}: no instance attribute {name}", program.name));
            assert!(found.next().is_none());
            attr.format
        };
        for (name, next) in [("outline_pos", "next_outline_pos"), ("decimals_ed", "next_decimals_ed")] {
            assert_eq!(format(name), format(next), "{}", program.name);
        }
    }
}

// compiled with the same defines writes there, flat where the other is, and
// the fragment stage writes one color.
#[test]
fn stages_agree_on_what_passes_between_them() {
    let mut failures = Vec::new();
    for (program, source) in programs() {
        for variant in variants(program, projection_variants(program), true) {
            let at = format!("{} ({})", program.name, variant.name);
            let outputs = stage_outputs(&module(program, source, Stage::Vertex, &variant.defines));
            let fragment = module(program, source, Stage::Fragment, &variant.defines);
            for (location, input) in stage_inputs(&fragment) {
                let output = outputs.get(&location);
                if output != Some(&input) {
                    failures.push(format!("{at}: location {location} is read as {input:?}, written as {output:?}"));
                }
            }
            let colors = stage_outputs(&fragment);
            if colors != BTreeMap::from([(0, (ScalarKind::Float, 4, false))]) {
                failures.push(format!("{at}: the fragment stage writes {colors:?}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// The bindings the GLSL spells out as numbers are the ones the packed ids
// have: `layerSSBOStartId`, `layerUBOStartId` and `drawableUBOStartId` with
// the offsets the shader preludes add, and `samplerBindingStartId`.
#[test]
fn bindings_follow_from_the_packed_ids() {
    use mln_builtins::layers::background::programs::BACKGROUND;
    use mln_builtins::layers::circle::programs::CIRCLE;
    use mln_builtins::layers::color_relief::programs::COLOR_RELIEF;
    use mln_builtins::layers::fill::programs::{FILL, FILL_OUTLINE_PULLED, FILL_PATTERN};
    use mln_builtins::layers::fill_extrusion::programs::{
        FILL_EXTRUSION, FILL_EXTRUSION_PATTERN, FILL_EXTRUSION_PULLED,
    };
    use mln_builtins::layers::heatmap::programs::{HEATMAP, HEATMAP_TEXTURE};
    use mln_builtins::layers::hillshade::programs::{HILLSHADE, HILLSHADE_PREPARE};
    use mln_builtins::layers::line::programs::LINE;
    use mln_builtins::layers::raster::programs::RASTER;
    use mln_builtins::layers::symbol::programs::SYMBOL_SDF;
    use mln_engine::shaders::programs::{ATMOSPHERE, SKY};

    assert_eq!(global_uniform_binding(PAINT_PARAMS_ID), Some(0));

    // idDrawableReservedVertexOnlyUBO, idDrawableReservedFragmentOnlyUBO,
    // idProjectionUBO and drawableReservedUBOCount, from layerSSBOStartId.
    assert_eq!(layer_storage_binding(VERTEX_DRAW_ID), Some(0));
    assert_eq!(layer_storage_binding(FRAGMENT_DRAW_ID), Some(1));
    assert_eq!(layer_storage_binding(PROJECTION_ID), Some(2));
    assert_eq!(layer_storage_binding(FIRST_UNIFORM_ID), Some(3));
    for id in [
        FILL_PATTERN.buffer_id("tilePropsVector"),
        FILL_EXTRUSION_PATTERN.buffer_id("tilePropsVector"),
        COLOR_RELIEF.buffer_id("tilePropsVector"),
    ] {
        assert_eq!(layer_storage_binding(id), Some(3));
    }
    assert_eq!(LAYER_STORAGE_COUNT, 4);

    // layerUBOStartId.
    for id in [
        BACKGROUND.buffer_id("props"),
        CIRCLE.buffer_id("props"),
        HEATMAP.buffer_id("props"),
        HEATMAP_TEXTURE.buffer_id("ubo"),
        HILLSHADE.buffer_id("props"),
        LINE.buffer_id("props"),
        RASTER.buffer_id("props"),
        SYMBOL_SDF.buffer_id("props"),
        SKY.buffer_id("sky"),
        ATMOSPHERE.buffer_id("atmosphere"),
    ] {
        assert_eq!(layer_uniform_binding(id), Some(4));
    }
    // layerUBOStartId + 1: the layers with a tile properties storage buffer.
    for id in [FILL.buffer_id("props"), FILL_EXTRUSION.buffer_id("props"), COLOR_RELIEF.buffer_id("props")] {
        assert_eq!(layer_uniform_binding(id), Some(5));
    }

    // drawableUBOStartId and its offsets.
    assert_eq!(drawable_uniform_binding(PROJECTION_ID), Some(2));
    assert_eq!(drawable_uniform_binding(HILLSHADE_PREPARE.buffer_id("drawable")), Some(0));
    assert_eq!(drawable_uniform_binding(HILLSHADE_PREPARE.buffer_id("tileProps")), Some(3));
    // The pulled shaders' vertex buffers, from drawableUBOStartId + 5.
    assert_eq!(DRAWABLE_UNIFORM_COUNT, 5);
    assert_eq!(drawable_vertex_binding(FILL_EXTRUSION_PULLED.buffer_id("wallVertices")), Some(5));
    assert_eq!(drawable_vertex_binding(FILL_EXTRUSION_PULLED.buffer_id("wallPaint")), Some(6));
    assert_eq!(drawable_vertex_binding(FILL_OUTLINE_PULLED.buffer_id("outlineVertices")), Some(5));
    assert_eq!(drawable_vertex_binding(FILL_OUTLINE_PULLED.buffer_id("outlineEnds")), Some(6));
    assert_eq!(drawable_vertex_binding(FILL_OUTLINE_PULLED.buffer_id("outlinePaint")), Some(7));

    // samplerBindingStartId.
    assert_eq!(IMAGE_SAMPLER_START, 3);
    assert_eq!(image_texture_binding(0), 0);
    assert_eq!(image_sampler_binding(0), 3);
}

// No program has more attributes and textures than a shader can have. Only
// the fill extrusion walls and the instanced fill outlines have instance
// attributes.
#[test]
fn the_programs_fit_a_shader() {
    for (program, _) in programs() {
        assert!(program.locations().count() <= MAX_ATTRIBUTES, "{}", program.name);
        assert!(program.textures.len() <= IMAGE_TEXTURE_COUNT, "{}", program.name);
        let instanced = matches!(
            program.name,
            "FillExtrusionInstancedShader"
                | "FillExtrusionPatternInstancedShader"
                | "FillOutlineInstancedShader"
                | "FillOutlinePatternInstancedShader"
        );
        assert_eq!(!program.instance_attributes.is_empty(), instanced, "{}", program.name);
    }
}

// A program's attributes and textures have names of their own, and each of
// its properties has an attribute of its name, which a layer binds its values
// to, unless the program has no attributes and reads the values itself.
#[test]
fn every_property_binds_an_attribute_of_its_name() {
    for (program, _) in programs() {
        let attributes: Vec<&str> = program.locations().map(|(_, _, attribute)| attribute.name).collect();
        let distinct: BTreeSet<&str> = attributes.iter().copied().collect();
        assert_eq!(distinct.len(), attributes.len(), "{}: two attributes have a name", program.name);
        let textures: Vec<&str> =
            program.textures.iter().filter(|texture| !texture.is_unused()).map(|texture| texture.name).collect();
        let distinct: BTreeSet<&str> = textures.iter().copied().collect();
        assert_eq!(distinct.len(), textures.len(), "{}: two textures have a name", program.name);
        if !attributes.is_empty() {
            for property in program.properties {
                assert!(attributes.contains(&property.name), "{}: no attribute {}", program.name, property.name);
            }
        }
    }
}

// Clip space is y-up: no vertex stage flips Y, which the back end for a
// y-down clip space does.
#[test]
fn no_stage_flips_y() {
    for (program, source) in programs() {
        let text: String = source.vertex.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(!text.contains("gl_Position.y*=-1"), "{}", program.name);
        assert!(!text.contains("applySurfaceTransform"), "{}", program.name);
    }
    assert!(!prelude::PRELUDE_VERTEX.contains("applySurfaceTransform"));
}
