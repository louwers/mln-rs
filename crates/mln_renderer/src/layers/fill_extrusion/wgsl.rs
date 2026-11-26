//! The fill extrusions' WGSL: the roofs, and the walls instanced as on
//! Metal, one instance per outline vertex.
//!
//! A wall instance reads its outline vertex and the next one as attributes,
//! the same buffer bound twice, one vertex apart. Where the lowest bit of the
//! vertex's `decimals_ed.x` is set, it ends a ring and the instance draws
//! nothing. Where its next bit is set, the walls on either side of the vertex
//! share the normal whose angle the remaining bits hold. The static quad's x picks the vertex (0) or the next one (1), its y
//! the base (0) or the top (1). The wall shaders run the same vertex stages
//! with every input per vertex, for walls whose quads are in the vertex
//! buffer.
//!
//! The pulled walls draw four vertices a wall without vertex attributes. A
//! vertex's index picks its wall and its corner of the quad, and the wall's
//! outline vertex, the next one and its paint values come from the buffers
//! that hold them for the roofs, bound as storage buffers.

use mln_gfx::shaders::source::WgslSource;

use crate::layers::pattern::wgsl::pattern_pos;

/// The pattern roofs' and walls' fragment stage.
const FILL_EXTRUSION_PATTERN_FRAGMENT: &str = r"
struct FragmentInput {
    @location(0) lighting: vec4<f32>,
    @location(1) pos_a: vec2<f32>,
    @location(2) pos_b: vec2<f32>,
#ifndef HAS_UNIFORM_u_pattern_from
    @location(3) @interpolate(flat) pattern_from: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_pattern_to
    @location(4) @interpolate(flat) pattern_to: vec4<f32>,
#endif
};

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
    let tileProps = tilePropsVector[globalIndex.value];

#ifdef HAS_UNIFORM_u_pattern_from
    let pattern_from = tileProps.pattern_from;
#else
    let pattern_from = in.pattern_from;
#endif

#ifdef HAS_UNIFORM_u_pattern_to
    let pattern_to = tileProps.pattern_to;
#else
    let pattern_to = in.pattern_to;
#endif

    let pattern_tl_a = pattern_from.xy;
    let pattern_br_a = pattern_from.zw;
    let pattern_tl_b = pattern_to.xy;
    let pattern_br_b = pattern_to.zw;

    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    var color_a = vec4<f32>(0.0);
    if (props.fade < 1.0) {
        let imagecoord_a = gl_mod(in.pos_a, vec2<f32>(1.0));
        let pos_a = atlas_pos(pattern_tl_a / tileProps.texsize, (pattern_br_a - pattern_tl_a) / tileProps.texsize * imagecoord_a);
        color_a = textureSample(image, image_sampler, pos_a);
    }
    var color_b = vec4<f32>(0.0);
    if (props.fade > 0.0) {
        let imagecoord_b = gl_mod(in.pos_b, vec2<f32>(1.0));
        let pos_b = atlas_pos(pattern_tl_b / tileProps.texsize, (pattern_br_b - pattern_tl_b) / tileProps.texsize * imagecoord_b);
        color_b = textureSample(image, image_sampler, pos_b);
    }

    let pattern_color = mix(color_a, color_b, props.fade);
    return pattern_color * in.lighting;
}
";

/// The plain roofs' and walls' fragment stage.
const FILL_EXTRUSION_FRAGMENT: &str = r"
struct FragmentInput {
    @location(0) color: vec4<f32>,
};

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
    return in.color;
}
";

/// The vertex output and the lighting of the plain shaders.
macro_rules! plain_declarations {
    () => {
        r#"
struct VertexOutput {
    @builtin(position) @invariant position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

// The color of a surface facing `normal`, at `t` (0 at the base, 1 at the
// top) of a wall.
fn lit_color(in_color: vec4<f32>, normal: vec3<f32>, t: f32, baseValue: f32, heightValue: f32) -> vec4<f32> {
    // The surface's own luminance, before the ambient light that keeps it
    // from going black.
    let luminance = dot(in_color.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    let color = in_color + vec4<f32>(0.03, 0.03, 0.03, 1.0);

    let directionalFraction = clamp(dot(normal, props.light_position), 0.0, 1.0);
    let minDirectional = 1.0 - props.light_intensity;
    let maxDirectional = max(1.0 - luminance + props.light_intensity, 1.0);
    var directional = mix(minDirectional, maxDirectional, directionalFraction);

    if (normal.z == 0.0) {
        let gradientMin = mix(0.7, 0.98, 1.0 - props.light_intensity);
        let factor = clamp((t + baseValue) * pow(heightValue / 150.0, 0.5), gradientMin, 1.0);
        directional *= (1.0 - props.vertical_gradient) + props.vertical_gradient * factor;
    }

    let lightColor = props.light_color;
    let minLight = mix(vec3<f32>(0.0), vec3<f32>(0.3), 1.0 - lightColor);
    let lit = clamp(color.rgb * directional * lightColor, minLight, vec3<f32>(1.0));

    var vcolor = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    vcolor += vec4<f32>(lit, 0.0);
    return vcolor * props.opacity;
}
"#
    };
}

/// The vertex output, the lighting and the pattern of the pattern shaders.
macro_rules! pattern_declarations {
    () => {
        r#"
struct VertexOutput {
    @builtin(position) @invariant position: vec4<f32>,
    @location(0) lighting: vec4<f32>,
    @location(1) pos_a: vec2<f32>,
    @location(2) pos_b: vec2<f32>,
#ifndef HAS_UNIFORM_u_pattern_from
    @location(3) @interpolate(flat) pattern_from: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_pattern_to
    @location(4) @interpolate(flat) pattern_to: vec4<f32>,
#endif
};

// The lighting of a surface facing `normal`, at `t` (0 at the base, 1 at the
// top) of a wall.
fn lighting_of(normal: vec3<f32>, t: f32, baseValue: f32, heightValue: f32) -> vec4<f32> {
    var lighting = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    var directional = clamp(dot(normal, props.light_position), 0.0, 1.0);
    directional = mix(1.0 - props.light_intensity, max(0.5 + props.light_intensity, 1.0), directional);

    if (normal.z == 0.0) {
        let gradientMin = mix(0.7, 0.98, 1.0 - props.light_intensity);
        let factor = clamp((t + baseValue) * pow(heightValue / 150.0, 0.5), gradientMin, 1.0);
        directional *= (1.0 - props.vertical_gradient) + props.vertical_gradient * factor;
    }

    let lightColor = props.light_color;
    let lit = clamp(directional * lightColor,
                    mix(vec3<f32>(0.0), vec3<f32>(0.3), 1.0 - lightColor),
                    vec3<f32>(1.0));
    lighting = vec4<f32>(lighting.rgb + lit, lighting.a);
    return lighting * props.opacity;
}

// Sets the output's pattern positions and data-driven patterns for the point
// `patternPos` of the pattern's plane.
fn set_pattern(out: ptr<function, VertexOutput>, drawable: FillExtrusionDrawableUBO,
               patternFrom: vec4<f32>, patternTo: vec4<f32>, patternPos: vec2<f32>) {
#ifndef HAS_UNIFORM_u_pattern_from
    (*out).pattern_from = patternFrom;
#endif
#ifndef HAS_UNIFORM_u_pattern_to
    (*out).pattern_to = patternTo;
#endif

    let pixelRatio = paintParams.pixel_ratio;
    let display_size_a = vec2<f32>((patternFrom.z - patternFrom.x) / pixelRatio,
                                   (patternFrom.w - patternFrom.y) / pixelRatio);
    let display_size_b = vec2<f32>((patternTo.z - patternTo.x) / pixelRatio,
                                   (patternTo.w - patternTo.y) / pixelRatio);

    (*out).pos_a = get_pattern_pos(drawable.pixel_coord_upper,
                                   drawable.pixel_coord_lower,
                                   props.from_scale * display_size_a,
                                   drawable.tile_ratio,
                                   patternPos);
    (*out).pos_b = get_pattern_pos(drawable.pixel_coord_upper,
                                   drawable.pixel_coord_lower,
                                   props.to_scale * display_size_b,
                                   drawable.tile_ratio,
                                   patternPos);
}
"#
    };
}

/// The paint values a vertex shader starts from: `baseValue` and
/// `heightValue`, of the attributes or the props.
macro_rules! base_and_height {
    () => {
        r#"
#ifndef HAS_UNIFORM_u_base
    let baseValue = max(unpack_mix_float(in.base, drawable.base_t), 0.0);
#else
    let baseValue = max(props.base, 0.0);
#endif

#ifndef HAS_UNIFORM_u_height
    let heightValue = max(unpack_mix_float(in.height, drawable.height_t), 0.0);
#else
    let heightValue = max(props.height, 0.0);
#endif
"#
    };
}

/// The heights the extrusion's base and roof stand at, `baseZ` and
/// `heightZ`: over terrain, raised by `in.ground_height`, the ground's under
/// its polygon's centroid, as GL JS raises them, and its base 10 m lower
/// where it is at the ground, so that it meets a slope; else `baseValue` and
/// `heightValue`.
macro_rules! stand {
    () => {
        r#"
#ifdef TERRAIN
    let ele = in.ground_height;
    let baseZ = baseValue + ele - select(10.0, 0.0, baseValue > 0.0);
    let heightZ = heightValue + ele;
#else
    let baseZ = baseValue;
    let heightZ = heightValue;
#endif
"#
    };
}

/// The pattern positions in the atlas, of the attributes or the tile props.
macro_rules! patterns {
    () => {
        r#"
#ifdef HAS_UNIFORM_u_pattern_from
    let patternFrom = tilePropsVector[globalIndex.value].pattern_from;
#else
    let patternFrom = vec4<f32>(in.pattern_from);
#endif

#ifdef HAS_UNIFORM_u_pattern_to
    let patternTo = tilePropsVector[globalIndex.value].pattern_to;
#else
    let patternTo = vec4<f32>(in.pattern_to);
#endif
"#
    };
}

/// `wall_normal`, the normal at a wall's end from its outline vertex's
/// `decimals_ed.x`: the one the walls there share, or else `own`, the
/// wall's own.
macro_rules! wall_normal {
    () => {
        r#"
fn wall_normal(bits: u32, own: vec2<f32>) -> vec2<f32> {
    if ((bits & 2u) == 0u) {
        return own;
    }
    let angle = f32(bits >> 2u) * (6.28318530718 / 16384.0) - 3.14159265359;
    return vec2<f32>(cos(angle), sin(angle));
}
"#
    };
}

/// A wall instance's two outline vertices, `p1` and `p2`, the corner's
/// `normal`, `t` and `z`. Returns `out` for an instance that ends a ring.
macro_rules! wall {
    () => {
        r#"
    if (glMod(f32(in.decimals_ed.x), 2.0) > 0.0) {
        return out;
    }

    let p1 = vec2<f32>(in.outline_pos);
    let p2 = vec2<f32>(in.next_outline_pos);
    let edge = normalize(p2 - p1);
    let own = vec2<f32>(-edge.y, edge.x);
    let normal = vec3<f32>(
        select(wall_normal(in.next_decimals_ed.x, own), wall_normal(in.decimals_ed.x, own), in.pos.x == 0),
        0.0,
    );

    let t = f32(in.pos.y);
    let z = select(baseZ, heightZ, t > 0.0);

    out.position = projectTileFor3D(select(p2, p1, in.pos.x == 0), z, projectionVector[globalIndex.value]);
"#
    };
}

/// The fill extrusion roofs.
pub(crate) const FILL_EXTRUSION: WgslSource = WgslSource {
    vertex: Some(concat!(
        r#"
"#,
        plain_declarations!(),
        r#"
@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = drawableVector[globalIndex.value];
"#,
        base_and_height!(),
        stand!(),
        r#"
    out.position = projectTileFor3D(vec2<f32>(in.pos), heightZ, projectionVector[globalIndex.value]);

#ifndef HAS_UNIFORM_u_color
    let color = unpack_mix_color(in.color, drawable.color_t);
#else
    let color = props.color;
#endif
    out.color = lit_color(color, vec3<f32>(0.0, 0.0, 1.0), 1.0, baseValue, heightValue);
    return out;
}
"#
    )),
    fragment: Some(FILL_EXTRUSION_FRAGMENT),
    prelude: None,
};

/// The instanced fill extrusion walls.
pub(crate) const FILL_EXTRUSION_INSTANCED: WgslSource = WgslSource {
    vertex: Some(concat!(
        r#"
"#,
        plain_declarations!(),
        wall_normal!(),
        r#"
@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = drawableVector[globalIndex.value];
"#,
        base_and_height!(),
        stand!(),
        wall!(),
        r#"

#ifndef HAS_UNIFORM_u_color
    let color = unpack_mix_color(in.color, drawable.color_t);
#else
    let color = props.color;
#endif
    out.color = lit_color(color, normal, t, baseValue, heightValue);
    return out;
}
"#
    )),
    fragment: Some(FILL_EXTRUSION_FRAGMENT),
    prelude: None,
};

/// The pulled fill extrusion walls.
pub(crate) const FILL_EXTRUSION_PULLED: WgslSource = WgslSource {
    vertex: Some(concat!(
        r#"
// What the instanced walls read as attributes.
struct VertexInput {
    pos: vec2<i32>,
    outline_pos: vec2<i32>,
    decimals_ed: vec2<u32>,
    next_outline_pos: vec2<i32>,
    next_decimals_ed: vec2<u32>,
#ifndef HAS_UNIFORM_u_color
    color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_base
    base: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_height
    height: vec2<f32>,
#endif
#ifdef TERRAIN
    ground_height: f32,
#endif
};
"#,
        plain_declarations!(),
        wall_normal!(),
        r#"
// `wallVertices` holds the bucket's vertices, two words each: the
// position's x and y in the halves of the first, `decimals_ed`'s in those
// of the second. Over terrain, `wallHeights` holds the height of the ground
// under each of the bucket's vertices' polygon's centroid.
@vertex
fn main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var out: VertexOutput;
    let drawable = drawableVector[globalIndex.value];

    // Four vertices a wall, in the order of the static quad's corners.
    let wall = vertex_index / 4u;
    let corner = vertex_index % 4u;
    let vertex = 2u * wall;
    let pos = wallVertices[vertex];
    let decimals_ed = wallVertices[vertex + 1u];
    let next_pos = wallVertices[vertex + 2u];
    let next_decimals_ed = wallVertices[vertex + 3u];

    var in: VertexInput;
    in.pos = vec2<i32>(i32(corner < 2u), i32(corner & 1u));
    in.outline_pos = vec2<i32>(i32(pos << 16u) >> 16u, i32(pos) >> 16u);
    in.decimals_ed = vec2<u32>(decimals_ed & 0xffffu, decimals_ed >> 16u);
    in.next_outline_pos = vec2<i32>(i32(next_pos << 16u) >> 16u, i32(next_pos) >> 16u);
    in.next_decimals_ed = vec2<u32>(next_decimals_ed & 0xffffu, next_decimals_ed >> 16u);
#ifdef TERRAIN
    in.ground_height = wallHeights[wall];
#endif

    // The wall's paint values are its outline vertex's.
    let paint_layout = drawable.paint_layout;
    let paint = wall * (paint_layout & 0xffu);
#ifndef HAS_UNIFORM_u_base
    let base_at = paint + ((paint_layout >> 8u) & 0xffu);
    in.base = vec2<f32>(wallPaint[base_at], wallPaint[base_at + 1u]);
#endif
#ifndef HAS_UNIFORM_u_height
    let height_at = paint + ((paint_layout >> 16u) & 0xffu);
    in.height = vec2<f32>(wallPaint[height_at], wallPaint[height_at + 1u]);
#endif
#ifndef HAS_UNIFORM_u_color
    let color_at = paint + (paint_layout >> 24u);
    in.color = vec4<f32>(wallPaint[color_at], wallPaint[color_at + 1u],
                         wallPaint[color_at + 2u], wallPaint[color_at + 3u]);
#endif
"#,
        base_and_height!(),
        stand!(),
        wall!(),
        r#"

#ifndef HAS_UNIFORM_u_color
    let color = unpack_mix_color(in.color, drawable.color_t);
#else
    let color = props.color;
#endif
    out.color = lit_color(color, normal, t, baseValue, heightValue);
    return out;
}
"#
    )),
    fragment: Some(FILL_EXTRUSION_FRAGMENT),
    prelude: None,
};

/// The fill extrusion pattern roofs.
pub(crate) const FILL_EXTRUSION_PATTERN: WgslSource = WgslSource {
    vertex: Some(concat!(
        pattern_pos!(),
        r#"
"#,
        pattern_declarations!(),
        r#"
@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = drawableVector[globalIndex.value];
"#,
        base_and_height!(),
        patterns!(),
        stand!(),
        r#"
    out.position = projectTileFor3D(vec2<f32>(in.pos), heightZ, projectionVector[globalIndex.value]);

    out.lighting = lighting_of(vec3<f32>(0.0, 0.0, 1.0), 1.0, baseValue, heightValue);
    set_pattern(&out, drawable, patternFrom, patternTo, vec2<f32>(in.pos));
    return out;
}
"#
    )),
    fragment: Some(FILL_EXTRUSION_PATTERN_FRAGMENT),
    prelude: None,
};

/// The instanced fill extrusion pattern walls.
pub(crate) const FILL_EXTRUSION_PATTERN_INSTANCED: WgslSource = WgslSource {
    vertex: Some(concat!(
        pattern_pos!(),
        r#"
"#,
        pattern_declarations!(),
        wall_normal!(),
        r#"
@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = drawableVector[globalIndex.value];
"#,
        base_and_height!(),
        patterns!(),
        stand!(),
        wall!(),
        r#"
    // The pattern runs along the outline: p1's side is at the next vertex's
    // edge distance.
    let edgedistance = f32(select(in.decimals_ed.y, in.next_decimals_ed.y, in.pos.x == 0));

    out.lighting = lighting_of(normal, t, baseValue, heightValue);
    set_pattern(&out, drawable, patternFrom, patternTo, vec2<f32>(edgedistance, z * drawable.height_factor));
    return out;
}
"#
    )),
    fragment: Some(FILL_EXTRUSION_PATTERN_FRAGMENT),
    prelude: None,
};
