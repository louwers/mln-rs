//! Parses and validates every program's GLSL with naga, as the backends
//! compose it: `#version 450`, the `ProgramParameters` defines, a
//! `HAS_UNIFORM_<name>` define per property passed as a uniform,
//! `PROJECTION_GLOBE` for the globe variant, then the stage's prelude, the
//! shader's prelude and the stage's source. Also checks that the buffers,
//! textures, samplers and vertex inputs the GLSL declares are the ones the
//! descriptor set layouts and the program's declaration describe, that the
//! buffers have the sizes the program declares, and that the stages of a
//! shader agree on what passes between them, in both projection variants.

use std::collections::{BTreeMap, BTreeSet};

use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::shaders::bindings::{
    FIRST_UNIFORM_ID, FIRST_VERTEX_BUFFER_ID, FRAGMENT_STORAGE_ID, MAX_TEXTURES, PROJECTION_BUFFER_ID,
    VERTEX_STORAGE_ID, VIEW_BUFFER_ID,
};
use mln_gfx::shaders::program::{Attribute, BufferKind, ENGINE_BUFFERS, Program, ProgramBuffer};
use mln_gfx::shaders::source::glsl::{self, Glsl};
use naga::{AddressSpace, ArraySize, Binding, Interpolation, Module, ScalarKind, StorageAccess, TypeInner};

use crate::front_end::parse;
use crate::layout::{
    DRAW_INDEX_PUSH_CONSTANT_SIZE, DRAWABLE_UNIFORM_COUNT, DRAWABLE_VERTEX_COUNT, DescriptorSetType,
    GLOBAL_UNIFORM_COUNT, IMAGE_SAMPLER_START, IMAGE_TEXTURE_COUNT, LAYER_STORAGE_COUNT, LAYER_UNIFORM_COUNT,
    drawable_uniform_binding, drawable_vertex_binding, global_uniform_binding, image_sampler_binding,
    image_texture_binding, layer_storage_binding, layer_uniform_binding,
};
use crate::program::declaration;
use crate::variants::variants;
use crate::{Stage, compose, sources};

/// Each program of the built-in layer types with GLSL.
fn programs() -> Vec<(&'static Program, &'static Glsl)> {
    let programs = mln_layers::PROGRAMS.iter();
    programs.filter_map(|&program| Some((program, program.sources.glsl?))).collect()
}

const STAGES: [Stage; 2] = [Stage::Vertex, Stage::Fragment];

/// A variant's defines read a property from its uniform only where the
/// program declares it.
#[test]
fn every_program_declares_the_properties_its_glsl_reads_from_uniforms() {
    const PREFIX: &str = "HAS_UNIFORM_";
    for (program, source) in programs() {
        let mut tested = BTreeSet::new();
        let [vertex, fragment] = STAGES.map(|stage| glsl::expand(program, source, stage.into()).expect("expands"));
        for text in [source.prelude, &vertex, &fragment] {
            for (start, _) in text.match_indices(PREFIX) {
                let rest = &text[start + PREFIX.len()..];
                let end = rest.find(|c: char| !c.is_ascii_alphanumeric() && c != '_').unwrap_or(rest.len());
                tested.insert(rest[..end].to_owned());
            }
        }
        let declared: BTreeSet<String> = program.properties.iter().map(|property| property.name.to_owned()).collect();
        assert_eq!(tested, declared, "{}", program.name);
    }
}

/// The module of a stage of `source` with `defines`, unvalidated.
fn module(program: &Program, source: &Glsl, stage: Stage, defines: &str) -> Module {
    parse(stage, &compose(program, source, stage, defines).expect("expands"))
        .unwrap_or_else(|error| panic!("{} ({stage:?}):\n{error}", program.name))
}

// Every stage parses and validates, with every property per vertex, every
// property as a uniform, each property alone as a uniform, and with the
// overdraw inspector, in both projections.
#[test]
fn every_stage_validates() {
    let mut failures = Vec::new();
    let mut validated = 0;
    for (program, source) in programs() {
        for variant in variants(program, true) {
            for stage in STAGES {
                match crate::Module::parse(stage, &compose(program, source, stage, &variant.defines).expect("expands"))
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

/// The buffer of `program`, one of its own or of the engine's, that
/// `compose` declares at `binding` of `set`.
fn program_buffer(program: &Program, set: u32, binding: u32) -> Option<ProgramBuffer> {
    let at = format!("set = {set}, binding = {binding})");
    let declared_at = |buffer: &&ProgramBuffer| declaration(buffer).is_ok_and(|text| text.contains(&at));
    program.buffers.iter().chain(&ENGINE_BUFFERS).find(declared_at).copied()
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
// the textures of a fragment stage are the ones of the texture table, each
// with its sampler, and the push constants are the general pipeline layout's
// index or, for the clipping mask, its matrices.
#[test]
fn resources_are_in_the_descriptor_set_layouts() {
    assert_eq!(DescriptorSetType::Global.set_index(), 0);
    assert_eq!(DescriptorSetType::Layer.set_index(), 1);
    assert_eq!(DescriptorSetType::DrawableUniform.set_index(), 2);
    assert_eq!(DescriptorSetType::DrawableImage.set_index(), 3);

    let mut failures = Vec::new();
    for (program, source) in programs() {
        for variant in variants(program, false) {
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
                    if set == 0 && global_uniform_binding(VIEW_BUFFER_ID) != Some(binding) {
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
                let declared = |binding: fn(usize) -> u32| -> BTreeSet<u32> {
                    match stage {
                        Stage::Vertex => BTreeSet::new(),
                        Stage::Fragment => (0..program.textures.len()).map(binding).collect(),
                    }
                };
                if textures != declared(image_texture_binding) {
                    fail(format!("textures at {textures:?}, the program has {:?}", declared(image_texture_binding)));
                }
                if samplers != declared(image_sampler_binding) {
                    fail(format!("samplers at {samplers:?}, the program has {:?}", declared(image_sampler_binding)));
                }

                let expected_push_constants = push_constants.map(|_| DRAW_INDEX_PUSH_CONSTANT_SIZE);
                if push_constants != expected_push_constants {
                    fail(format!("push constants of {push_constants:?} bytes, not {expected_push_constants:?}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// Every buffer a stage declares is one of the program's or the engine's, at
// the binding its id has, and as large as the program declares it: the GLSL
// blocks are std140, as the `#[repr(C)]` structs of the program's buffers
// are laid out. A storage buffer has the stride of the program's, but a
// vertex buffer read as one, whose words the GLSL reads. The projection
// block is a storage buffer, or a uniform buffer of the draw's own.
#[test]
fn buffers_are_the_programs() {
    let mut failures = Vec::new();
    for (program, source) in programs() {
        for variant in variants(program, false) {
            for stage in STAGES {
                let at = format!("{} ({}, {stage:?})", program.name, variant.name);
                let resources = resources(&module(program, source, stage, &variant.defines))
                    .unwrap_or_else(|message| panic!("{at}: {message}"));
                for ((set, binding), resource) in resources.bindings {
                    let size = match resource {
                        Resource::Uniform(size) | Resource::Storage(size) => size as usize,
                        Resource::Texture | Resource::Sampler => continue,
                    };
                    let Some(buffer) = program_buffer(program, set, binding) else {
                        failures.push(format!("{at}: {resource:?} at set {set} binding {binding} is no buffer of it"));
                        continue;
                    };
                    let expected = match (buffer.kind, resource) {
                        (BufferKind::Uniform(block) | BufferKind::DrawUniform(block), Resource::Uniform(_)) => {
                            Some(block.size)
                        }
                        (BufferKind::Vertices { .. }, Resource::Storage(_)) => None,
                        (BufferKind::Storage { block }, Resource::Storage(_)) => Some(block.size),
                        _ => {
                            failures.push(format!("{at}: buffer {} is {resource:?}, not {:?}", buffer.id, buffer.kind));
                            continue;
                        }
                    };
                    if expected.is_some_and(|expected| expected != size) {
                        failures.push(format!("{at}: buffer {} is {size} bytes, not {expected:?}", buffer.id));
                    }
                }
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

/// Each attribute of `program` with its location: the vertex attributes,
/// then the instance attributes.
fn locations(program: &'static Program) -> impl Iterator<Item = (usize, &'static Attribute)> {
    program.attributes.iter().chain(program.instance_attributes).enumerate()
}

/// The attributes a program declares that the GLSL doesn't read, by shader
/// and location: the color relief's texture position, which its vertex
/// stage works out from the position.
const UNREAD_ATTRIBUTES: &[(&str, usize)] = &[("color-relief", 1)];

// The inputs of a vertex stage are the vertex and instance attributes of the
// shader's program, at their locations, in types of the scalar kinds of their
// formats with as many components or more. With a property as a uniform, the
// stage reads a subset of them.
#[test]
fn vertex_inputs_are_the_programs_attributes() {
    let mut failures = Vec::new();
    for (program, source) in programs() {
        for variant in variants(program, true) {
            let declared: BTreeMap<u32, (ScalarKind, u32)> = locations(program)
                .filter(|&(location, _)| !UNREAD_ATTRIBUTES.contains(&(program.name, location)))
                .map(|(location, attribute)| (location as u32, input_type(attribute.format)))
                .collect();
            let inputs: BTreeMap<u32, (ScalarKind, u32)> =
                stage_inputs(&module(program, source, Stage::Vertex, &variant.defines))
                    .into_iter()
                    .map(|(location, (kind, components, _))| (location, (kind, components)))
                    .collect();
            let reads = |location: &u32, &(kind, components): &(ScalarKind, u32)| {
                declared.get(location).is_some_and(|&(declared_kind, declared_components)| {
                    declared_kind == kind && declared_components <= components
                })
            };
            let all = !variant.per_vertex || inputs.len() == declared.len();
            if !all || !inputs.iter().all(|(location, input)| reads(location, input)) {
                failures.push(format!(
                    "{} ({}): inputs {inputs:?}, the program has {declared:?}",
                    program.name, variant.name
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// A fragment stage reads each of its inputs where its vertex stage,
// compiled with the same defines, writes it, flat where the other is, and
// the fragment stage writes one color.
#[test]
fn stages_agree_on_what_passes_between_them() {
    let mut failures = Vec::new();
    for (program, source) in programs() {
        for variant in variants(program, true) {
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

// The bindings of the ids and texture slots in the shared pipeline layout,
// which the preludes and `compose` declare the buffers and textures at.
#[test]
fn ids_bind_where_the_shared_layout_puts_them() {
    assert_eq!(global_uniform_binding(VIEW_BUFFER_ID), Some(0));

    // A layer's storage buffers, from binding 0.
    assert_eq!(layer_storage_binding(VERTEX_STORAGE_ID), Some(0));
    assert_eq!(layer_storage_binding(FRAGMENT_STORAGE_ID), Some(1));
    assert_eq!(layer_storage_binding(PROJECTION_BUFFER_ID), Some(2));
    assert_eq!(layer_storage_binding(FIRST_UNIFORM_ID), Some(3));
    assert_eq!(LAYER_STORAGE_COUNT, 4);

    // A layer's uniform blocks, after its storage buffers.
    assert_eq!(layer_uniform_binding(FIRST_UNIFORM_ID), Some(4));
    assert_eq!(layer_uniform_binding(FIRST_UNIFORM_ID + 1), Some(5));
    assert_eq!(LAYER_UNIFORM_COUNT, 2);

    // A draw's own uniform blocks, from binding 0, then the vertex buffers
    // of the pulled shaders.
    assert_eq!(drawable_uniform_binding(VERTEX_STORAGE_ID), Some(0));
    assert_eq!(drawable_uniform_binding(PROJECTION_BUFFER_ID), Some(2));
    assert_eq!(drawable_uniform_binding(FIRST_UNIFORM_ID), Some(3));
    assert_eq!(DRAWABLE_UNIFORM_COUNT, 5);
    assert_eq!(drawable_vertex_binding(FIRST_VERTEX_BUFFER_ID), Some(5));
    assert_eq!(DRAWABLE_VERTEX_COUNT, 3);

    // A texture's sampler, after the image set's textures.
    assert_eq!(IMAGE_SAMPLER_START, 3);
    assert_eq!(image_texture_binding(0), 0);
    assert_eq!(image_sampler_binding(0), 3);
}

// Every program checks, and has no more textures than the image set has
// bindings for.
#[test]
fn the_programs_fit_a_shader() {
    const { assert!(MAX_TEXTURES <= IMAGE_TEXTURE_COUNT) };
    for (program, _) in programs() {
        program.check().unwrap_or_else(|error| panic!("{error}"));
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
    assert!(!sources::PRELUDE_VERTEX.contains("applySurfaceTransform"));
}

/// No source declares a buffer or a texture: `compose` declares the
/// engine's buffers and its program's buffers and textures, by their names.
#[test]
fn no_source_declares_a_buffer_or_a_texture() {
    for (program, source) in programs() {
        for text in [source.prelude, source.vertex, source.fragment] {
            for line in text.lines().filter(|line| line.contains("layout(")) {
                assert!(!line.contains("binding ="), "{}: {line}", program.name);
            }
        }
    }
}

/// No source declares a block of the engine's or its program's: the
/// backend declares them, from their layouts.
#[test]
fn no_source_declares_a_block() {
    let engine = &ENGINE_BUFFERS;
    let preludes = [sources::PRELUDE_VERTEX, sources::PRELUDE_FRAGMENT].concat();
    for (program, source) in programs() {
        let text = [&preludes, source.prelude, source.vertex, source.fragment].concat();
        for block in program.buffers.iter().chain(engine).filter_map(ProgramBuffer::block) {
            for declaration in [format!("struct {} {{", block.name), format!("uniform {} {{", block.name)] {
                assert!(!text.contains(&declaration), "{} declares {}", program.name, block.name);
            }
        }
    }
}

/// The GLSL lays out every block's struct, padded to its buffer's stride,
/// as its Rust struct is laid out.
#[test]
fn every_block_has_its_layout() {
    let engine = &ENGINE_BUFFERS;
    for (program, source) in programs() {
        let variant = &variants(program, false)[0];
        for stage in STAGES {
            let module = module(program, source, stage, &variant.defines);
            let mut layouter = naga::proc::Layouter::default();
            layouter.update(module.to_ctx()).expect("a layout");
            for buffer in program.buffers.iter().chain(engine) {
                let Some(block) = buffer.block() else { continue };
                let Some((handle, ty)) = module.types.iter().find(|(_, ty)| ty.name.as_deref() == Some(block.name))
                else {
                    panic!("{} ({stage:?}) has no struct {}", program.name, block.name);
                };
                let TypeInner::Struct { members, span } = &ty.inner else { panic!("{} isn't a struct", block.name) };
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

/// A buffer's declaration is at the set and binding of its id and kind: the
/// view's block in the global set, a layer's uniform block and a storage
/// buffer in the layer set, a draw's own uniform block and a vertex buffer
/// in the drawable set.
#[test]
fn a_buffers_declaration_is_at_its_ids_binding() {
    use mln_gfx::gfx_types::AttributeDataType::{Float, Short4};
    use mln_gfx::shaders::layer_ubo::ViewUBO;
    use mln_gfx::shaders::program::{DRAW_PROJECTION_BUFFER, PROJECTION_BUFFER, Stages, VIEW_BUFFER};

    let at = |buffer: &ProgramBuffer| {
        let text = declaration(buffer).expect("a declaration");
        text.split_once(')').expect("a layout").0.to_owned()
    };
    assert_eq!(at(&ProgramBuffer::uniform::<ViewUBO>("props", 0)), "layout(std140, set = 1, binding = 4");
    assert_eq!(
        at(&ProgramBuffer::storage::<ViewUBO>("drawableVector", Stages::Vertex)),
        "layout(std140, set = 1, binding = 0"
    );
    assert_eq!(
        at(&ProgramBuffer::draw_uniform::<ViewUBO>("tileProps", Stages::Both)),
        "layout(std140, set = 2, binding = 3"
    );
    assert_eq!(at(&ProgramBuffer::vertices("wallVertices", 0, Short4)), "layout(std430, set = 2, binding = 5");
    assert_eq!(at(&VIEW_BUFFER), "layout(std140, set = 0, binding = 0");
    assert_eq!(at(&PROJECTION_BUFFER), "layout(std140, set = 1, binding = 2");
    assert_eq!(at(&DRAW_PROJECTION_BUFFER), "layout(std140, set = 2, binding = 2");
    assert_eq!(
        declaration(&ProgramBuffer::vertices("wallPaint", 1, Float)).expect("one"),
        "layout(std430, set = 2, binding = 6) readonly buffer wallPaintWords {\n    float wallPaint[];\n};"
    );
}
