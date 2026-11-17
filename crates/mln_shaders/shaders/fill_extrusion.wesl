// What the fill extrusion programs share: the layer's buffers, where the
// vertices of roofs and walls go, the lighting of their surfaces, and the
// pattern's color.
//
// A roof is the polygon at the extrusion's height. A wall is a quad from an
// outline vertex to the next one of its ring, which its corner picks: x the
// vertex (0) or the next one (1), y the base (0) or the top (1).

import package::common::{atlas_pos, get_pattern_pos, glMod2, unpack_float};
import package::globals::{drawable_index, paintParams};
import package::projection::{load_projection, projectTileFor3D};

struct FillExtrusionDrawableUBO {
    matrix: mat4x4<f32>,
    pixel_coord_upper: vec2<f32>,
    pixel_coord_lower: vec2<f32>,
    height_factor: f32,
    tile_ratio: f32,
    // Each data-driven property's interpolation between its zoom stops.
    base_t: f32,
    height_t: f32,
    color_t: f32,
    pattern_from_t: f32,
    pattern_to_t: f32,
    pad1: f32,
};

struct FillExtrusionTilePropsUBO {
    pattern_from: vec4<f32>,
    pattern_to: vec4<f32>,
    // The pattern atlas's size.
    texsize: vec2<f32>,
    pad1: f32,
    pad2: f32,
};

struct FillExtrusionPropsUBO {
    color: vec4<f32>,
    light_color: vec3<f32>,
    pad1: f32,
    light_position: vec3<f32>,
    base: f32,
    height: f32,
    light_intensity: f32,
    vertical_gradient: f32,
    opacity: f32,
    // How far the pattern has faded from `pattern_from` to `pattern_to`.
    fade: f32,
    from_scale: f32,
    to_scale: f32,
    pad2: f32,
};

// ID_FILL_EXTRUSION_DRAWABLE_UBO: one element for each drawable of the layer.
@group(1) @binding(2) var<storage, read> drawableVector: array<FillExtrusionDrawableUBO>;
// ID_FILL_EXTRUSION_TILE_PROPS_UBO: one element for each drawable of the layer.
@group(1) @binding(5) var<storage, read> tilePropsVector: array<FillExtrusionTilePropsUBO>;
// ID_FILL_EXTRUSION_PROPS_UBO
@group(1) @binding(6) var<uniform> props: FillExtrusionPropsUBO;

// ID_FILL_EXTRUSION_IMAGE_TEXTURE: the pattern atlas.
@group(3) @binding(0) var image: texture_2d<f32>;
@group(3) @binding(3) var image_sampler: sampler;

fn load_drawable() -> FillExtrusionDrawableUBO {
    return drawableVector[drawable_index()];
}

fn load_tile_props() -> FillExtrusionTilePropsUBO {
    return tilePropsVector[drawable_index()];
}

// Whether the outline vertex of `decimals_ed` ends its ring, and so starts
// no wall.
fn ends_ring(decimals_ed: vec2<u32>) -> bool {
    return (decimals_ed.x & 1u) != 0u;
}

// A vertex's tile position, which its decimals refine to 1/128.
fn vertex_position(position: vec2<i32>, decimals_ed: vec2<u32>) -> vec2<f32> {
    return vec2<f32>(position) + unpack_float(f32(decimals_ed.x >> 1u)) / 128.0;
}

// A roof vertex in clip space, at `height`.
fn roof_position(position: vec2<i32>, decimals_ed: vec2<u32>, height: f32) -> vec4<f32> {
    return projectTileFor3D(vertex_position(position, decimals_ed), height, load_projection());
}

// A corner of a wall from the outline vertex `p1` to the next one, `p2`.
struct WallCorner {
    // In clip space.
    position: vec4<f32>,
    // The wall's normal, in the tile's plane.
    normal: vec3<f32>,
    // 0 at the base, 1 at the top.
    t: f32,
    // The corner's elevation.
    z: f32,
};

fn wall_corner(corner: vec2<i32>, p1: vec2<f32>, p2: vec2<f32>, base: f32, height: f32) -> WallCorner {
    let edge = normalize(p2 - p1);
    let t = f32(corner.y);
    let z = select(base, height, t > 0.0);
    let position = projectTileFor3D(select(p2, p1, corner.x == 0), z, load_projection());
    return WallCorner(position, vec3<f32>(-edge.y, edge.x, 0.0), t, z);
}

// How lit a surface facing `normal` is, `t` of the way up a wall from `base`
// to `height`. `brightest` is the light on a surface that faces the light.
fn directional_light(normal: vec3<f32>, t: f32, base: f32, height: f32, brightest: f32) -> f32 {
    let facing = clamp(dot(normal, props.light_position), 0.0, 1.0);
    var directional = mix(1.0 - props.light_intensity, brightest, facing);
    // Walls darken toward their base, unless the vertical gradient is off.
    if (normal.z == 0.0) {
        let darkest = mix(0.7, 0.98, 1.0 - props.light_intensity);
        let factor = clamp((t + base) * pow(height / 150.0, 0.5), darkest, 1.0);
        directional *= (1.0 - props.vertical_gradient) + props.vertical_gradient * factor;
    }
    return directional;
}

// The least light, which tints the shade with the light color's complement.
fn least_light() -> vec3<f32> {
    return mix(vec3<f32>(0.0), vec3<f32>(0.3), 1.0 - props.light_color);
}

// The premultiplied color of a surface of `color` facing `normal`, `t` of the
// way up a wall from `base` to `height`.
fn lit_color(color: vec4<f32>, normal: vec3<f32>, t: f32, base: f32, height: f32) -> vec4<f32> {
    // The light's range narrows on brighter surfaces.
    let luminance = dot(color.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    let directional = directional_light(normal, t, base, height, max(1.0 - luminance + props.light_intensity, 1.0));
    // A little ambient light, so that no extrusion is black.
    let ambient = color.rgb + vec3<f32>(0.03);
    let lit = clamp(ambient * directional * props.light_color, least_light(), vec3<f32>(1.0));
    return vec4<f32>(lit, 1.0) * props.opacity;
}

// The premultiplied light on a patterned surface facing `normal`, `t` of the
// way up a wall from `base` to `height`, which the pattern's color is
// multiplied by.
fn pattern_lighting(normal: vec3<f32>, t: f32, base: f32, height: f32) -> vec4<f32> {
    let directional = directional_light(normal, t, base, height, max(0.5 + props.light_intensity, 1.0));
    let lit = clamp(directional * props.light_color, least_light(), vec3<f32>(1.0));
    return vec4<f32>(lit, 1.0) * props.opacity;
}

// The positions in the patterns `pattern_from` (xy) and `pattern_to` (zw),
// in pattern sizes, of `pos` on the pattern's plane: the tile position on a
// roof, the distance along the outline and the elevation on a wall.
fn pattern_positions(drawable: FillExtrusionDrawableUBO, pattern_from: vec4<f32>, pattern_to: vec4<f32>,
                     pos: vec2<f32>) -> vec4<f32> {
    let size_from = (pattern_from.zw - pattern_from.xy) / paintParams.pixel_ratio;
    let size_to = (pattern_to.zw - pattern_to.xy) / paintParams.pixel_ratio;
    let pos_from = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower,
                                   props.from_scale * size_from, drawable.tile_ratio, pos);
    let pos_to = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower,
                                 props.to_scale * size_to, drawable.tile_ratio, pos);
    return vec4<f32>(pos_from, pos_to);
}

// The color of the patterns `pattern_from` and `pattern_to` at the positions
// `pattern_positions` gave, faded from one to the other.
fn pattern_color(positions: vec4<f32>, pattern_from: vec4<f32>, pattern_to: vec4<f32>,
                 texsize: vec2<f32>) -> vec4<f32> {
    let pos_from = atlas_pos(pattern_from.xy / texsize,
                             (pattern_from.zw - pattern_from.xy) / texsize * glMod2(positions.xy, 1.0));
    let pos_to = atlas_pos(pattern_to.xy / texsize,
                           (pattern_to.zw - pattern_to.xy) / texsize * glMod2(positions.zw, 1.0));
    let color_from = textureSample(image, image_sampler, pos_from);
    let color_to = textureSample(image, image_sampler, pos_to);
    return mix(color_from, color_to, props.fade);
}
