// The global uniform buffer, and the index of a draw's elements in its
// layer's per-drawable buffers.

struct GlobalPaintParamsUBO {
    pattern_atlas_texsize: vec2<f32>,
    units_to_pixels: vec2<f32>,
    world_size: vec2<f32>,
    camera_to_center_distance: f32,
    symbol_fade_change: f32,
    aspect_ratio: f32,
    pixel_ratio: f32,
    map_zoom: f32,
    pad1: f32,
};

// ID_GLOBAL_PAINT_PARAMS_UBO
@group(0) @binding(0) var<uniform> paintParams: GlobalPaintParamsUBO;

// Vulkan pushes the index as a push constant, D3D12 as a root constant.
@if(vulkan || d3d12)
struct DrawableIndex {
    value: u32,
};
@if(vulkan || d3d12)
var<immediate> drawableIndex: DrawableIndex;

// WebGPU binds an entry of a table of indices at the draw's dynamic offset,
// whose entries are this size; Metal sets the index's bytes.
@if(webgpu)
struct GlobalIndexUBO {
    value: u32,
    pad0: vec3<u32>,
};
@if(metal)
struct GlobalIndexUBO {
    value: u32,
};
// ID_GLOBAL_UBO_INDEX
@if(webgpu || metal)
@group(0) @binding(1) var<uniform> globalIndex: GlobalIndexUBO;

// The index of the draw's element in the layer's per-drawable buffers. On
// OpenGL each of those buffers is a uniform block of the draw's one element.
@if(vulkan || d3d12)
fn drawable_index() -> u32 {
    return drawableIndex.value;
}
@elif(webgpu || metal)
fn drawable_index() -> u32 {
    return globalIndex.value;
}
@else
fn drawable_index() -> u32 {
    return 0u;
}
