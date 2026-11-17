// The background layer in a pattern: an image of the pattern atlas,
// cross-faded between two zoom levels' images.

import package::common::{atlas_pos, get_pattern_pos, glMod2};
import package::globals::{drawable_index, paintParams};
import package::projection::{load_projection, projectTileWithPoles};

struct BackgroundPatternDrawableUBO {
    matrix: mat4x4<f32>,
    pixel_coord_upper: vec2<f32>,
    pixel_coord_lower: vec2<f32>,
    tile_units_to_pixels: f32,
    pad1: f32,
    pad2: f32,
    pad3: f32,
};

struct BackgroundPatternPropsUBO {
    pattern_tl_a: vec2<f32>,
    pattern_br_a: vec2<f32>,
    pattern_tl_b: vec2<f32>,
    pattern_br_b: vec2<f32>,
    pattern_size_a: vec2<f32>,
    pattern_size_b: vec2<f32>,
    scale_a: f32,
    scale_b: f32,
    mix: f32,
    opacity: f32,
};

// ID_BACKGROUND_DRAWABLE_UBO: one element for each drawable of the layer.
@group(1) @binding(2) var<storage, read> drawableVector: array<BackgroundPatternDrawableUBO>;
// ID_BACKGROUND_PROPS_UBO
@group(1) @binding(5) var<uniform> props: BackgroundPatternPropsUBO;

// ID_BACKGROUND_IMAGE_TEXTURE: the pattern atlas.
@group(3) @binding(0) var image: texture_2d<f32>;
@group(3) @binding(3) var image_sampler: sampler;

// The locations are the ID_BACKGROUND_*_VERTEX_ATTRIBUTE ids.
struct VertexInput {
    @location(0) position: vec2<i32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) pos_a: vec2<f32>,
    @location(1) pos_b: vec2<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = drawableVector[drawable_index()];
    let pos = vec2<f32>(in.position);
    out.position = projectTileWithPoles(pos, pos, load_projection());
    out.pos_a = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower,
                                props.scale_a * props.pattern_size_a, drawable.tile_units_to_pixels, pos);
    out.pos_b = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower,
                                props.scale_b * props.pattern_size_b, drawable.tile_units_to_pixels, pos);
    return out;
}

struct FragmentInput {
    @location(0) pos_a: vec2<f32>,
    @location(1) pos_b: vec2<f32>,
};

@fragment
fn fs_main(in: FragmentInput) -> @location(0) vec4<f32> {
    @if(OVERDRAW_INSPECTOR) {
        return vec4<f32>(1.0);
    }
    @else {
        let texsize = paintParams.pattern_atlas_texsize;
        let pos_a = atlas_pos(props.pattern_tl_a / texsize,
                              (props.pattern_br_a - props.pattern_tl_a) / texsize * glMod2(in.pos_a, 1.0));
        let pos_b = atlas_pos(props.pattern_tl_b / texsize,
                              (props.pattern_br_b - props.pattern_tl_b) / texsize * glMod2(in.pos_b, 1.0));
        let color_a = textureSample(image, image_sampler, pos_a);
        let color_b = textureSample(image, image_sampler, pos_b);
        return mix(color_a, color_b, props.mix) * props.opacity;
    }
}
