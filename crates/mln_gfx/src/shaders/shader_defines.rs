// The values were checked against the C++ headers compiled with
// -DMLN_RENDER_BACKEND_WEBGPU=1 (idGlobalUBOIndex = 1, idBackgroundDrawableUBO
// = 2, idProjectionUBO = 4, idBackgroundPropsUBO = 5, layerUBOStartId = 6,
// maxUBOCountPerShader = 7, maxTextureCountPerShader = 3,
// maxAttributeCountPerShader = 11).

// --- layer_ubo.hpp: global UBOs (WebGPU branch) ---
pub const ID_GLOBAL_PAINT_PARAMS_UBO: usize = 0;
pub const ID_GLOBAL_UBO_INDEX: usize = 1;
pub const GLOBAL_UBO_COUNT: usize = 2;

// --- layer SSBOs ---
pub const LAYER_SSBO_START_ID: usize = GLOBAL_UBO_COUNT;
pub const ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO: usize = LAYER_SSBO_START_ID;
pub const ID_DRAWABLE_RESERVED_FRAGMENT_ONLY_UBO: usize = LAYER_SSBO_START_ID + 1;
/// The per-drawable projection block (`ProjectionUBO`), one SSBO per layer.
pub const ID_PROJECTION_UBO: usize = LAYER_SSBO_START_ID + 2;
pub const DRAWABLE_RESERVED_UBO_COUNT: usize = LAYER_SSBO_START_ID + 3;

pub const ID_BACKGROUND_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // SSBO
pub(crate) const BACKGROUND_LAYER_SSBO_COUNT: usize = DRAWABLE_RESERVED_UBO_COUNT;

pub const ID_CIRCLE_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // SSBO
pub(crate) const CIRCLE_LAYER_SSBO_COUNT: usize = DRAWABLE_RESERVED_UBO_COUNT;

pub const ID_FILL_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // SSBO
pub const ID_FILL_TILE_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT; // SSBO
pub(crate) const FILL_LAYER_SSBO_COUNT: usize = ID_FILL_TILE_PROPS_UBO + 1;

pub const ID_LINE_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // SSBO
pub const ID_LINE_TILE_PROPS_UBO: usize = ID_DRAWABLE_RESERVED_FRAGMENT_ONLY_UBO; // SSBO
pub(crate) const LINE_LAYER_SSBO_COUNT: usize = DRAWABLE_RESERVED_UBO_COUNT;

// fill/color-relief/fill-extrusion reserve one more SSBO, so the max of the
// layer SSBO counts is FILL_LAYER_SSBO_COUNT.

// --- layer UBOs ---
pub const LAYER_UBO_START_ID: usize = FILL_LAYER_SSBO_COUNT;
pub const ID_BACKGROUND_PROPS_UBO: usize = BACKGROUND_LAYER_SSBO_COUNT; // packed
pub(crate) const BACKGROUND_LAYER_UBO_COUNT: usize = ID_BACKGROUND_PROPS_UBO + 1;
pub const ID_CIRCLE_EVALUATED_PROPS_UBO: usize = CIRCLE_LAYER_SSBO_COUNT; // packed
pub const CIRCLE_LAYER_UBO_COUNT: usize = ID_CIRCLE_EVALUATED_PROPS_UBO + 1;
pub const ID_FILL_EVALUATED_PROPS_UBO: usize = FILL_LAYER_SSBO_COUNT; // packed
pub const FILL_LAYER_UBO_COUNT: usize = ID_FILL_EVALUATED_PROPS_UBO + 1;
pub const ID_LINE_EVALUATED_PROPS_UBO: usize = LINE_LAYER_SSBO_COUNT; // packed
pub const LINE_LAYER_UBO_COUNT: usize = ID_LINE_EVALUATED_PROPS_UBO + 1;

// --- drawable SSBOs / UBOs ---
pub const BACKGROUND_UBO_COUNT: usize = BACKGROUND_LAYER_UBO_COUNT; // packed

pub const MAX_UBO_COUNT_PER_SHADER: usize = 7;

pub const MAX_TEXTURE_COUNT_PER_SHADER: usize = 3;
pub const MAX_ATTRIBUTE_COUNT_PER_SHADER: usize = 12;

// The vertex buffers that `FillOutlinePulledShader` reads as storage
// buffers, whose ids follow the uniform buffers'.
pub const ID_FILL_OUTLINE_VERTICES: usize = MAX_UBO_COUNT_PER_SHADER;
pub const ID_FILL_OUTLINE_ENDS: usize = MAX_UBO_COUNT_PER_SHADER + 1;
pub const ID_FILL_OUTLINE_PAINT: usize = MAX_UBO_COUNT_PER_SHADER + 2;

// --- conformance: raster, fill-extrusion, hillshade, heatmap, color-relief
// (WebGPU values; the packed SSBO/UBO counts above already include them) ---
pub const ID_RASTER_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // SSBO
pub(crate) const RASTER_LAYER_SSBO_COUNT: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub const ID_RASTER_EVALUATED_PROPS_UBO: usize = RASTER_LAYER_SSBO_COUNT; // packed
pub const RASTER_LAYER_UBO_COUNT: usize = ID_RASTER_EVALUATED_PROPS_UBO + 1;

pub const ID_FILL_EXTRUSION_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // SSBO
pub const ID_FILL_EXTRUSION_TILE_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT; // SSBO
pub(crate) const FILL_EXTRUSION_LAYER_SSBO_COUNT: usize = ID_FILL_EXTRUSION_TILE_PROPS_UBO + 1;
pub const ID_FILL_EXTRUSION_PROPS_UBO: usize = FILL_EXTRUSION_LAYER_SSBO_COUNT; // packed
pub const FILL_EXTRUSION_LAYER_UBO_COUNT: usize = ID_FILL_EXTRUSION_PROPS_UBO + 1;
// The vertex buffers that `FillExtrusionPulledShader` reads as storage
// buffers, whose ids follow the uniform buffers'.
pub const ID_FILL_EXTRUSION_WALL_VERTICES: usize = MAX_UBO_COUNT_PER_SHADER;
pub const ID_FILL_EXTRUSION_WALL_PAINT: usize = MAX_UBO_COUNT_PER_SHADER + 1;
/// Over terrain, the height of the ground under each of the bucket's
/// vertices' polygon's centroid, which the pulled walls read by vertex.
pub const ID_FILL_EXTRUSION_WALL_HEIGHTS: usize = MAX_UBO_COUNT_PER_SHADER + 2;

// --- layer composite (port-only): a layer drawn into a texture, composited
// onto the frame with its `*-layer-opacity` and `*-layer-blend`.
pub const ID_LAYER_COMPOSITE_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT; // packed
pub const LAYER_COMPOSITE_UBO_COUNT: usize = ID_LAYER_COMPOSITE_UBO + 1;

// --- terrain: a tile's grid, raised by the ground's heights by the height
// rule of `mln_util::dem`, and the texture draped over it.
pub const ID_TERRAIN_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // SSBO
/// How the surface stands and the fog over it: a `TerrainSurfaceUBO`.
pub const ID_TERRAIN_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT; // packed
/// What shades a tile's drape: a `HillshadeTilePropsUBO` of the tile's
/// latitudes and the hillshade's values, whose lights shade a drape of a
/// hillshade's slopes, and none for a drape of colors.
pub const ID_TERRAIN_LIGHT_TILE_UBO: usize = ID_DRAWABLE_RESERVED_FRAGMENT_ONLY_UBO; // SSBO
/// The lights of a drape of a hillshade's slopes: its
/// `HillshadeEvaluatedPropsUBO`.
pub const ID_TERRAIN_LIGHT_UBO: usize = ID_TERRAIN_PROPS_UBO + 1; // packed

// --- sky (`idSkyPropsUBO` / `idAtmospherePropsUBO = getEnumValue(drawableReservedUBOCount, layerUBOStartId)`)
pub const ID_SKY_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT; // packed
pub const SKY_LAYER_UBO_COUNT: usize = ID_SKY_PROPS_UBO + 1;
pub const ID_ATMOSPHERE_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT; // packed
pub const ATMOSPHERE_LAYER_UBO_COUNT: usize = ID_ATMOSPHERE_PROPS_UBO + 1;

pub const ID_HEATMAP_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // SSBO
pub(crate) const HEATMAP_LAYER_SSBO_COUNT: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub const ID_HEATMAP_EVALUATED_PROPS_UBO: usize = HEATMAP_LAYER_SSBO_COUNT; // packed
pub const HEATMAP_LAYER_UBO_COUNT: usize = ID_HEATMAP_EVALUATED_PROPS_UBO + 1;
pub const ID_HEATMAP_TEXTURE_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT; // packed

pub const ID_HILLSHADE_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // SSBO
pub const ID_HILLSHADE_TILE_PROPS_UBO: usize = ID_DRAWABLE_RESERVED_FRAGMENT_ONLY_UBO; // SSBO
pub(crate) const HILLSHADE_LAYER_SSBO_COUNT: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub const ID_HILLSHADE_EVALUATED_PROPS_UBO: usize = HILLSHADE_LAYER_SSBO_COUNT; // packed
pub const HILLSHADE_LAYER_UBO_COUNT: usize = ID_HILLSHADE_EVALUATED_PROPS_UBO + 1;
pub const ID_HILLSHADE_PREPARE_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // packed
pub const ID_HILLSHADE_PREPARE_TILE_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT; // packed

pub const ID_COLOR_RELIEF_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // SSBO
pub const ID_COLOR_RELIEF_TILE_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT; // SSBO
pub(crate) const COLOR_RELIEF_LAYER_SSBO_COUNT: usize = ID_COLOR_RELIEF_TILE_PROPS_UBO + 1;
pub const ID_COLOR_RELIEF_EVALUATED_PROPS_UBO: usize = COLOR_RELIEF_LAYER_SSBO_COUNT; // packed
pub const COLOR_RELIEF_LAYER_UBO_COUNT: usize = ID_COLOR_RELIEF_EVALUATED_PROPS_UBO + 1;

// symbols: symbol and collision ids (shader_defines.hpp, packed WebGPU
// enums, MLN_USE_SYMBOL_INSTANCING off).
pub const ID_SYMBOL_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // SSBO
pub const ID_SYMBOL_TILE_PROPS_UBO: usize = ID_DRAWABLE_RESERVED_FRAGMENT_ONLY_UBO; // SSBO
pub(crate) const SYMBOL_LAYER_SSBO_COUNT: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub const ID_SYMBOL_EVALUATED_PROPS_UBO: usize = SYMBOL_LAYER_SSBO_COUNT; // packed
pub const SYMBOL_LAYER_UBO_COUNT: usize = ID_SYMBOL_EVALUATED_PROPS_UBO + 1;
pub const ID_COLLISION_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // packed
pub const ID_COLLISION_TILE_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT; // packed
pub const ID_COLLISION_PROJECTION_UBO: usize = ID_PROJECTION_UBO; // packed
pub const COLLISION_UBO_COUNT: usize = ID_COLLISION_TILE_PROPS_UBO + 1; // packed
