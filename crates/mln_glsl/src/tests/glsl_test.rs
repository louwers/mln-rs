//! Parses and validates every program's GLSL with naga, as the backends
//! compose it: `#version 450`, the `ProgramParameters` defines, a
//! `HAS_UNIFORM_u_<name>` define per property passed as a uniform,
//! `PROJECTION_GLOBE` for the globe variant, then the stage's prelude, the
//! shader's prelude and the stage's source. Also checks that the buffers,
//! textures, samplers and vertex inputs the GLSL declares are the ones the
//! descriptor set layouts and the attribute and texture tables describe, that
//! the buffers have the sizes of what the renderer binds there, and that the
//! stages of a shader agree on what passes between them, in both projection
//! variants.

use std::collections::{BTreeMap, BTreeSet};
use std::mem::size_of;

use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::shaders::background_layer_ubo::{
    BackgroundDrawableUnionUBO, BackgroundPatternPropsUBO, BackgroundPropsUBO,
};
use mln_gfx::shaders::circle_layer_ubo::{CircleDrawableUBO, CircleEvaluatedPropsUBO};
use mln_gfx::shaders::color_relief_layer_ubo::{
    ColorReliefDrawableUBO, ColorReliefEvaluatedPropsUBO, ColorReliefTilePropsUBO,
};
use mln_gfx::shaders::fill_extrusion_layer_ubo::{
    FillExtrusionDrawableUBO, FillExtrusionPropsUBO, FillExtrusionTilePropsUBO,
};
use mln_gfx::shaders::fill_layer_ubo::{FillDrawableUnionUBO, FillEvaluatedPropsUBO, FillTilePropsUnionUBO};
use mln_gfx::shaders::heatmap_layer_ubo::{HeatmapDrawableUBO, HeatmapEvaluatedPropsUBO};
use mln_gfx::shaders::heatmap_texture_layer_ubo::HeatmapTexturePropsUBO;
use mln_gfx::shaders::hillshade_layer_ubo::{HillshadeDrawableUBO, HillshadeEvaluatedPropsUBO, HillshadeTilePropsUBO};
use mln_gfx::shaders::hillshade_prepare_layer_ubo::{HillshadePrepareDrawableUBO, HillshadePrepareTilePropsUBO};
use mln_gfx::shaders::layer_composite_ubo::LayerCompositeUBO;
use mln_gfx::shaders::layer_ubo::{GlobalPaintParamsUBO, ProjectionUBO};
use mln_gfx::shaders::line_layer_ubo::{LineDrawableUnionUBO, LineEvaluatedPropsUBO, LineTilePropsUnionUBO};
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::raster_layer_ubo::{RasterDrawableUBO, RasterEvaluatedPropsUBO};
use mln_gfx::shaders::shader_defines::*;
use mln_gfx::shaders::sky_ubo::{AtmospherePropsUBO, SkyPropsUBO};
use mln_gfx::shaders::symbol_layer_ubo::{
    CollisionDrawableUBO, CollisionTilePropsUBO, SymbolDrawableUBO, SymbolEvaluatedPropsUBO, SymbolTilePropsUBO,
};
use mln_gfx::shaders::terrain_ubo::{TerrainDrawableUBO, TerrainSurfaceUBO};
use naga::{AddressSpace, ArraySize, Binding, Interpolation, Module, ScalarKind, StorageAccess, TypeInner};

use crate::front_end::parse;
use crate::layout::{
    DRAWABLE_UNIFORM_COUNT, DRAWABLE_VERTEX_COUNT, DescriptorSetType, GLOBAL_UNIFORM_COUNT, IMAGE_SAMPLER_START,
    IMAGE_TEXTURE_COUNT, LAYER_STORAGE_COUNT, LAYER_UNIFORM_COUNT, UBO_INDEX_PUSH_CONSTANT_SIZE,
    drawable_uniform_binding, drawable_vertex_binding, global_uniform_binding, image_sampler_binding,
    image_texture_binding, layer_storage_binding, layer_uniform_binding,
};
use crate::variants::variants;
use crate::{AttributeInfo, BUILT_INS, ShaderSource, Stage, sources};

fn programs() -> impl Iterator<Item = (&'static Program, &'static ShaderSource)> {
    BUILT_INS.iter().copied()
}

const STAGES: [Stage; 2] = [Stage::Vertex, Stage::Fragment];

/// A variant's defines read a property from its uniform only where the
/// program declares it.
#[test]
fn every_program_declares_the_properties_its_glsl_reads_from_uniforms() {
    const PREFIX: &str = "HAS_UNIFORM_u_";
    for (program, source) in programs() {
        let mut tested = BTreeSet::new();
        for text in [source.glsl.prelude, source.glsl.vertex, source.glsl.fragment] {
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

/// Every program that draws a layer outputs white for the overdraw
/// inspector, which counts its draws.
#[test]
fn every_layer_program_shows_the_overdraw_inspector() {
    use mln_gfx::shaders::programs::{
        ATMOSPHERE, CLIPPING_MASK, COLLISION_BOX, COLLISION_CIRCLE, GLOBE_DEPTH, SKY, TERRAIN, TERRAIN_PROBE,
        VERTEX_TEXEL,
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
        &VERTEX_TEXEL,
        &TERRAIN_PROBE,
    ];
    for &(program, source) in BUILT_INS {
        if !not_layers.contains(&program) {
            assert!(
                [source.glsl.prelude, source.glsl.vertex, source.glsl.fragment]
                    .iter()
                    .any(|text| text.contains("OVERDRAW_INSPECTOR")),
                "{}",
                program.name
            );
        }
    }
}

/// The module of a stage of `source` with `defines`, unvalidated.
fn module(program: &Program, source: &ShaderSource, stage: Stage, defines: &str) -> Module {
    parse(stage, &source.compose(stage, defines))
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
                match crate::Module::parse(stage, &source.compose(stage, &variant.defines)) {
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

/// The buffers of each shader, with the size of what the renderer binds: a
/// uniform buffer's struct, or an element of a storage buffer's vector (the
/// union of the layer's drawable structs where it has several, a word of a
/// vertex buffer).
fn buffers(program: &Program) -> Vec<(Buffer, usize)> {
    use Buffer::{DrawableUniform, DrawableVertex, Global, LayerStorage, LayerUniform};
    let projection = (LayerStorage(ID_PROJECTION_UBO), size_of::<ProjectionUBO>());
    let mut buffers = vec![(Global(ID_GLOBAL_PAINT_PARAMS_UBO), size_of::<GlobalPaintParamsUBO>())];
    let own: Vec<(Buffer, usize)> = match program.name {
        "AtmosphereShader" => vec![(LayerUniform(ID_ATMOSPHERE_PROPS_UBO), size_of::<AtmospherePropsUBO>())],
        "BackgroundShader" => vec![
            (LayerStorage(ID_BACKGROUND_DRAWABLE_UBO), size_of::<BackgroundDrawableUnionUBO>()),
            projection,
            (LayerUniform(ID_BACKGROUND_PROPS_UBO), size_of::<BackgroundPropsUBO>()),
        ],
        "BackgroundPatternShader" => vec![
            (LayerStorage(ID_BACKGROUND_DRAWABLE_UBO), size_of::<BackgroundDrawableUnionUBO>()),
            projection,
            (LayerUniform(ID_BACKGROUND_PROPS_UBO), size_of::<BackgroundPatternPropsUBO>()),
        ],
        "CircleShader" => vec![
            (LayerStorage(ID_CIRCLE_DRAWABLE_UBO), size_of::<CircleDrawableUBO>()),
            projection,
            (LayerUniform(ID_CIRCLE_EVALUATED_PROPS_UBO), size_of::<CircleEvaluatedPropsUBO>()),
        ],
        "ClippingMaskProgram" => vec![(DrawableUniform(ID_PROJECTION_UBO), size_of::<ProjectionUBO>())],
        "CollisionBoxShader" | "CollisionCircleShader" => vec![
            (DrawableUniform(ID_COLLISION_DRAWABLE_UBO), size_of::<CollisionDrawableUBO>()),
            (DrawableUniform(ID_COLLISION_TILE_PROPS_UBO), size_of::<CollisionTilePropsUBO>()),
            (DrawableUniform(ID_COLLISION_PROJECTION_UBO), size_of::<ProjectionUBO>()),
        ],
        "ColorReliefShader" => vec![
            (LayerStorage(ID_COLOR_RELIEF_DRAWABLE_UBO), size_of::<ColorReliefDrawableUBO>()),
            projection,
            (LayerStorage(ID_COLOR_RELIEF_TILE_PROPS_UBO), size_of::<ColorReliefTilePropsUBO>()),
            (LayerUniform(ID_COLOR_RELIEF_EVALUATED_PROPS_UBO), size_of::<ColorReliefEvaluatedPropsUBO>()),
        ],
        "FillShader" | "FillOutlineShader" | "FillOutlineInstancedShader" => vec![
            (LayerStorage(ID_FILL_DRAWABLE_UBO), size_of::<FillDrawableUnionUBO>()),
            projection,
            (LayerUniform(ID_FILL_EVALUATED_PROPS_UBO), size_of::<FillEvaluatedPropsUBO>()),
        ],
        "FillOutlinePulledShader" => vec![
            (LayerStorage(ID_FILL_DRAWABLE_UBO), size_of::<FillDrawableUnionUBO>()),
            projection,
            (LayerUniform(ID_FILL_EVALUATED_PROPS_UBO), size_of::<FillEvaluatedPropsUBO>()),
            (DrawableVertex(ID_FILL_OUTLINE_VERTICES), size_of::<u32>()),
            (DrawableVertex(ID_FILL_OUTLINE_ENDS), size_of::<u32>()),
            (DrawableVertex(ID_FILL_OUTLINE_PAINT), size_of::<f32>()),
        ],
        "FillPatternShader" | "FillOutlinePatternShader" | "FillOutlinePatternInstancedShader" => vec![
            (LayerStorage(ID_FILL_DRAWABLE_UBO), size_of::<FillDrawableUnionUBO>()),
            projection,
            (LayerStorage(ID_FILL_TILE_PROPS_UBO), size_of::<FillTilePropsUnionUBO>()),
            (LayerUniform(ID_FILL_EVALUATED_PROPS_UBO), size_of::<FillEvaluatedPropsUBO>()),
        ],
        "FillExtrusionShader" | "FillExtrusionInstancedShader" | "FillExtrusionWallShader" => vec![
            (LayerStorage(ID_FILL_EXTRUSION_DRAWABLE_UBO), size_of::<FillExtrusionDrawableUBO>()),
            projection,
            (LayerUniform(ID_FILL_EXTRUSION_PROPS_UBO), size_of::<FillExtrusionPropsUBO>()),
        ],
        "FillExtrusionPulledShader" => vec![
            (LayerStorage(ID_FILL_EXTRUSION_DRAWABLE_UBO), size_of::<FillExtrusionDrawableUBO>()),
            projection,
            (LayerUniform(ID_FILL_EXTRUSION_PROPS_UBO), size_of::<FillExtrusionPropsUBO>()),
            (DrawableVertex(ID_FILL_EXTRUSION_WALL_VERTICES), size_of::<u32>()),
            (DrawableVertex(ID_FILL_EXTRUSION_WALL_PAINT), size_of::<f32>()),
        ],
        "FillExtrusionPatternShader" | "FillExtrusionPatternInstancedShader" | "FillExtrusionPatternWallShader" => {
            vec![
                (LayerStorage(ID_FILL_EXTRUSION_DRAWABLE_UBO), size_of::<FillExtrusionDrawableUBO>()),
                projection,
                (LayerStorage(ID_FILL_EXTRUSION_TILE_PROPS_UBO), size_of::<FillExtrusionTilePropsUBO>()),
                (LayerUniform(ID_FILL_EXTRUSION_PROPS_UBO), size_of::<FillExtrusionPropsUBO>()),
            ]
        }
        "GlobeDepthShader" => vec![projection],
        "HeatmapShader" => vec![
            (LayerStorage(ID_HEATMAP_DRAWABLE_UBO), size_of::<HeatmapDrawableUBO>()),
            projection,
            (LayerUniform(ID_HEATMAP_EVALUATED_PROPS_UBO), size_of::<HeatmapEvaluatedPropsUBO>()),
        ],
        "HeatmapTextureShader" => {
            vec![(LayerUniform(ID_HEATMAP_TEXTURE_PROPS_UBO), size_of::<HeatmapTexturePropsUBO>())]
        }
        "LayerCompositeShader" => vec![(LayerUniform(ID_LAYER_COMPOSITE_UBO), size_of::<LayerCompositeUBO>())],
        "VertexTexelShader" => vec![],
        "TerrainShader" => vec![
            (LayerStorage(ID_TERRAIN_DRAWABLE_UBO), size_of::<TerrainDrawableUBO>()),
            projection,
            (LayerStorage(ID_TERRAIN_LIGHT_TILE_UBO), size_of::<HillshadeTilePropsUBO>()),
            (LayerUniform(ID_TERRAIN_PROPS_UBO), size_of::<TerrainSurfaceUBO>()),
            (LayerUniform(ID_TERRAIN_LIGHT_UBO), size_of::<HillshadeEvaluatedPropsUBO>()),
        ],
        "TerrainProbeShader" => vec![(LayerUniform(ID_TERRAIN_PROPS_UBO), size_of::<TerrainSurfaceUBO>())],
        "HillshadeShader" => vec![
            (LayerStorage(ID_HILLSHADE_DRAWABLE_UBO), size_of::<HillshadeDrawableUBO>()),
            projection,
            (LayerStorage(ID_HILLSHADE_TILE_PROPS_UBO), size_of::<HillshadeTilePropsUBO>()),
            (LayerUniform(ID_HILLSHADE_EVALUATED_PROPS_UBO), size_of::<HillshadeEvaluatedPropsUBO>()),
        ],
        "HillshadePrepareShader" => vec![
            (DrawableUniform(ID_HILLSHADE_PREPARE_DRAWABLE_UBO), size_of::<HillshadePrepareDrawableUBO>()),
            (DrawableUniform(ID_HILLSHADE_PREPARE_TILE_PROPS_UBO), size_of::<HillshadePrepareTilePropsUBO>()),
        ],
        "LineShader" | "LineGradientShader" => vec![
            (LayerStorage(ID_LINE_DRAWABLE_UBO), size_of::<LineDrawableUnionUBO>()),
            projection,
            (LayerUniform(ID_LINE_EVALUATED_PROPS_UBO), size_of::<LineEvaluatedPropsUBO>()),
        ],
        "LinePatternShader" | "LineSDFShader" | "LineGradientSDFShader" => vec![
            (LayerStorage(ID_LINE_DRAWABLE_UBO), size_of::<LineDrawableUnionUBO>()),
            projection,
            (LayerStorage(ID_LINE_TILE_PROPS_UBO), size_of::<LineTilePropsUnionUBO>()),
            (LayerUniform(ID_LINE_EVALUATED_PROPS_UBO), size_of::<LineEvaluatedPropsUBO>()),
        ],
        "RasterShader" => vec![
            (LayerStorage(ID_RASTER_DRAWABLE_UBO), size_of::<RasterDrawableUBO>()),
            projection,
            (LayerUniform(ID_RASTER_EVALUATED_PROPS_UBO), size_of::<RasterEvaluatedPropsUBO>()),
        ],
        "SkyShader" => vec![(LayerUniform(ID_SKY_PROPS_UBO), size_of::<SkyPropsUBO>())],
        "SymbolIconShader" | "SymbolSDFShader" | "SymbolTextAndIconShader" => vec![
            (LayerStorage(ID_SYMBOL_DRAWABLE_UBO), size_of::<SymbolDrawableUBO>()),
            projection,
            (LayerStorage(ID_SYMBOL_TILE_PROPS_UBO), size_of::<SymbolTilePropsUBO>()),
            (LayerUniform(ID_SYMBOL_EVALUATED_PROPS_UBO), size_of::<SymbolEvaluatedPropsUBO>()),
        ],
        name => panic!("no buffers for {name}"),
    };
    buffers.extend(own);
    buffers
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
// the stages declare the textures of the texture table between them, a
// fragment stage each of its own with its sampler and a vertex stage none,
// since it fetches texels, and the push constants are the general pipeline
// layout's index or, for the clipping mask, its matrices.
#[test]
fn resources_are_in_the_descriptor_set_layouts() {
    assert_eq!(DescriptorSetType::Global.set_index(), 0);
    assert_eq!(DescriptorSetType::Layer.set_index(), 1);
    assert_eq!(DescriptorSetType::DrawableUniform.set_index(), 2);
    assert_eq!(DescriptorSetType::DrawableImage.set_index(), 3);

    let mut failures = Vec::new();
    for (program, source) in programs() {
        let table: BTreeSet<u32> = source.textures.iter().map(|texture| image_texture_binding(texture.index)).collect();
        for variant in variants(program, false) {
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
                    if set == 0 && global_uniform_binding(ID_GLOBAL_PAINT_PARAMS_UBO) != Some(binding) {
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
                let sampled: BTreeSet<u32> = match stage {
                    Stage::Vertex => BTreeSet::new(),
                    Stage::Fragment => {
                        textures.iter().map(|&binding| image_sampler_binding(binding as usize)).collect()
                    }
                };
                if samplers != sampled {
                    fail(format!("samplers at {samplers:?}, not {sampled:?}"));
                }
                declared.extend(textures);

                let expected_push_constants = push_constants.map(|_| UBO_INDEX_PUSH_CONSTANT_SIZE);
                if push_constants != expected_push_constants {
                    fail(format!("push constants of {push_constants:?} bytes, not {expected_push_constants:?}"));
                }
            }
            if declared != table {
                failures.push(format!(
                    "{} ({}): textures at {declared:?}, the table has {table:?}",
                    program.name, variant.name
                ));
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
        for variant in variants(program, false) {
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

/// The vertex and instance attributes of a table.
fn all_attributes(source: &ShaderSource) -> impl Iterator<Item = &AttributeInfo> {
    source.attributes.iter().chain(source.instance_attributes)
}

/// The attributes a table has that the GLSL doesn't read, by shader and
/// index: the color relief's texture position, which its vertex stage works
/// out from the position.
const UNREAD_ATTRIBUTES: &[(&str, usize)] = &[("ColorReliefShader", 1)];

// The inputs of a vertex stage are the vertex and instance attributes of the
// shader's tables, at the locations of their indices and in the types that
// read their formats. With a property as a uniform, the stage reads a subset
// of them.
#[test]
fn vertex_inputs_are_the_attribute_tables() {
    let mut failures = Vec::new();
    for (program, source) in programs() {
        let indices: BTreeSet<usize> = all_attributes(source).map(|attr| attr.index).collect();
        assert_eq!(indices.len(), all_attributes(source).count(), "{}: attribute indices repeat", program.name);
        for variant in variants(program, true) {
            let table: BTreeMap<u32, (ScalarKind, u32)> = all_attributes(source)
                .filter(|attr| !UNREAD_ATTRIBUTES.contains(&(program.name, attr.index)))
                .map(|attr| (attr.index as u32, input_type(attr.data_type)))
                .collect();
            let inputs: BTreeMap<u32, (ScalarKind, u32)> =
                stage_inputs(&module(program, source, Stage::Vertex, &variant.defines))
                    .into_iter()
                    .map(|(location, (kind, components, _))| (location, (kind, components)))
                    .collect();
            let matches = if variant.per_vertex {
                inputs == table
            } else {
                inputs.iter().all(|(location, input)| table.get(location) == Some(input))
            };
            if !matches {
                failures
                    .push(format!("{} ({}): inputs {inputs:?}, the table has {table:?}", program.name, variant.name));
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
        .filter(|(program, source)| program.name.starts_with("FillExtrusion") && !source.instance_attributes.is_empty())
        .collect();
    assert_eq!(instanced.len(), 2);
    for (program, source) in instanced {
        let format = |id: usize| {
            let mut found = source.instance_attributes.iter().filter(|attr| attr.id == id);
            let attr = found.next().unwrap_or_else(|| panic!("{}: no instance attribute {id}", program.name));
            assert!(found.next().is_none());
            attr.data_type
        };
        for (id, next_id) in [
            (ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE, ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE),
            (ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE),
        ] {
            assert_eq!(format(id), format(next_id), "{}", program.name);
        }
    }
}

// compiled with the same defines writes there, flat where the other is, and
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

// The bindings the GLSL spells out as numbers are the ones the packed ids
// have: `layerSSBOStartId`, `layerUBOStartId` and `drawableUBOStartId` with
// the offsets the shader preludes add, and `samplerBindingStartId`.
#[test]
fn bindings_follow_from_the_packed_ids() {
    assert_eq!(global_uniform_binding(ID_GLOBAL_PAINT_PARAMS_UBO), Some(0));

    // idDrawableReservedVertexOnlyUBO, idDrawableReservedFragmentOnlyUBO,
    // idProjectionUBO and drawableReservedUBOCount, from layerSSBOStartId.
    assert_eq!(layer_storage_binding(ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO), Some(0));
    assert_eq!(layer_storage_binding(ID_DRAWABLE_RESERVED_FRAGMENT_ONLY_UBO), Some(1));
    assert_eq!(layer_storage_binding(ID_PROJECTION_UBO), Some(2));
    assert_eq!(layer_storage_binding(DRAWABLE_RESERVED_UBO_COUNT), Some(3));
    for id in [ID_FILL_TILE_PROPS_UBO, ID_FILL_EXTRUSION_TILE_PROPS_UBO, ID_COLOR_RELIEF_TILE_PROPS_UBO] {
        assert_eq!(layer_storage_binding(id), Some(3));
    }
    assert_eq!(LAYER_STORAGE_COUNT, 4);

    // layerUBOStartId.
    for id in [
        ID_BACKGROUND_PROPS_UBO,
        ID_CIRCLE_EVALUATED_PROPS_UBO,
        ID_HEATMAP_EVALUATED_PROPS_UBO,
        ID_HEATMAP_TEXTURE_PROPS_UBO,
        ID_HILLSHADE_EVALUATED_PROPS_UBO,
        ID_LINE_EVALUATED_PROPS_UBO,
        ID_RASTER_EVALUATED_PROPS_UBO,
        ID_SYMBOL_EVALUATED_PROPS_UBO,
        ID_SKY_PROPS_UBO,
        ID_ATMOSPHERE_PROPS_UBO,
    ] {
        assert_eq!(layer_uniform_binding(id), Some(4));
    }
    // layerUBOStartId + 1: the layers with a tile properties storage buffer.
    for id in [ID_FILL_EVALUATED_PROPS_UBO, ID_FILL_EXTRUSION_PROPS_UBO, ID_COLOR_RELIEF_EVALUATED_PROPS_UBO] {
        assert_eq!(layer_uniform_binding(id), Some(5));
    }

    // drawableUBOStartId and its offsets.
    assert_eq!(drawable_uniform_binding(ID_COLLISION_DRAWABLE_UBO), Some(0));
    assert_eq!(drawable_uniform_binding(ID_COLLISION_PROJECTION_UBO), Some(2));
    assert_eq!(drawable_uniform_binding(ID_COLLISION_TILE_PROPS_UBO), Some(3));
    assert_eq!(drawable_uniform_binding(ID_HILLSHADE_PREPARE_DRAWABLE_UBO), Some(0));
    assert_eq!(drawable_uniform_binding(ID_HILLSHADE_PREPARE_TILE_PROPS_UBO), Some(3));
    // The pulled shaders' vertex buffers, from drawableUBOStartId + 5.
    assert_eq!(DRAWABLE_UNIFORM_COUNT, 5);
    assert_eq!(drawable_vertex_binding(ID_FILL_EXTRUSION_WALL_VERTICES), Some(5));
    assert_eq!(drawable_vertex_binding(ID_FILL_EXTRUSION_WALL_PAINT), Some(6));
    assert_eq!(drawable_vertex_binding(ID_FILL_OUTLINE_VERTICES), Some(5));
    assert_eq!(drawable_vertex_binding(ID_FILL_OUTLINE_ENDS), Some(6));
    assert_eq!(drawable_vertex_binding(ID_FILL_OUTLINE_PAINT), Some(7));

    // samplerBindingStartId.
    assert_eq!(IMAGE_SAMPLER_START, 3);
    assert_eq!(image_texture_binding(0), 0);
    assert_eq!(image_sampler_binding(0), 3);
}

// No table has more attributes and textures than a shader can have. Only
// the fill extrusion walls and the instanced fill outlines have instance
// attributes.
#[test]
fn the_tables_fit_a_shader() {
    for (program, source) in programs() {
        assert!(all_attributes(source).count() <= MAX_ATTRIBUTE_COUNT_PER_SHADER, "{}", program.name);
        assert!(source.textures.len() <= MAX_TEXTURE_COUNT_PER_SHADER, "{}", program.name);
        let instanced = matches!(
            program.name,
            "FillExtrusionInstancedShader"
                | "FillExtrusionPatternInstancedShader"
                | "FillOutlineInstancedShader"
                | "FillOutlinePatternInstancedShader"
        );
        assert_eq!(!source.instance_attributes.is_empty(), instanced, "{}", program.name);
        let ids: BTreeSet<usize> = source.attributes.iter().map(|attr| attr.id).collect();
        assert_eq!(ids.len(), source.attributes.len(), "{}: attribute ids repeat", program.name);
        let ids: BTreeSet<usize> = source.instance_attributes.iter().map(|attr| attr.id).collect();
        assert_eq!(ids.len(), source.instance_attributes.len(), "{}: instance attribute ids repeat", program.name);
        let textures: BTreeSet<usize> = source.textures.iter().map(|texture| texture.index).collect();
        assert_eq!(textures.len(), source.textures.len(), "{}: texture indices repeat", program.name);
        assert!(textures.iter().all(|&index| index < IMAGE_TEXTURE_COUNT), "{}", program.name);
    }
}

// Clip space is y-up: no vertex stage flips Y, which the back end for a
// y-down clip space does.
#[test]
fn no_stage_flips_y() {
    for (program, source) in programs() {
        let text: String = source.glsl.vertex.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(!text.contains("gl_Position.y*=-1"), "{}", program.name);
        assert!(!text.contains("applySurfaceTransform"), "{}", program.name);
    }
    assert!(!sources::PRELUDE_VERTEX.contains("applySurfaceTransform"));
}
