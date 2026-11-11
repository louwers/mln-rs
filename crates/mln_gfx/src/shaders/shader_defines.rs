// Ports include/mln/shaders/shader_defines.hpp and the id enum of
// include/mln/shaders/layer_ubo.hpp, for `MLN_RENDER_BACKEND_WEBGPU`
// (`getEnumValue(packed, unpacked)` = packed).
//
// The values were checked against the C++ headers compiled with
// -DMLN_RENDER_BACKEND_WEBGPU=1 (idGlobalUBOIndex = 1, idBackgroundDrawableUBO
// = 2, idProjectionUBO = 4, idBackgroundPropsUBO = 5, layerUBOStartId = 6,
// maxUBOCountPerShader = 7, maxTextureCountPerShader = 3,
// maxAttributeCountPerShader = 11).
//
// TODO(port): the ids of the other layer types arrive with those layers
// (circle, fill and line: layers agent, M3).

// --- layer_ubo.hpp: global UBOs (WebGPU branch) ---
pub const ID_GLOBAL_PAINT_PARAMS_UBO: usize = 0;
pub const ID_GLOBAL_UBO_INDEX: usize = 1;
pub const GLOBAL_UBO_COUNT: usize = 2;

/// `MLN_UBO_CONSOLIDATION` (Metal, Vulkan, WebGPU).
pub const MLN_UBO_CONSOLIDATION: bool = true;

// --- layer SSBOs ---
pub const LAYER_SSBO_START_ID: usize = GLOBAL_UBO_COUNT;
pub const ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO: usize = LAYER_SSBO_START_ID;
pub const ID_DRAWABLE_RESERVED_FRAGMENT_ONLY_UBO: usize = LAYER_SSBO_START_ID + 1;
/// The per-drawable projection block (`ProjectionUBO`), one SSBO per layer.
pub const ID_PROJECTION_UBO: usize = LAYER_SSBO_START_ID + 2;
pub const DRAWABLE_RESERVED_UBO_COUNT: usize = LAYER_SSBO_START_ID + 3;

pub const ID_BACKGROUND_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // SSBO
pub const BACKGROUND_LAYER_SSBO_COUNT: usize = DRAWABLE_RESERVED_UBO_COUNT;

pub const ID_CIRCLE_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // SSBO
pub const CIRCLE_LAYER_SSBO_COUNT: usize = DRAWABLE_RESERVED_UBO_COUNT;

pub const ID_FILL_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // SSBO
pub const ID_FILL_TILE_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT; // SSBO
pub const FILL_LAYER_SSBO_COUNT: usize = ID_FILL_TILE_PROPS_UBO + 1;

pub const ID_LINE_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // SSBO
pub const ID_LINE_TILE_PROPS_UBO: usize = ID_DRAWABLE_RESERVED_FRAGMENT_ONLY_UBO; // SSBO
pub const LINE_LAYER_SSBO_COUNT: usize = DRAWABLE_RESERVED_UBO_COUNT;

/// `ClippingMask` UBO (`idClippingMaskUBO = idDrawableReservedVertexOnlyUBO`).
pub const ID_CLIPPING_MASK_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO;
/// `idDebugUBO = getEnumValue(drawableReservedUBOCount, ...)` (layers: debug).
pub const ID_DEBUG_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT;

// fill/color-relief/fill-extrusion reserve one more SSBO, so the max of the
// layer SSBO counts is FILL_LAYER_SSBO_COUNT.

// --- layer UBOs ---
pub const LAYER_UBO_START_ID: usize = FILL_LAYER_SSBO_COUNT;
pub const ID_BACKGROUND_PROPS_UBO: usize = BACKGROUND_LAYER_SSBO_COUNT; // packed
pub const BACKGROUND_LAYER_UBO_COUNT: usize = ID_BACKGROUND_PROPS_UBO + 1;
pub const ID_CIRCLE_EVALUATED_PROPS_UBO: usize = CIRCLE_LAYER_SSBO_COUNT; // packed
pub const CIRCLE_LAYER_UBO_COUNT: usize = ID_CIRCLE_EVALUATED_PROPS_UBO + 1;
pub const ID_FILL_EVALUATED_PROPS_UBO: usize = FILL_LAYER_SSBO_COUNT; // packed
pub const FILL_LAYER_UBO_COUNT: usize = ID_FILL_EVALUATED_PROPS_UBO + 1;
pub const ID_LINE_EVALUATED_PROPS_UBO: usize = LINE_LAYER_SSBO_COUNT; // packed
pub const ID_LINE_EXPRESSION_UBO: usize = ID_LINE_EVALUATED_PROPS_UBO + 1;
pub const LINE_LAYER_UBO_COUNT: usize = ID_LINE_EXPRESSION_UBO + 1;

// --- drawable SSBOs / UBOs ---
pub const DRAWABLE_SSBO_START_ID: usize = 7;
pub const DRAWABLE_UBO_START_ID: usize = 7;
pub const BACKGROUND_UBO_COUNT: usize = BACKGROUND_LAYER_UBO_COUNT; // packed

pub const MAX_UBO_COUNT_PER_SHADER: usize = 7;
pub const MAX_SSBO_COUNT_PER_LAYER: usize = LAYER_UBO_START_ID - LAYER_SSBO_START_ID;
pub const MAX_UBO_COUNT_PER_LAYER: usize = DRAWABLE_SSBO_START_ID - LAYER_UBO_START_ID;
pub const MAX_SSBO_COUNT_PER_DRAWABLE: usize = DRAWABLE_UBO_START_ID - DRAWABLE_SSBO_START_ID;
pub const MAX_UBO_COUNT_PER_DRAWABLE: usize = MAX_UBO_COUNT_PER_SHADER - DRAWABLE_UBO_START_ID;

// --- textures ---
pub const ID_BACKGROUND_IMAGE_TEXTURE: usize = 0;
pub const BACKGROUND_TEXTURE_COUNT: usize = 1;
pub const CIRCLE_TEXTURE_COUNT: usize = 0;
pub const ID_FILL_IMAGE_TEXTURE: usize = 0;
pub const FILL_TEXTURE_COUNT: usize = 1;
pub const ID_LINE_IMAGE_TEXTURE: usize = 0;
pub const ID_LINE_DASH_TEXTURE: usize = 1;
/// layers: debug
pub const ID_DEBUG_OVERLAY_TEXTURE: usize = 0;
pub const LINE_TEXTURE_COUNT: usize = 2;
pub const MAX_TEXTURE_COUNT_PER_SHADER: usize = 3;

// --- vertex attributes ---
pub const ID_BACKGROUND_POS_VERTEX_ATTRIBUTE: usize = 0;
pub const BACKGROUND_VERTEX_ATTRIBUTE_COUNT: usize = 1;
pub const ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE: usize = 0;
/// layers: debug
pub const ID_DEBUG_POS_VERTEX_ATTRIBUTE: usize = 0;

pub const ID_CIRCLE_POS_VERTEX_ATTRIBUTE: usize = 0;
// Data driven
pub const ID_CIRCLE_COLOR_VERTEX_ATTRIBUTE: usize = 1;
pub const ID_CIRCLE_RADIUS_VERTEX_ATTRIBUTE: usize = 2;
pub const ID_CIRCLE_BLUR_VERTEX_ATTRIBUTE: usize = 3;
pub const ID_CIRCLE_OPACITY_VERTEX_ATTRIBUTE: usize = 4;
pub const ID_CIRCLE_STROKE_COLOR_VERTEX_ATTRIBUTE: usize = 5;
pub const ID_CIRCLE_STROKE_WIDTH_VERTEX_ATTRIBUTE: usize = 6;
pub const ID_CIRCLE_STROKE_OPACITY_VERTEX_ATTRIBUTE: usize = 7;
pub const CIRCLE_VERTEX_ATTRIBUTE_COUNT: usize = 8;

pub const ID_FILL_POS_VERTEX_ATTRIBUTE: usize = 0;
// Data driven
pub const ID_FILL_COLOR_VERTEX_ATTRIBUTE: usize = 1;
pub const ID_FILL_OPACITY_VERTEX_ATTRIBUTE: usize = 2;
pub const ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE: usize = 3;
pub const ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE: usize = 4;
pub const ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE: usize = 5;
pub const FILL_VERTEX_ATTRIBUTE_COUNT: usize = 6;

pub const ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE: usize = 0;
pub const ID_LINE_DATA_VERTEX_ATTRIBUTE: usize = 1;
// Data driven
pub const ID_LINE_COLOR_VERTEX_ATTRIBUTE: usize = 2;
pub const ID_LINE_BLUR_VERTEX_ATTRIBUTE: usize = 3;
pub const ID_LINE_OPACITY_VERTEX_ATTRIBUTE: usize = 4;
pub const ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE: usize = 5;
pub const ID_LINE_OFFSET_VERTEX_ATTRIBUTE: usize = 6;
pub const ID_LINE_WIDTH_VERTEX_ATTRIBUTE: usize = 7;
pub const ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE: usize = 8;
pub const ID_LINE_PATTERN_FROM_VERTEX_ATTRIBUTE: usize = 9;
pub const ID_LINE_PATTERN_TO_VERTEX_ATTRIBUTE: usize = 10;
pub const LINE_VERTEX_ATTRIBUTE_COUNT: usize = 11;
pub const MAX_ATTRIBUTE_COUNT_PER_SHADER: usize = 11;

// --- conformance: raster, fill-extrusion, hillshade, heatmap, color-relief
// (WebGPU values; the packed SSBO/UBO counts above already include them) ---
pub const ID_RASTER_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // SSBO
pub const RASTER_LAYER_SSBO_COUNT: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub const ID_RASTER_EVALUATED_PROPS_UBO: usize = RASTER_LAYER_SSBO_COUNT; // packed
pub const RASTER_LAYER_UBO_COUNT: usize = ID_RASTER_EVALUATED_PROPS_UBO + 1;
pub const ID_RASTER_IMAGE0_TEXTURE: usize = 0;
pub const ID_RASTER_IMAGE1_TEXTURE: usize = 1;
pub const RASTER_TEXTURE_COUNT: usize = 2;
pub const ID_RASTER_POS_VERTEX_ATTRIBUTE: usize = 0;
pub const ID_RASTER_TEXTURE_POS_VERTEX_ATTRIBUTE: usize = 1;
pub const RASTER_VERTEX_ATTRIBUTE_COUNT: usize = 2;

pub const ID_FILL_EXTRUSION_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // SSBO
pub const ID_FILL_EXTRUSION_TILE_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT; // SSBO
pub const FILL_EXTRUSION_LAYER_SSBO_COUNT: usize = ID_FILL_EXTRUSION_TILE_PROPS_UBO + 1;
pub const ID_FILL_EXTRUSION_PROPS_UBO: usize = FILL_EXTRUSION_LAYER_SSBO_COUNT; // packed
pub const FILL_EXTRUSION_LAYER_UBO_COUNT: usize = ID_FILL_EXTRUSION_PROPS_UBO + 1;
pub const ID_FILL_EXTRUSION_IMAGE_TEXTURE: usize = 0;
pub const FILL_EXTRUSION_TEXTURE_COUNT: usize = 1;
pub const ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE: usize = 0;
pub const ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE: usize = 1;
/// `MLN_USE_FILL_EXTRUSION_INSTANCING`
pub const ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE: usize = 2;
/// Port-only: the next outline vertex of a wall instance, which the WebGPU
/// shaders read as attributes (Metal's read the instance buffer).
pub const ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE: usize = 3;
pub const ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE: usize = 4;
// Data driven
pub const ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE: usize = 5;
pub const ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE: usize = 6;
pub const ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE: usize = 7;
pub const ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE: usize = 8;
pub const ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE: usize = 9;
pub const FILL_EXTRUSION_VERTEX_ATTRIBUTE_COUNT: usize = 10;

pub const ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE: usize = 0;
pub const GLOBE_DEPTH_VERTEX_ATTRIBUTE_COUNT: usize = 1;

// --- sky (`idSkyPropsUBO` / `idAtmospherePropsUBO = getEnumValue(drawableReservedUBOCount, layerUBOStartId)`)
pub const ID_SKY_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT; // packed
pub const SKY_LAYER_UBO_COUNT: usize = ID_SKY_PROPS_UBO + 1;
pub const ID_ATMOSPHERE_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT; // packed
pub const ATMOSPHERE_LAYER_UBO_COUNT: usize = ID_ATMOSPHERE_PROPS_UBO + 1;
pub const ID_SKY_POS_VERTEX_ATTRIBUTE: usize = 0;
pub const SKY_VERTEX_ATTRIBUTE_COUNT: usize = 1;
pub const ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE: usize = 0;
pub const ATMOSPHERE_VERTEX_ATTRIBUTE_COUNT: usize = 1;

pub const ID_HEATMAP_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // SSBO
pub const HEATMAP_LAYER_SSBO_COUNT: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub const ID_HEATMAP_EVALUATED_PROPS_UBO: usize = HEATMAP_LAYER_SSBO_COUNT; // packed
pub const HEATMAP_LAYER_UBO_COUNT: usize = ID_HEATMAP_EVALUATED_PROPS_UBO + 1;
pub const ID_HEATMAP_TEXTURE_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT; // packed
pub const ID_HEATMAP_IMAGE_TEXTURE: usize = 0;
pub const ID_HEATMAP_COLOR_RAMP_TEXTURE: usize = 1;
pub const HEATMAP_TEXTURE_COUNT: usize = 2;
pub const ID_HEATMAP_POS_VERTEX_ATTRIBUTE: usize = 0;
// Data driven
pub const ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE: usize = 1;
pub const ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE: usize = 2;
pub const HEATMAP_VERTEX_ATTRIBUTE_COUNT: usize = 3;

pub const ID_HILLSHADE_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // SSBO
pub const ID_HILLSHADE_TILE_PROPS_UBO: usize = ID_DRAWABLE_RESERVED_FRAGMENT_ONLY_UBO; // SSBO
pub const HILLSHADE_LAYER_SSBO_COUNT: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub const ID_HILLSHADE_EVALUATED_PROPS_UBO: usize = HILLSHADE_LAYER_SSBO_COUNT; // packed
pub const HILLSHADE_LAYER_UBO_COUNT: usize = ID_HILLSHADE_EVALUATED_PROPS_UBO + 1;
pub const ID_HILLSHADE_PREPARE_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // packed
pub const ID_HILLSHADE_PREPARE_TILE_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT; // packed
pub const ID_HILLSHADE_IMAGE_TEXTURE: usize = 0;
pub const HILLSHADE_TEXTURE_COUNT: usize = 1;
pub const ID_HILLSHADE_POS_VERTEX_ATTRIBUTE: usize = 0;
pub const ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE: usize = 1;
pub const HILLSHADE_VERTEX_ATTRIBUTE_COUNT: usize = 2;

pub const ID_COLOR_RELIEF_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // SSBO
pub const ID_COLOR_RELIEF_TILE_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT; // SSBO
pub const COLOR_RELIEF_LAYER_SSBO_COUNT: usize = ID_COLOR_RELIEF_TILE_PROPS_UBO + 1;
pub const ID_COLOR_RELIEF_EVALUATED_PROPS_UBO: usize = COLOR_RELIEF_LAYER_SSBO_COUNT; // packed
pub const COLOR_RELIEF_LAYER_UBO_COUNT: usize = ID_COLOR_RELIEF_EVALUATED_PROPS_UBO + 1;
pub const ID_COLOR_RELIEF_IMAGE_TEXTURE: usize = 0;
pub const ID_COLOR_RELIEF_ELEVATION_STOPS_TEXTURE: usize = 1;
pub const ID_COLOR_RELIEF_COLOR_STOPS_TEXTURE: usize = 2;
pub const COLOR_RELIEF_TEXTURE_COUNT: usize = 3;
pub const ID_COLOR_RELIEF_POS_VERTEX_ATTRIBUTE: usize = 0;
pub const ID_COLOR_RELIEF_TEXTURE_POS_VERTEX_ATTRIBUTE: usize = 1;
pub const COLOR_RELIEF_VERTEX_ATTRIBUTE_COUNT: usize = 2;

// symbols: symbol and collision ids (shader_defines.hpp, packed WebGPU
// enums, MLN_USE_SYMBOL_INSTANCING off).
pub const ID_SYMBOL_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // SSBO
pub const ID_SYMBOL_TILE_PROPS_UBO: usize = ID_DRAWABLE_RESERVED_FRAGMENT_ONLY_UBO; // SSBO
pub const SYMBOL_LAYER_SSBO_COUNT: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub const ID_SYMBOL_EVALUATED_PROPS_UBO: usize = SYMBOL_LAYER_SSBO_COUNT; // packed
pub const SYMBOL_LAYER_UBO_COUNT: usize = ID_SYMBOL_EVALUATED_PROPS_UBO + 1;
pub const ID_COLLISION_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // packed
pub const ID_COLLISION_TILE_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT; // packed
pub const ID_COLLISION_PROJECTION_UBO: usize = ID_PROJECTION_UBO; // packed
pub const COLLISION_UBO_COUNT: usize = ID_COLLISION_TILE_PROPS_UBO + 1; // packed
pub const ID_SYMBOL_IMAGE_TEXTURE: usize = 0;
pub const ID_SYMBOL_IMAGE_ICON_TEXTURE: usize = 1;
pub const SYMBOL_TEXTURE_COUNT: usize = 2;
pub const ID_COLLISION_POS_VERTEX_ATTRIBUTE: usize = 0;
pub const ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE: usize = 1;
pub const ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE: usize = 2;
pub const ID_COLLISION_PLACED_VERTEX_ATTRIBUTE: usize = 3;
pub const ID_COLLISION_SHIFT_VERTEX_ATTRIBUTE: usize = 4;
pub const COLLISION_VERTEX_ATTRIBUTE_COUNT: usize = 5;
pub const ID_SYMBOL_POS_OFFSET_ATTRIBUTE: usize = 0;
pub const ID_SYMBOL_DATA_ATTRIBUTE: usize = 1;
pub const ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE: usize = 2;
pub const ID_SYMBOL_PROJECTED_POS_ATTRIBUTE: usize = 3;
pub const ID_SYMBOL_FADE_OPACITY_ATTRIBUTE: usize = 4;
// Data driven
pub const ID_SYMBOL_OPACITY_ATTRIBUTE: usize = 5;
pub const ID_SYMBOL_COLOR_ATTRIBUTE: usize = 6;
pub const ID_SYMBOL_HALO_COLOR_ATTRIBUTE: usize = 7;
pub const ID_SYMBOL_HALO_WIDTH_ATTRIBUTE: usize = 8;
pub const ID_SYMBOL_HALO_BLUR_ATTRIBUTE: usize = 9;
// Port-only: the quad's atlas origin (`SymbolLayoutVertex::a4`).
pub const ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE: usize = 10;
pub const SYMBOL_ATTRIBUTE_COUNT: usize = 11;
