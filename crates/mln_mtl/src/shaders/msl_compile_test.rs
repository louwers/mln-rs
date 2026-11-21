//! Compiles every built-in program's MSL with the system's
//! Metal compiler, the way `MetalGpu` does: `PRELUDE + prelude + source`,
//! MSL 2.4, fast math unless the program's table turns it off, the
//! `ProgramParameters` defines, a
//! `HAS_UNIFORM_u_<name>` define per property passed as a uniform, and
//! `PROJECTION_GLOBE` for the globe variant and `TERRAIN` for the terrain
//! one. Also checks that the UBO ids and sizes the MSL declares are the
//! renderer's, and that the shaders' pipelines build with the vertex layouts
//! their tables describe, in each projection variant.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::mem::size_of;

use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::program::{GLOBE_DEFINE, Program, TERRAIN_DEFINE};
use mln_gfx::shaders::programs::projection_variants;
use mln_gfx::shaders::shader_defines::*;
use mln_gfx::shaders::*;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::{NSDictionary, NSObject, NSString};
use objc2_metal::{
    MTLCompileOptions, MTLCreateSystemDefaultDevice, MTLDevice, MTLFunction, MTLLanguageVersion, MTLLibrary,
    MTLPixelFormat, MTLRenderPipelineDescriptor, MTLVertexDescriptor, MTLVertexFormat, MTLVertexStepFunction,
};

use super::msl_sources::PRELUDE;
use super::shader_source::{AttributeInfo, BUILT_INS, ShaderSource};
use crate::Id;

fn sources() -> impl Iterator<Item = (&'static Program, &'static ShaderSource)> {
    BUILT_INS.iter().copied()
}

/// The projection variants of `program`, by name.
fn projections(program: &Program) -> impl Iterator<Item = (String, ProjectionVariant)> {
    projection_variants(program).iter().map(|&variant| (format!("{variant:?}"), variant))
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

/// ProgramParameters(1.0, overdraw), the define of `projection`, and a
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
    match projection {
        ProjectionVariant::Mercator => {}
        ProjectionVariant::Globe => defines.push((GLOBE_DEFINE.to_owned(), String::new())),
        ProjectionVariant::Terrain => defines.push((TERRAIN_DEFINE.to_owned(), String::new())),
    }
    defines.extend(uniforms.into_iter().map(|name| (format!("HAS_UNIFORM_u_{name}"), String::new())));
    defines
}

/// Every program that draws a layer outputs white for the overdraw
/// inspector, which counts its draws.
#[test]
fn every_layer_program_shows_the_overdraw_inspector() {
    use mln_gfx::shaders::programs::{
        ATMOSPHERE, CLIPPING_MASK, COLLISION_BOX, COLLISION_CIRCLE, GLOBE_DEPTH, SKY, TERRAIN, TERRAIN_PROBE,
        VERTEX_TEXEL,
    };
    // The terrain shows its drapes, where the draped layers count their draws.
    let not_layers = [
        &CLIPPING_MASK,
        &COLLISION_BOX,
        &COLLISION_CIRCLE,
        &GLOBE_DEPTH,
        &SKY,
        &ATMOSPHERE,
        &TERRAIN,
        &TERRAIN_PROBE,
        &VERTEX_TEXEL,
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
        for (projection, variant) in projections(program) {
            // Every property per vertex, every property as a uniform, each one
            // alone as a uniform, and the overdraw inspector.
            let mut variants = vec![
                ("no uniforms".to_owned(), defines(false, variant, [])),
                ("all uniforms".to_owned(), defines(false, variant, &properties)),
                ("overdraw inspector".to_owned(), defines(true, variant, [])),
                ("overdraw inspector, all uniforms".to_owned(), defines(true, variant, &properties)),
            ];
            variants.extend(properties.iter().map(|name| (format!("uniform {name}"), defines(false, variant, [name]))));
            for (variant, defines) in variants {
                match compile(&device, &text, &defines, source.fast_math) {
                    Ok(library) => {
                        compiled += 1;
                        for function in [source.msl.vertex_main_function, source.msl.fragment_main_function] {
                            if library.newFunctionWithName(&NSString::from_str(function)).is_none() {
                                failures.push(format!(
                                    "{} ({projection}, {variant}): no function {function}",
                                    program.name
                                ));
                            }
                        }
                    }
                    Err(error) => failures.push(format!("{} ({projection}, {variant}):\n{error}", program.name)),
                }
            }
        }
    }
    assert!(failures.is_empty(), "{} of {} variants failed:\n{}", failures.len(), compiled + failures.len(), {
        failures.join("\n\n")
    });
}

/// The UBO id enumerators of each shader's prelude, with the renderer's
/// values; the `<layer>UBOCount` enumerator is also where its vertex buffers
/// start.
fn ubo_ids(program: &Program, source: &ShaderSource) -> Vec<(&'static str, usize)> {
    let first_vertex_buffer = source.attributes.iter().map(|attr| attr.buffer_index).min();
    let mut ids = vec![
        ("idGlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        ("idGlobalUBOIndex", ID_GLOBAL_UBO_INDEX),
        ("idDrawableReservedVertexOnlyUBO", ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO),
        ("idDrawableReservedFragmentOnlyUBO", ID_DRAWABLE_RESERVED_FRAGMENT_ONLY_UBO),
        ("idProjectionUBO", ID_PROJECTION_UBO),
        ("drawableReservedUBOCount", DRAWABLE_RESERVED_UBO_COUNT),
    ];
    let (layer, count): (&[(&str, usize)], &str) = match program.name {
        "AtmosphereShader" => (&[("idAtmospherePropsUBO", ID_ATMOSPHERE_PROPS_UBO)], "atmosphereUBOCount"),
        "BackgroundShader" | "BackgroundPatternShader" => (
            &[
                ("idBackgroundDrawableUBO", ID_BACKGROUND_DRAWABLE_UBO),
                ("idBackgroundPropsUBO", ID_BACKGROUND_PROPS_UBO),
            ],
            "backgroundUBOCount",
        ),
        "CircleShader" => (
            &[
                ("idCircleDrawableUBO", ID_CIRCLE_DRAWABLE_UBO),
                ("idCircleEvaluatedPropsUBO", ID_CIRCLE_EVALUATED_PROPS_UBO),
            ],
            "circleUBOCount",
        ),
        "ClippingMaskProgram" => (&[], "clippingMaskUBOCount"),
        "CollisionBoxShader" | "CollisionCircleShader" => (
            &[
                ("idCollisionDrawableUBO", ID_COLLISION_DRAWABLE_UBO),
                ("idCollisionTilePropsUBO", ID_COLLISION_TILE_PROPS_UBO),
                ("idCollisionProjectionUBO", ID_COLLISION_PROJECTION_UBO),
            ],
            "collisionUBOCount",
        ),
        "ColorReliefShader" => (
            &[
                ("idColorReliefDrawableUBO", ID_COLOR_RELIEF_DRAWABLE_UBO),
                ("idColorReliefTilePropsUBO", ID_COLOR_RELIEF_TILE_PROPS_UBO),
                ("idColorReliefEvaluatedPropsUBO", ID_COLOR_RELIEF_EVALUATED_PROPS_UBO),
            ],
            "colorReliefUBOCount",
        ),
        "FillShader"
        | "FillOutlineShader"
        | "FillPatternShader"
        | "FillOutlinePatternShader"
        | "FillOutlineInstancedShader"
        | "FillOutlinePulledShader"
        | "FillOutlinePatternInstancedShader" => (
            &[
                ("idFillDrawableUBO", ID_FILL_DRAWABLE_UBO),
                ("idFillTilePropsUBO", ID_FILL_TILE_PROPS_UBO),
                ("idFillEvaluatedPropsUBO", ID_FILL_EVALUATED_PROPS_UBO),
            ],
            "fillUBOCount",
        ),
        "FillExtrusionShader"
        | "FillExtrusionInstancedShader"
        | "FillExtrusionPulledShader"
        | "FillExtrusionPatternShader"
        | "FillExtrusionPatternInstancedShader" => (
            &[
                ("idFillExtrusionDrawableUBO", ID_FILL_EXTRUSION_DRAWABLE_UBO),
                ("idFillExtrusionTilePropsUBO", ID_FILL_EXTRUSION_TILE_PROPS_UBO),
                ("idFillExtrusionPropsUBO", ID_FILL_EXTRUSION_PROPS_UBO),
            ],
            "fillExtrusionUBOCount",
        ),
        "GlobeDepthShader" => (&[], "globeDepthUBOCount"),
        "HeatmapShader" => (
            &[
                ("idHeatmapDrawableUBO", ID_HEATMAP_DRAWABLE_UBO),
                ("idHeatmapEvaluatedPropsUBO", ID_HEATMAP_EVALUATED_PROPS_UBO),
            ],
            "heatmapUBOCount",
        ),
        "HeatmapTextureShader" => {
            (&[("idHeatmapTexturePropsUBO", ID_HEATMAP_TEXTURE_PROPS_UBO)], "heatmapTextureUBOCount")
        }
        "LayerCompositeShader" => (&[("idLayerCompositeUBO", ID_LAYER_COMPOSITE_UBO)], "layerCompositeUBOCount"),
        "HillshadeShader" => (
            &[
                ("idHillshadeDrawableUBO", ID_HILLSHADE_DRAWABLE_UBO),
                ("idHillshadeTilePropsUBO", ID_HILLSHADE_TILE_PROPS_UBO),
                ("idHillshadeEvaluatedPropsUBO", ID_HILLSHADE_EVALUATED_PROPS_UBO),
            ],
            "hillshadeUBOCount",
        ),
        "HillshadePrepareShader" => (
            &[
                ("idHillshadePrepareDrawableUBO", ID_HILLSHADE_PREPARE_DRAWABLE_UBO),
                ("idHillshadePrepareTilePropsUBO", ID_HILLSHADE_PREPARE_TILE_PROPS_UBO),
            ],
            "hillshadePrepareUBOCount",
        ),
        "LineShader" | "LineGradientShader" | "LinePatternShader" | "LineSDFShader" | "LineGradientSDFShader" => (
            &[
                ("idLineDrawableUBO", ID_LINE_DRAWABLE_UBO),
                ("idLineTilePropsUBO", ID_LINE_TILE_PROPS_UBO),
                ("idLineEvaluatedPropsUBO", ID_LINE_EVALUATED_PROPS_UBO),
            ],
            "lineUBOCount",
        ),
        "RasterShader" => (
            &[
                ("idRasterDrawableUBO", ID_RASTER_DRAWABLE_UBO),
                ("idRasterEvaluatedPropsUBO", ID_RASTER_EVALUATED_PROPS_UBO),
            ],
            "rasterUBOCount",
        ),
        "SkyShader" => (&[("idSkyPropsUBO", ID_SKY_PROPS_UBO)], "skyUBOCount"),
        "VertexTexelShader" => (&[], "vertexTexelUBOCount"),
        "TerrainShader" | "TerrainProbeShader" => (
            &[
                ("idTerrainDrawableUBO", ID_TERRAIN_DRAWABLE_UBO),
                ("idTerrainPropsUBO", ID_TERRAIN_PROPS_UBO),
                ("idTerrainFogUBO", ID_TERRAIN_FOG_UBO),
            ],
            "terrainUBOCount",
        ),
        "SymbolIconShader" | "SymbolSDFShader" | "SymbolTextAndIconShader" => (
            &[
                ("idSymbolDrawableUBO", ID_SYMBOL_DRAWABLE_UBO),
                ("idSymbolTilePropsUBO", ID_SYMBOL_TILE_PROPS_UBO),
                ("idSymbolEvaluatedPropsUBO", ID_SYMBOL_EVALUATED_PROPS_UBO),
            ],
            "symbolUBOCount",
        ),
        name => panic!("no UBO ids for {name}"),
    };
    ids.extend_from_slice(layer);
    if let Some(first_vertex_buffer) = first_vertex_buffer {
        ids.push((count, first_vertex_buffer));
    }
    // The vertex buffers that the pulled shaders read as storage buffers.
    ids.extend_from_slice(match program.name {
        "FillExtrusionPulledShader" => &[
            ("idFillExtrusionWallVertices", ID_FILL_EXTRUSION_WALL_VERTICES),
            ("idFillExtrusionWallPaint", ID_FILL_EXTRUSION_WALL_PAINT),
            ("idFillExtrusionWallCentroids", ID_FILL_EXTRUSION_WALL_CENTROIDS),
        ],
        "FillOutlinePulledShader" => &[
            ("idFillOutlineVertices", ID_FILL_OUTLINE_VERTICES),
            ("idFillOutlineEnds", ID_FILL_OUTLINE_ENDS),
            ("idFillOutlinePaint", ID_FILL_OUTLINE_PAINT),
        ],
        _ => &[],
    });
    ids
}

/// The renderer's UBO structs, by the name the MSL gives them.
const UBO_SIZES: &[(&str, usize)] = &[
    ("GlobalPaintParamsUBO", size_of::<layer_ubo::GlobalPaintParamsUBO>()),
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
    ("TerrainPropsUBO", size_of::<terrain_ubo::TerrainPropsUBO>()),
    ("TerrainFogUBO", size_of::<terrain_ubo::TerrainFogUBO>()),
    ("GroundDrawableUBO", size_of::<terrain_ubo::GroundDrawableUBO>()),
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
        for (id, value) in ubo_ids(program, source) {
            let _ = writeln!(asserts, "static_assert({id} == {value}, \"{id} != {value}\");");
        }
        for &(name, size) in UBO_SIZES {
            if declares(&text, name) {
                let _ = writeln!(asserts, "static_assert(sizeof({name}) == {size}, \"sizeof({name}) != {size}\");");
            }
        }
        // A terrain variant's projection entries carry the ground under its
        // draws.
        let (flat, terrain) = (size_of::<layer_ubo::ProjectionUBO>(), size_of::<terrain_ubo::TerrainProjectionUBO>());
        let _ = writeln!(
            asserts,
            "#if defined(TERRAIN)\nstatic_assert(sizeof(ProjectionUBO) == {terrain}, \"sizeof(ProjectionUBO) != \
             {terrain}\");\n#else\nstatic_assert(sizeof(ProjectionUBO) == {flat}, \"sizeof(ProjectionUBO) != {flat}\");\n#endif"
        );
        text.push_str(&asserts);
        for (projection, variant) in projections(program) {
            if let Err(error) = compile(&device, &text, &defines(false, variant, []), source.fast_math) {
                failures.push(format!("{} ({projection}):\n{error}", program.name));
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

/// The vertex and instance attributes of a table.
fn all_attributes(source: &ShaderSource) -> impl Iterator<Item = &AttributeInfo> {
    source.attributes.iter().chain(source.instance_attributes)
}

/// A vertex descriptor with every attribute of the table, the attributes of
/// a buffer one after the other in its vertices, the instance attributes'
/// buffers stepping per instance.
fn vertex_descriptor(
    source: &ShaderSource,
    data_type: impl Fn(AttributeDataType) -> AttributeDataType,
) -> Retained<MTLVertexDescriptor> {
    let descriptor = MTLVertexDescriptor::new();
    let mut strides = [0usize; 31];
    for attr in all_attributes(source) {
        let (format, size) = vertex_format(data_type(attr.data_type));
        // SAFETY: the attribute and buffer indices are below 31.
        unsafe {
            let attribute = descriptor.attributes().objectAtIndexedSubscript(attr.index);
            attribute.setFormat(format);
            attribute.setOffset(strides[attr.buffer_index]);
            attribute.setBufferIndex(attr.buffer_index);
        }
        strides[attr.buffer_index] += size;
    }
    for (buffer_index, &stride) in strides.iter().enumerate().filter(|(_, stride)| **stride > 0) {
        let per_instance = source.instance_attributes.iter().any(|attr| attr.buffer_index == buffer_index);
        // SAFETY: the buffer index is below 31.
        unsafe {
            let layout = descriptor.layouts().objectAtIndexedSubscript(buffer_index);
            layout.setStride(stride);
            if per_instance {
                layout.setStepFunction(MTLVertexStepFunction::PerInstance);
            }
        }
    }
    descriptor
}

/// Builds each shader's render pipeline with the vertex layout its table
/// describes: the vertex attributes the MSL reads are all in the table, in
/// formats Metal can convert to the MSL's types. The data-driven attributes
/// also come as one value instead of two (source functions instead of
/// composite functions), which leaves the MSL's second component 0.
#[test]
fn pipelines_build_with_the_tables_vertex_layout() {
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
            ("table formats", vertex_descriptor(source, |data_type| data_type)),
            ("single-value formats", vertex_descriptor(source, narrow)),
        ];
        let variants = projections(program).flat_map(|(projection, variant)| {
            [
                (format!("{projection}, no uniforms"), defines(false, variant, [])),
                (format!("{projection}, all uniforms"), defines(false, variant, &properties)),
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
                if !all_attributes(source).any(|attr| attr.index == attribute.attributeIndex()) {
                    failures.push(format!(
                        "{} ({variant}): [[attribute({})]] {} has no table entry",
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
fn attribute_and_texture_slots_are_distinct() {
    // Metal's argument tables have 31 buffers, 31 vertex attributes and 16
    // samplers.
    for (program, source) in sources() {
        let count = all_attributes(source).count();
        let indices: BTreeSet<usize> = all_attributes(source).map(|attr| attr.index).collect();
        assert_eq!(indices.len(), count, "{}: attribute indices repeat", program.name);
        let ids: BTreeSet<usize> = source.attributes.iter().map(|attr| attr.id).collect();
        assert_eq!(ids.len(), source.attributes.len(), "{}: attribute ids repeat", program.name);
        let ids: BTreeSet<usize> = source.instance_attributes.iter().map(|attr| attr.id).collect();
        assert_eq!(ids.len(), source.instance_attributes.len(), "{}: instance attribute ids repeat", program.name);
        assert!(all_attributes(source).all(|attr| attr.index < 31 && attr.buffer_index < 31), "{}", program.name);
        // A buffer steps per vertex or per instance.
        assert!(
            source
                .attributes
                .iter()
                .all(|attr| source.instance_attributes.iter().all(|i| i.buffer_index != attr.buffer_index)),
            "{}: a buffer has vertex and instance attributes",
            program.name
        );
        let textures: BTreeSet<usize> = source.textures.iter().map(|texture| texture.index).collect();
        assert_eq!(textures.len(), source.textures.len(), "{}: texture indices repeat", program.name);
        assert!(source.textures.iter().all(|texture| texture.index < 16), "{}", program.name);
    }
}
