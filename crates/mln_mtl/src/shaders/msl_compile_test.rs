//! Compiles every built-in program's MSL with the system's
//! Metal compiler, the way `MetalGpu` does: `PRELUDE + prelude + source`,
//! MSL 2.4, fast math unless the program's table turns it off, the
//! `ProgramParameters` defines, a
//! `HAS_UNIFORM_u_<name>` define per property passed as a uniform, and
//! `PROJECTION_GLOBE` for the globe variant. Also checks that the UBO ids and
//! sizes the MSL declares are the renderer's, and that the shaders' pipelines
//! build with vertex buffers of the attributes their programs declare, in
//! both projection variants.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::mem::size_of;

use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::gpu::VertexStep;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::bindings::{
    FIRST_UNIFORM_ID, FRAGMENT_DRAW_ID, GLOBAL_INDEX_ID, PAINT_PARAMS_ID, PROJECTION_ID, VERTEX_DRAW_ID,
};
use mln_gfx::shaders::program::{GLOBE_DEFINE, Program, TERRAIN_DEFINE};
use mln_gfx::shaders::*;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::{NSDictionary, NSObject, NSString};
use objc2_metal::{
    MTLCompileOptions, MTLCreateSystemDefaultDevice, MTLDevice, MTLFunction, MTLLanguageVersion, MTLLibrary,
    MTLPixelFormat, MTLRenderPipelineDescriptor, MTLVertexDescriptor, MTLVertexFormat, MTLVertexStepFunction,
};

use super::msl_sources::PRELUDE;
use super::shader_source::{BUILT_INS, ShaderSource, first_vertex_buffer};
use crate::Id;

fn sources() -> impl Iterator<Item = (&'static Program, &'static ShaderSource)> {
    BUILT_INS.iter().copied()
}

fn device() -> Id<dyn MTLDevice> {
    MTLCreateSystemDefaultDevice().expect("no Metal device")
}

/// The library of `source` with `defines`, and with fast math if
/// `fast_math`, or the compiler's error.
fn compile(
    device: &ProtocolObject<dyn MTLDevice>,
    source: &str,
    defines: &[(String, String)],
    fast_math: bool,
) -> Result<Id<dyn MTLLibrary>, String> {
    let keys: Vec<Retained<NSString>> = defines.iter().map(|(key, _)| NSString::from_str(key)).collect();
    let values: Vec<Retained<NSString>> = defines.iter().map(|(_, value)| NSString::from_str(value)).collect();
    let keys: Vec<&NSString> = keys.iter().map(|key| &**key).collect();
    let values: Vec<&NSObject> = values.iter().map(|value| &***value).collect();
    let macros = NSDictionary::<NSString, NSObject>::from_slices(&keys, &values);

    let options = MTLCompileOptions::new();
    // SAFETY: the macros map NSString names to NSString values.
    unsafe { options.setPreprocessorMacros(Some(&macros)) };
    // Fast math is Metal's default.
    if !fast_math {
        #[expect(deprecated, reason = "its replacement, mathMode, needs macOS 15 and iOS 18")]
        options.setFastMathEnabled(false);
    }
    options.setLanguageVersion(MTLLanguageVersion::Version2_4);
    options.setPreserveInvariance(true);
    device
        .newLibraryWithSource_options_error(&NSString::from_str(source), Some(&options))
        .map_err(|error| error.localizedDescription().to_string())
}

fn full_source(source: &ShaderSource) -> String {
    format!("{PRELUDE}{}{}", source.msl.prelude, source.msl.source)
}

/// The `<name>`s of the `HAS_UNIFORM_u_<name>` the shader tests.
fn uniform_properties(text: &str) -> BTreeSet<String> {
    const PREFIX: &str = "HAS_UNIFORM_u_";
    let mut names = BTreeSet::new();
    let mut rest = text;
    while let Some(start) = rest.find(PREFIX) {
        rest = &rest[start + PREFIX.len()..];
        let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(rest.len());
        names.insert(rest[..end].to_owned());
        rest = &rest[end..];
    }
    names
}

/// ProgramParameters(1.0, overdraw), the globe define if `globe`, and a
/// uniform define per name.
fn defines<'a>(
    overdraw: bool,
    projection: ProjectionVariant,
    uniforms: impl IntoIterator<Item = &'a String>,
) -> Vec<(String, String)> {
    let mut defines = vec![("DEVICE_PIXEL_RATIO".to_owned(), "1.0".to_owned())];
    if overdraw {
        defines.push(("OVERDRAW_INSPECTOR".to_owned(), String::new()));
    }
    if projection.is_globe() {
        defines.push((GLOBE_DEFINE.to_owned(), String::new()));
    }
    if projection.stands() {
        defines.push((TERRAIN_DEFINE.to_owned(), String::new()));
    }
    defines.extend(uniforms.into_iter().map(|name| (format!("HAS_UNIFORM_u_{name}"), String::new())));
    defines
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
            assert!(full_source(source).contains("OVERDRAW_INSPECTOR"), "{}", program.name);
        }
    }
}

#[test]
fn every_shader_source_compiles() {
    let device = device();
    let mut failures = Vec::new();
    let mut compiled = 0;
    for (program, source) in sources() {
        let text = full_source(source);
        let properties: BTreeSet<String> = program.properties.iter().map(|property| property.name.to_owned()).collect();
        assert_eq!(
            uniform_properties(&text),
            properties,
            "{}: the properties its MSL reads from uniforms",
            program.name
        );
        for &projection in ProjectionVariant::of(program) {
            // Every property per vertex, every property as a uniform, each one
            // alone as a uniform, and the overdraw inspector.
            let mut variants = vec![
                ("no uniforms".to_owned(), defines(false, projection, [])),
                ("all uniforms".to_owned(), defines(false, projection, &properties)),
                ("overdraw inspector".to_owned(), defines(true, projection, [])),
                ("overdraw inspector, all uniforms".to_owned(), defines(true, projection, &properties)),
            ];
            variants
                .extend(properties.iter().map(|name| (format!("uniform {name}"), defines(false, projection, [name]))));
            for (variant, defines) in variants {
                match compile(&device, &text, &defines, source.fast_math) {
                    Ok(library) => {
                        compiled += 1;
                        for function in [source.msl.vertex_main_function, source.msl.fragment_main_function] {
                            if library.newFunctionWithName(&NSString::from_str(function)).is_none() {
                                failures.push(format!(
                                    "{} ({projection:?}, {variant}): no function {function}",
                                    program.name
                                ));
                            }
                        }
                    }
                    Err(error) => failures.push(format!("{} ({projection:?}, {variant}):\n{error}", program.name)),
                }
            }
        }
    }
    assert!(failures.is_empty(), "{} of {} variants failed:\n{}", failures.len(), compiled + failures.len(), {
        failures.join("\n\n")
    });
}

/// The UBO id enumerators of each shader's prelude, with the ids of the
/// buffers the programs declare.
fn ubo_ids(program: &Program) -> Vec<(&'static str, usize)> {
    let mut ids = vec![
        ("idGlobalPaintParamsUBO", PAINT_PARAMS_ID),
        ("idGlobalUBOIndex", GLOBAL_INDEX_ID),
        ("idDrawableReservedVertexOnlyUBO", VERTEX_DRAW_ID),
        ("idDrawableReservedFragmentOnlyUBO", FRAGMENT_DRAW_ID),
        ("idProjectionUBO", PROJECTION_ID),
        ("drawableReservedUBOCount", FIRST_UNIFORM_ID),
    ];
    let layer: Vec<(&str, usize)> = match program.name {
        "AtmosphereShader" => vec![("idAtmospherePropsUBO", programs::ATMOSPHERE.buffer_id("atmosphere"))],
        "BackgroundShader" | "BackgroundPatternShader" => vec![
            ("idBackgroundDrawableUBO", programs::BACKGROUND.buffer_id("drawableVector")),
            ("idBackgroundPropsUBO", programs::BACKGROUND.buffer_id("props")),
        ],
        "CircleShader" => vec![
            ("idCircleDrawableUBO", programs::CIRCLE.buffer_id("drawableVector")),
            ("idCircleEvaluatedPropsUBO", programs::CIRCLE.buffer_id("props")),
        ],
        "ClippingMaskProgram" | "GlobeDepthShader" | "VertexTexelShader" => vec![],
        "CollisionBoxShader" | "CollisionCircleShader" => vec![
            ("idCollisionDrawableUBO", programs::COLLISION_BOX.buffer_id("drawable")),
            ("idCollisionTilePropsUBO", programs::COLLISION_BOX.buffer_id("tile_props")),
            ("idCollisionProjectionUBO", PROJECTION_ID),
        ],
        "ColorReliefShader" => vec![
            ("idColorReliefDrawableUBO", programs::COLOR_RELIEF.buffer_id("drawableVector")),
            ("idColorReliefTilePropsUBO", programs::COLOR_RELIEF.buffer_id("tilePropsVector")),
            ("idColorReliefEvaluatedPropsUBO", programs::COLOR_RELIEF.buffer_id("props")),
        ],
        "FillShader"
        | "FillOutlineShader"
        | "FillPatternShader"
        | "FillOutlinePatternShader"
        | "FillOutlineInstancedShader"
        | "FillOutlinePulledShader"
        | "FillOutlinePatternInstancedShader" => vec![
            ("idFillDrawableUBO", programs::FILL.buffer_id("drawableVector")),
            ("idFillTilePropsUBO", programs::FILL_PATTERN.buffer_id("tilePropsVector")),
            ("idFillEvaluatedPropsUBO", programs::FILL.buffer_id("props")),
        ],
        "FillExtrusionShader"
        | "FillExtrusionInstancedShader"
        | "FillExtrusionPulledShader"
        | "FillExtrusionPatternShader"
        | "FillExtrusionPatternInstancedShader" => vec![
            ("idFillExtrusionDrawableUBO", programs::FILL_EXTRUSION.buffer_id("drawableVector")),
            ("idFillExtrusionTilePropsUBO", programs::FILL_EXTRUSION_PATTERN.buffer_id("tilePropsVector")),
            ("idFillExtrusionPropsUBO", programs::FILL_EXTRUSION.buffer_id("props")),
        ],
        "HeatmapShader" => vec![
            ("idHeatmapDrawableUBO", programs::HEATMAP.buffer_id("drawableVector")),
            ("idHeatmapEvaluatedPropsUBO", programs::HEATMAP.buffer_id("props")),
        ],
        "HeatmapTextureShader" => vec![("idHeatmapTexturePropsUBO", programs::HEATMAP_TEXTURE.buffer_id("ubo"))],
        "LayerCompositeShader" => vec![("idLayerCompositeUBO", programs::LAYER_COMPOSITE.buffer_id("props"))],
        "TerrainShader" | "TerrainProbeShader" | "TerrainDepthShader" => vec![
            ("idTerrainDrawableUBO", programs::TERRAIN.buffer_id("drawableVector")),
            ("idTerrainLightTileUBO", programs::TERRAIN.buffer_id("lightVector")),
            ("idTerrainPropsUBO", programs::TERRAIN.buffer_id("terrain")),
            ("idTerrainLightUBO", programs::TERRAIN.buffer_id("props")),
        ],
        "HillshadeShader" => vec![
            ("idHillshadeDrawableUBO", programs::HILLSHADE.buffer_id("drawableVector")),
            ("idHillshadeTilePropsUBO", programs::HILLSHADE.buffer_id("tilePropsVector")),
            ("idHillshadeEvaluatedPropsUBO", programs::HILLSHADE.buffer_id("props")),
        ],
        "HillshadePrepareShader" => vec![
            ("idHillshadePrepareDrawableUBO", programs::HILLSHADE_PREPARE.buffer_id("drawable")),
            ("idHillshadePrepareTilePropsUBO", programs::HILLSHADE_PREPARE.buffer_id("tileProps")),
        ],
        "LineShader" | "LineGradientShader" | "LinePatternShader" | "LineSDFShader" | "LineGradientSDFShader" => vec![
            ("idLineDrawableUBO", programs::LINE.buffer_id("drawableVector")),
            ("idLineTilePropsUBO", programs::LINE_PATTERN.buffer_id("tilePropsVector")),
            ("idLineEvaluatedPropsUBO", programs::LINE.buffer_id("props")),
        ],
        "RasterShader" => vec![
            ("idRasterDrawableUBO", programs::RASTER.buffer_id("drawableVector")),
            ("idRasterEvaluatedPropsUBO", programs::RASTER.buffer_id("props")),
        ],
        "SkyShader" => vec![("idSkyPropsUBO", programs::SKY.buffer_id("sky"))],
        "SymbolIconShader" | "SymbolSDFShader" | "SymbolTextAndIconShader" => vec![
            ("idSymbolDrawableUBO", programs::SYMBOL_SDF.buffer_id("drawableVector")),
            ("idSymbolTilePropsUBO", programs::SYMBOL_SDF.buffer_id("tilePropsVector")),
            ("idSymbolEvaluatedPropsUBO", programs::SYMBOL_SDF.buffer_id("props")),
        ],
        name => panic!("no UBO ids for {name}"),
    };
    ids.extend(layer);
    // The vertex buffers that the pulled shaders read as storage buffers.
    ids.extend(match program.name {
        "FillExtrusionPulledShader" => vec![
            ("idFillExtrusionWallVertices", programs::FILL_EXTRUSION_PULLED.buffer_id("wallVertices")),
            ("idFillExtrusionWallPaint", programs::FILL_EXTRUSION_PULLED.buffer_id("wallPaint")),
            ("idFillExtrusionWallHeights", programs::FILL_EXTRUSION_PULLED.buffer_id("wallHeights")),
        ],
        "FillOutlinePulledShader" => vec![
            ("idFillOutlineVertices", programs::FILL_OUTLINE_PULLED.buffer_id("outlineVertices")),
            ("idFillOutlineEnds", programs::FILL_OUTLINE_PULLED.buffer_id("outlineEnds")),
            ("idFillOutlinePaint", programs::FILL_OUTLINE_PULLED.buffer_id("outlinePaint")),
        ],
        _ => vec![],
    });
    ids
}

/// The renderer's UBO structs, by the name the MSL gives them.
const UBO_SIZES: &[(&str, usize)] = &[
    ("GlobalPaintParamsUBO", size_of::<layer_ubo::GlobalPaintParamsUBO>()),
    ("ProjectionUBO", size_of::<layer_ubo::ProjectionUBO>()),
    ("SkyPropsUBO", size_of::<sky_ubo::SkyPropsUBO>()),
    ("AtmospherePropsUBO", size_of::<sky_ubo::AtmospherePropsUBO>()),
    ("BackgroundDrawableUBO", size_of::<background_layer_ubo::BackgroundDrawableUBO>()),
    ("BackgroundPropsUBO", size_of::<background_layer_ubo::BackgroundPropsUBO>()),
    ("BackgroundPatternDrawableUBO", size_of::<background_layer_ubo::BackgroundPatternDrawableUBO>()),
    ("BackgroundPatternPropsUBO", size_of::<background_layer_ubo::BackgroundPatternPropsUBO>()),
    ("BackgroundDrawableUnionUBO", size_of::<background_layer_ubo::BackgroundDrawableUnionUBO>()),
    ("CircleDrawableUBO", size_of::<circle_layer_ubo::CircleDrawableUBO>()),
    ("CircleEvaluatedPropsUBO", size_of::<circle_layer_ubo::CircleEvaluatedPropsUBO>()),
    ("CollisionDrawableUBO", size_of::<symbol_layer_ubo::CollisionDrawableUBO>()),
    ("CollisionTilePropsUBO", size_of::<symbol_layer_ubo::CollisionTilePropsUBO>()),
    ("ColorReliefDrawableUBO", size_of::<color_relief_layer_ubo::ColorReliefDrawableUBO>()),
    ("ColorReliefTilePropsUBO", size_of::<color_relief_layer_ubo::ColorReliefTilePropsUBO>()),
    ("ColorReliefEvaluatedPropsUBO", size_of::<color_relief_layer_ubo::ColorReliefEvaluatedPropsUBO>()),
    ("FillDrawableUBO", size_of::<fill_layer_ubo::FillDrawableUBO>()),
    ("FillOutlineDrawableUBO", size_of::<fill_layer_ubo::FillOutlineDrawableUBO>()),
    ("FillPatternDrawableUBO", size_of::<fill_layer_ubo::FillPatternDrawableUBO>()),
    ("FillPatternTilePropsUBO", size_of::<fill_layer_ubo::FillPatternTilePropsUBO>()),
    ("FillOutlinePatternDrawableUBO", size_of::<fill_layer_ubo::FillOutlinePatternDrawableUBO>()),
    ("FillOutlinePatternTilePropsUBO", size_of::<fill_layer_ubo::FillOutlinePatternTilePropsUBO>()),
    ("FillEvaluatedPropsUBO", size_of::<fill_layer_ubo::FillEvaluatedPropsUBO>()),
    ("FillDrawableUnionUBO", size_of::<fill_layer_ubo::FillDrawableUnionUBO>()),
    ("FillTilePropsUnionUBO", size_of::<fill_layer_ubo::FillTilePropsUnionUBO>()),
    ("FillExtrusionDrawableUBO", size_of::<fill_extrusion_layer_ubo::FillExtrusionDrawableUBO>()),
    ("FillExtrusionTilePropsUBO", size_of::<fill_extrusion_layer_ubo::FillExtrusionTilePropsUBO>()),
    ("FillExtrusionPropsUBO", size_of::<fill_extrusion_layer_ubo::FillExtrusionPropsUBO>()),
    ("HeatmapDrawableUBO", size_of::<heatmap_layer_ubo::HeatmapDrawableUBO>()),
    ("HeatmapEvaluatedPropsUBO", size_of::<heatmap_layer_ubo::HeatmapEvaluatedPropsUBO>()),
    ("HeatmapTexturePropsUBO", size_of::<heatmap_texture_layer_ubo::HeatmapTexturePropsUBO>()),
    ("LayerCompositeUBO", size_of::<layer_composite_ubo::LayerCompositeUBO>()),
    ("HillshadeDrawableUBO", size_of::<hillshade_layer_ubo::HillshadeDrawableUBO>()),
    ("HillshadeTilePropsUBO", size_of::<hillshade_layer_ubo::HillshadeTilePropsUBO>()),
    ("HillshadeEvaluatedPropsUBO", size_of::<hillshade_layer_ubo::HillshadeEvaluatedPropsUBO>()),
    ("HillshadePrepareDrawableUBO", size_of::<hillshade_prepare_layer_ubo::HillshadePrepareDrawableUBO>()),
    ("HillshadePrepareTilePropsUBO", size_of::<hillshade_prepare_layer_ubo::HillshadePrepareTilePropsUBO>()),
    ("LineDrawableUBO", size_of::<line_layer_ubo::LineDrawableUBO>()),
    ("LineGradientDrawableUBO", size_of::<line_layer_ubo::LineGradientDrawableUBO>()),
    ("LinePatternDrawableUBO", size_of::<line_layer_ubo::LinePatternDrawableUBO>()),
    ("LinePatternTilePropsUBO", size_of::<line_layer_ubo::LinePatternTilePropsUBO>()),
    ("LineSDFDrawableUBO", size_of::<line_layer_ubo::LineSDFDrawableUBO>()),
    ("LineSDFTilePropsUBO", size_of::<line_layer_ubo::LineSDFTilePropsUBO>()),
    ("LineEvaluatedPropsUBO", size_of::<line_layer_ubo::LineEvaluatedPropsUBO>()),
    ("LineDrawableUnionUBO", size_of::<line_layer_ubo::LineDrawableUnionUBO>()),
    ("LineTilePropsUnionUBO", size_of::<line_layer_ubo::LineTilePropsUnionUBO>()),
    ("RasterDrawableUBO", size_of::<raster_layer_ubo::RasterDrawableUBO>()),
    ("RasterEvaluatedPropsUBO", size_of::<raster_layer_ubo::RasterEvaluatedPropsUBO>()),
    ("SymbolDrawableUBO", size_of::<symbol_layer_ubo::SymbolDrawableUBO>()),
    ("SymbolTilePropsUBO", size_of::<symbol_layer_ubo::SymbolTilePropsUBO>()),
    ("SymbolEvaluatedPropsUBO", size_of::<symbol_layer_ubo::SymbolEvaluatedPropsUBO>()),
    ("TerrainDrawableUBO", size_of::<terrain_ubo::TerrainDrawableUBO>()),
    ("TerrainSurfaceUBO", size_of::<terrain_ubo::TerrainSurfaceUBO>()),
];

/// True if `text` declares the struct or union `name`.
fn declares(text: &str, name: &str) -> bool {
    [format!("struct alignas(16) {name} {{"), format!("struct {name} {{"), format!("union {name} {{")]
        .iter()
        .any(|declaration| text.contains(declaration))
}

#[test]
fn ubo_ids_and_sizes_are_the_renderers() {
    let device = device();
    let mut failures = Vec::new();
    for (program, source) in sources() {
        let mut text = full_source(source);
        let mut asserts = String::new();
        for (id, value) in ubo_ids(program) {
            let _ = writeln!(asserts, "static_assert({id} == {value}, \"{id} != {value}\");");
        }
        for &(name, size) in UBO_SIZES {
            if declares(&text, name) {
                let _ = writeln!(asserts, "static_assert(sizeof({name}) == {size}, \"sizeof({name}) != {size}\");");
            }
        }
        text.push_str(&asserts);
        for &projection in ProjectionVariant::of(program) {
            if let Err(error) = compile(&device, &text, &defines(false, projection, []), source.fast_math) {
                failures.push(format!("{} ({projection:?}):\n{error}", program.name));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// The vertex format of `data_type` and its size.
fn vertex_format(data_type: AttributeDataType) -> (MTLVertexFormat, usize) {
    match data_type {
        AttributeDataType::UByte4 => (MTLVertexFormat::UChar4, 4),
        AttributeDataType::Short2 => (MTLVertexFormat::Short2, 4),
        AttributeDataType::Short4 => (MTLVertexFormat::Short4, 8),
        AttributeDataType::UShort2 => (MTLVertexFormat::UShort2, 4),
        AttributeDataType::UShort4 => (MTLVertexFormat::UShort4, 8),
        AttributeDataType::Float => (MTLVertexFormat::Float, 4),
        AttributeDataType::Float2 => (MTLVertexFormat::Float2, 8),
        AttributeDataType::Float3 => (MTLVertexFormat::Float3, 12),
        AttributeDataType::Float4 => (MTLVertexFormat::Float4, 16),
        other => panic!("no vertex format for {other:?}"),
    }
}

/// A vertex descriptor with every attribute of `program` at its location,
/// each in a buffer of its own, from the shader's first vertex buffer on, the
/// instance attributes' buffers stepping per instance.
fn vertex_descriptor(
    program: &Program,
    data_type: impl Fn(AttributeDataType) -> AttributeDataType,
) -> Retained<MTLVertexDescriptor> {
    let descriptor = MTLVertexDescriptor::new();
    let first = first_vertex_buffer(program).unwrap_or(0);
    for (location, step, attribute) in program.locations() {
        let (format, size) = vertex_format(data_type(attribute.format));
        let buffer_index = first + location;
        // SAFETY: the attribute and buffer indices are below 31.
        unsafe {
            let descriptor_attribute = descriptor.attributes().objectAtIndexedSubscript(location);
            descriptor_attribute.setFormat(format);
            descriptor_attribute.setOffset(0);
            descriptor_attribute.setBufferIndex(buffer_index);
            let layout = descriptor.layouts().objectAtIndexedSubscript(buffer_index);
            layout.setStride(size);
            if step == VertexStep::Instance {
                layout.setStepFunction(MTLVertexStepFunction::PerInstance);
            }
        }
    }
    descriptor
}

/// Builds each shader's render pipeline with a buffer of each attribute its
/// program declares: the vertex attributes the MSL reads are all the
/// program's, at their locations, in formats Metal can convert to the MSL's
/// types. The data-driven attributes also come as one value instead of two
/// (source functions instead of composite functions), which leaves the MSL's
/// second component 0.
#[test]
fn pipelines_build_with_the_programs_attributes() {
    let device = device();
    let mut failures = Vec::new();
    for (program, source) in sources() {
        let text = full_source(source);
        let properties: BTreeSet<String> = program.properties.iter().map(|property| property.name.to_owned()).collect();
        assert_eq!(
            uniform_properties(&text),
            properties,
            "{}: the properties its MSL reads from uniforms",
            program.name
        );
        let narrow = |data_type| match data_type {
            AttributeDataType::Float2 => AttributeDataType::Float,
            AttributeDataType::Float4 => AttributeDataType::Float2,
            other => other,
        };
        let layouts: [(&str, Retained<MTLVertexDescriptor>); 2] = [
            ("declared formats", vertex_descriptor(program, |data_type| data_type)),
            ("single-value formats", vertex_descriptor(program, narrow)),
        ];
        let variants = ProjectionVariant::of(program).iter().flat_map(|&projection| {
            [
                (format!("{projection:?}, no uniforms"), defines(false, projection, [])),
                (format!("{projection:?}, all uniforms"), defines(false, projection, &properties)),
            ]
        });
        for (variant, defines) in variants {
            let library = compile(&device, &text, &defines, source.fast_math)
                .unwrap_or_else(|error| panic!("{} ({variant}): {error}", program.name));
            let function = |name: &str| library.newFunctionWithName(&NSString::from_str(name)).unwrap();
            let vertex = function(source.msl.vertex_main_function);
            let fragment = function(source.msl.fragment_main_function);
            // A vertex function without `[[stage_in]]` has no attributes.
            let attributes = vertex.vertexAttributes().map(|attributes| attributes.to_vec()).unwrap_or_default();
            for attribute in attributes.iter().filter(|attribute| attribute.isActive()) {
                if program.attribute_at(attribute.attributeIndex()).is_none() {
                    failures.push(format!(
                        "{} ({variant}): [[attribute({})]] {} isn't the program's",
                        program.name,
                        attribute.attributeIndex(),
                        attribute.name()
                    ));
                }
            }
            for (layout, vertex_descriptor) in &layouts {
                let descriptor = MTLRenderPipelineDescriptor::new();
                descriptor.setVertexFunction(Some(&vertex));
                descriptor.setFragmentFunction(Some(&fragment));
                descriptor.setVertexDescriptor(Some(vertex_descriptor));
                // SAFETY: index 0 of the 8 color attachments.
                unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) }
                    .setPixelFormat(MTLPixelFormat::BGRA8Unorm);
                descriptor.setDepthAttachmentPixelFormat(MTLPixelFormat::Depth32Float_Stencil8);
                descriptor.setStencilAttachmentPixelFormat(MTLPixelFormat::Depth32Float_Stencil8);
                if let Err(error) = device.newRenderPipelineStateWithDescriptor_error(&descriptor) {
                    failures.push(format!("{} ({variant}, {layout}): {}", program.name, error.localizedDescription()));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn the_programs_fit_the_argument_tables() {
    // Metal's argument tables have 31 buffers, 31 vertex attributes and 16
    // samplers.
    for (program, _) in sources() {
        let attributes = program.locations().count();
        assert!(attributes < 31, "{}", program.name);
        assert_eq!(first_vertex_buffer(program).is_some(), attributes > 0, "{}", program.name);
        assert!(first_vertex_buffer(program).is_none_or(|first| first + attributes <= 31), "{}", program.name);
        assert!(program.textures.len() <= 16, "{}", program.name);
    }
}
