// The fill extrusion WGSL the port draws with: the roofs, and the walls
// instanced as on Metal (include/mln/shaders/mtl/fill_extrusion.hpp), one
// instance per outline vertex. C++'s WebGPU backend has no instanced walls, so
// these vertex shaders are the port's and `just vendor-shaders` leaves them
// alone; the fragment shaders are the vendored ones (`wgsl_sources`).
//
// A wall instance reads its outline vertex and the next one as attributes,
// the same buffer bound twice, one vertex apart. Where the lowest bit of the
// vertex's `decimals_ed.x` is set, it ends a ring and the instance draws
// nothing. The static quad's x picks the vertex (0) or the next one (1), its y
// the base (0) or the top (1).

use super::shader_source::WgslShaderSource;
use super::wgsl_sources;

/// The vertex output, UBOs and bindings of the plain shaders.
macro_rules! plain_declarations {
    () => {
        r#"
struct VertexOutput {
    @builtin(position) @invariant position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

struct FillExtrusionDrawableUBO {
    matrix: mat4x4<f32>,
    pixel_coord_upper: vec2<f32>,
    pixel_coord_lower: vec2<f32>,
    height_factor: f32,
    tile_ratio: f32,
    base_t: f32,
    height_t: f32,
    color_t: f32,
    pattern_from_t: f32,
    pattern_to_t: f32,
    pad1: f32,
};

struct FillExtrusionPropsUBO {
    color: vec4<f32>,
    light_color_pad: vec4<f32>,
    light_position_base: vec4<f32>,
    height: f32,
    light_intensity: f32,
    vertical_gradient: f32,
    opacity: f32,
    fade: f32,
    from_scale: f32,
    to_scale: f32,
    pad2: f32,
};

struct GlobalIndexUBO {
    value: u32,
    pad0: vec3<u32>,
};

@group(0) @binding(2) var<storage, read> drawableVector: array<FillExtrusionDrawableUBO>;
@group(0) @binding(4) var<storage, read> projectionVector: array<ProjectionUBO>;
@group(0) @binding(6) var<uniform> props: FillExtrusionPropsUBO;
@group(0) @binding(1) var<uniform> globalIndex: GlobalIndexUBO;

// The color of a surface facing `normal`, at `t` (0 at the base, 1 at the
// top) of a wall.
fn lit_color(in_color: vec4<f32>, normal: vec3<f32>, t: f32, baseValue: f32, heightValue: f32) -> vec4<f32> {
    let color = in_color + min(vec4<f32>(0.03, 0.03, 0.03, 1.0), vec4<f32>(1.0));

    let luminance = dot(color.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    let directionalFraction = clamp(dot(normal, props.light_position_base.xyz), 0.0, 1.0);
    let minDirectional = 1.0 - props.light_intensity;
    let maxDirectional = max(1.0 - luminance + props.light_intensity, 1.0);
    var directional = mix(minDirectional, maxDirectional, directionalFraction);

    if (normal.z == 0.0) {
        let gradientMin = mix(0.7, 0.98, 1.0 - props.light_intensity);
        let factor = clamp((t + baseValue) * pow(heightValue / 150.0, 0.5), gradientMin, 1.0);
        directional *= (1.0 - props.vertical_gradient) + props.vertical_gradient * factor;
    }

    let lightColor = props.light_color_pad.xyz;
    let minLight = mix(vec3<f32>(0.0), vec3<f32>(0.3), 1.0 - lightColor);
    let lit = clamp(color.rgb * directional * lightColor, minLight, vec3<f32>(1.0));

    var vcolor = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    vcolor += vec4<f32>(lit, 0.0);
    return vcolor * props.opacity;
}
"#
    };
}

/// The vertex output, UBOs and bindings of the pattern shaders.
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

struct FillExtrusionPatternDrawableUBO {
    matrix: mat4x4<f32>,
    pixel_coord_upper: vec2<f32>,
    pixel_coord_lower: vec2<f32>,
    height_factor: f32,
    tile_ratio: f32,
    base_t: f32,
    height_t: f32,
    color_t: f32,
    pattern_from_t: f32,
    pattern_to_t: f32,
    pad1: f32,
};

struct FillExtrusionPatternTilePropsUBO {
    pattern_from: vec4<f32>,
    pattern_to: vec4<f32>,
    texsize: vec2<f32>,
    pad1: f32,
    pad2: f32,
};

struct FillExtrusionPropsUBO {
    color: vec4<f32>,
    light_color_pad: vec4<f32>,
    light_position_base: vec4<f32>,
    height: f32,
    light_intensity: f32,
    vertical_gradient: f32,
    opacity: f32,
    fade: f32,
    from_scale: f32,
    to_scale: f32,
    pad2: f32,
};

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

struct GlobalIndexUBO {
    value: u32,
    pad0: vec3<u32>,
};

@group(0) @binding(0) var<uniform> paintParams: GlobalPaintParamsUBO;
@group(0) @binding(2) var<storage, read> drawableVector: array<FillExtrusionPatternDrawableUBO>;
@group(0) @binding(4) var<storage, read> projectionVector: array<ProjectionUBO>;
@group(0) @binding(5) var<storage, read> tilePropsVector: array<FillExtrusionPatternTilePropsUBO>;
@group(0) @binding(6) var<uniform> props: FillExtrusionPropsUBO;
@group(0) @binding(1) var<uniform> globalIndex: GlobalIndexUBO;

// The lighting of a surface facing `normal`, at `t` (0 at the base, 1 at the
// top) of a wall.
fn lighting_of(normal: vec3<f32>, t: f32, baseValue: f32, heightValue: f32) -> vec4<f32> {
    var lighting = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    var directional = clamp(dot(normal, props.light_position_base.xyz), 0.0, 1.0);
    directional = mix(1.0 - props.light_intensity, max(0.5 + props.light_intensity, 1.0), directional);

    if (normal.z == 0.0) {
        let gradientMin = mix(0.7, 0.98, 1.0 - props.light_intensity);
        let factor = clamp((t + baseValue) * pow(heightValue / 150.0, 0.5), gradientMin, 1.0);
        directional *= (1.0 - props.vertical_gradient) + props.vertical_gradient * factor;
    }

    let lightColor = props.light_color_pad.xyz;
    let lit = clamp(directional * lightColor,
                    mix(vec3<f32>(0.0), vec3<f32>(0.3), 1.0 - lightColor),
                    vec3<f32>(1.0));
    lighting = vec4<f32>(lighting.rgb + lit, lighting.a);
    return lighting * props.opacity;
}

// Sets the output's pattern positions and data-driven patterns for the point
// `patternPos` of the pattern's plane.
fn set_pattern(out: ptr<function, VertexOutput>, drawable: FillExtrusionPatternDrawableUBO,
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
    let baseValue = max(props.light_position_base.w, 0.0);
#endif

#ifndef HAS_UNIFORM_u_height
    let heightValue = max(unpack_mix_float(in.height, drawable.height_t), 0.0);
#else
    let heightValue = max(props.height, 0.0);
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

/// A wall instance's two outline vertices, `p1` and `p2`, its `normal`, and
/// the corner's `t` and `z`. Returns `out` for an instance that ends a ring.
macro_rules! wall {
    () => {
        r#"
    if (glMod(f32(in.decimals_ed.x), 2.0) > 0.0) {
        return out;
    }

    let p1 = vec2<f32>(in.outline_pos) + unpack_float(f32(in.decimals_ed.x / 2)) / 128.0;
    let p2 = vec2<f32>(in.next_outline_pos) + unpack_float(f32(in.next_decimals_ed.x / 2)) / 128.0;
    let edge = normalize(p2 - p1);
    let normal = vec3<f32>(-edge.y, edge.x, 0.0);

    let t = f32(in.position.y);
    let z = select(baseValue, heightValue, t > 0.0);

    out.position = projectTileFor3D(select(p2, p1, in.position.x == 0), z, projectionVector[globalIndex.value]);
"#
    };
}

/// `ShaderSource<BuiltIn::FillExtrusionShader>`: the roofs.
pub const FILL_EXTRUSION_SHADER: WgslShaderSource = WgslShaderSource {
    built_in: "FillExtrusionShader",
    name: "FillExtrusionShader",
    vertex: Some(concat!(
        r#"
struct VertexInput {
    @location(0) position: vec2<i32>,
    @location(1) decimals_ed: vec2<u32>,
#ifndef HAS_UNIFORM_u_color
    @location(2) color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_base
    @location(3) base: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_height
    @location(4) height: vec2<f32>,
#endif
};
"#,
        plain_declarations!(),
        r#"
@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = drawableVector[globalIndex.value];
"#,
        base_and_height!(),
        r#"
    let decimals = unpack_float(f32(in.decimals_ed.x / 2)) / 128.0;
    out.position = projectTileFor3D(vec2<f32>(in.position) + decimals, heightValue, projectionVector[globalIndex.value]);

#ifdef OVERDRAW_INSPECTOR
    out.color = vec4<f32>(1.0, 1.0, 1.0, 1.0);
    return out;
#endif

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
    fragment: wgsl_sources::FILL_EXTRUSION_SHADER.fragment,
    prelude: None,
};

/// `ShaderSource<BuiltIn::FillExtrusionInstancedShader>`: the walls.
pub const FILL_EXTRUSION_INSTANCED_SHADER: WgslShaderSource = WgslShaderSource {
    built_in: "FillExtrusionInstancedShader",
    name: "FillExtrusionInstancedShader",
    vertex: Some(concat!(
        r#"
struct VertexInput {
    @location(0) position: vec2<i32>,
    @location(1) outline_pos: vec2<i32>,
    @location(2) decimals_ed: vec2<u32>,
    @location(3) next_outline_pos: vec2<i32>,
    @location(4) next_decimals_ed: vec2<u32>,
#ifndef HAS_UNIFORM_u_color
    @location(5) color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_base
    @location(6) base: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_height
    @location(7) height: vec2<f32>,
#endif
};
"#,
        plain_declarations!(),
        r#"
@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = drawableVector[globalIndex.value];
"#,
        base_and_height!(),
        wall!(),
        r#"
#ifdef OVERDRAW_INSPECTOR
    out.color = vec4<f32>(1.0, 1.0, 1.0, 1.0);
    return out;
#endif

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
    fragment: wgsl_sources::FILL_EXTRUSION_SHADER.fragment,
    prelude: None,
};

/// `ShaderSource<BuiltIn::FillExtrusionPatternShader>`: the roofs.
pub const FILL_EXTRUSION_PATTERN_SHADER: WgslShaderSource = WgslShaderSource {
    built_in: "FillExtrusionPatternShader",
    name: "FillExtrusionPatternShader",
    vertex: Some(concat!(
        r#"
struct VertexInput {
    @location(0) position: vec2<i32>,
    @location(1) decimals_ed: vec2<u32>,
#ifndef HAS_UNIFORM_u_base
    @location(2) base: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_height
    @location(3) height: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_pattern_from
    @location(4) pattern_from: vec4<u32>,
#endif
#ifndef HAS_UNIFORM_u_pattern_to
    @location(5) pattern_to: vec4<u32>,
#endif
};
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
        r#"
    let decimals = unpack_float(f32(in.decimals_ed.x / 2)) / 128.0;
    out.position = projectTileFor3D(vec2<f32>(in.position) + decimals, heightValue, projectionVector[globalIndex.value]);

    out.lighting = lighting_of(vec3<f32>(0.0, 0.0, 1.0), 1.0, baseValue, heightValue);
    set_pattern(&out, drawable, patternFrom, patternTo, vec2<f32>(in.position));
    return out;
}
"#
    )),
    fragment: wgsl_sources::FILL_EXTRUSION_PATTERN_SHADER.fragment,
    prelude: None,
};

/// `ShaderSource<BuiltIn::FillExtrusionPatternInstancedShader>`: the walls.
pub const FILL_EXTRUSION_PATTERN_INSTANCED_SHADER: WgslShaderSource = WgslShaderSource {
    built_in: "FillExtrusionPatternInstancedShader",
    name: "FillExtrusionPatternInstancedShader",
    vertex: Some(concat!(
        r#"
struct VertexInput {
    @location(0) position: vec2<i32>,
    @location(1) outline_pos: vec2<i32>,
    @location(2) decimals_ed: vec2<u32>,
    @location(3) next_outline_pos: vec2<i32>,
    @location(4) next_decimals_ed: vec2<u32>,
#ifndef HAS_UNIFORM_u_base
    @location(5) base: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_height
    @location(6) height: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_pattern_from
    @location(7) pattern_from: vec4<u32>,
#endif
#ifndef HAS_UNIFORM_u_pattern_to
    @location(8) pattern_to: vec4<u32>,
#endif
};
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
        wall!(),
        r#"
    // The pattern runs along the outline: p1's side is at the next vertex's
    // edge distance.
    let edgedistance = f32(select(in.decimals_ed.y, in.next_decimals_ed.y, in.position.x == 0));

    out.lighting = lighting_of(normal, t, baseValue, heightValue);
    set_pattern(&out, drawable, patternFrom, patternTo, vec2<f32>(edgedistance, z * drawable.height_factor));
    return out;
}
"#
    )),
    fragment: wgsl_sources::FILL_EXTRUSION_PATTERN_SHADER.fragment,
    prelude: None,
};
