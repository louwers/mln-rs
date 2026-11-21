//! The GLSL ES preludes of every shader, and how a stage composes them with
//! the blocks it reads and the program's GLSL ES.

use std::ptr;

use mln_gfx::gpu::{GpuError, ShaderKey};
use mln_gfx::shaders::block::Block;
use mln_gfx::shaders::program::PROJECTION_BUFFER;
use mln_gfx::shaders::source::glsl_es::{self, Glsl};
use mln_gfx::shaders::source::identifiers;
use mln_util::hash::HashSet;

/// The default precisions of each stage, and on desktop OpenGL, which has
/// none, no precision qualifiers.
const PRECISION: Glsl = Glsl {
    vertex: r"#ifdef GL_ES
precision highp float;
#else

#if !defined(lowp)
#define lowp
#endif

#if !defined(mediump)
#define mediump
#endif

#if !defined(highp)
#define highp
#endif

#endif
",
    fragment: r"#ifdef GL_ES
#ifdef HIGHP_FRAGMENT
precision highp float;
#else
precision mediump float;
#endif
#else

#if !defined(lowp)
#define lowp
#endif

#if !defined(mediump)
#define mediump
#endif

#if !defined(highp)
#define highp
#endif

#endif

",
};

/// The helpers every stage compiles after.
const PRELUDE: Glsl = Glsl {
    vertex: r"
const float PI = 3.141592653589793;

// Unpack a pair of values that have been packed into a single float.
// The packed values are assumed to be 8-bit unsigned integers, and are
// packed like so:
// packedValue = floor(input[0]) * 256 + input[1],
vec2 unpack_float(const float packedValue) {
    int packedIntValue = int(packedValue);
    int v0 = packedIntValue / 256;
    return vec2(v0, packedIntValue - v0 * 256);
}

// To minimize the number of attributes needed, we encode a 4-component
// color into a pair of floats (i.e. a vec2) as follows:
// [ floor(color.r * 255) * 256 + color.g * 255,
//   floor(color.b * 255) * 256 + color.a * 255 ]
vec4 decode_color(const vec2 encodedColor) {
    return vec4(
        unpack_float(encodedColor[0]) / 255.0,
        unpack_float(encodedColor[1]) / 255.0
    );
}

// Unpack a pair of paint values and interpolate between them.
float unpack_mix_vec2(const vec2 packedValue, const float t) {
    return mix(packedValue[0], packedValue[1], t);
}

// Unpack a pair of paint values and interpolate between them.
vec4 unpack_mix_color(const vec4 packedColors, const float t) {
    vec4 minColor = decode_color(vec2(packedColors[0], packedColors[1]));
    vec4 maxColor = decode_color(vec2(packedColors[2], packedColors[3]));
    return mix(minColor, maxColor, t);
}

// The offset depends on how many pixels are between the world origin and the edge of the tile:
// vec2 offset = mod(pixel_coord, size)
//
// At high zoom levels there are a ton of pixels between the world origin and the edge of the tile.
// The glsl spec only guarantees 16 bits of precision for highp floats. We need more than that.
//
// The pixel_coord is passed in as two 16 bit values:
// pixel_coord_upper = floor(pixel_coord / 2^16)
// pixel_coord_lower = mod(pixel_coord, 2^16)
//
// The offset is calculated in a series of steps that should preserve this precision:
vec2 get_pattern_pos(const vec2 pixel_coord_upper, const vec2 pixel_coord_lower,
    const vec2 pattern_size, const float tile_units_to_pixels, const vec2 pos) {

    vec2 offset = mod(mod(mod(pixel_coord_upper, pattern_size) * 256.0, pattern_size) * 256.0 + pixel_coord_lower, pattern_size);
    return (tile_units_to_pixels * pos + offset) / pattern_size;
}
",
    fragment: r"out highp vec4 fragColor;

// The atlas texture coordinate of an image's origin plus an offset from it,
// both normalized by a power-of-two atlas size. Snapping the offset to 2^-24
// makes the sum exact, so the sampled position, and the pixels, don't depend
// on where the image sits in the atlas. Adding an unsnapped offset rounds it
// to a precision that depends on the origin.
highp vec2 atlas_pos(highp vec2 origin, highp vec2 offset) {
    return origin + roundEven(offset * 16777216.0) / 16777216.0;
}
",
};

/// The helpers of a vertex stage that projects, which read the projection
/// block: they follow [`PRELUDE`]'s.
const PROJECTION_PRELUDE: &str = r"
// Pole vertices carry these sentinel Y values in their raw position.
#define GLOBE_POLE_NORTH_Y -32767.5
#define GLOBE_POLE_SOUTH_Y 32766.5

#ifdef PROJECTION_GLOBE

#define GLOBE_RADIUS 6371008.8
#define GLOBE_PI 3.1415926535897932384626433832795

// Tile position (0..EXTENT) to a point on the unit sphere; the pole sentinels in rawPos map to the poles.
vec3 projectToSphere(vec2 translatedPos, vec2 rawPos) {
    vec2 mercator_pos = u_projection_tile_mercator_coords.xy + u_projection_tile_mercator_coords.zw * translatedPos;
    float spherical_x = mercator_pos.x * GLOBE_PI * 2.0 + GLOBE_PI;
    // sin/cos of the latitude from the Mercator Y via the tangent half-angle identities: no atan, and float32 precision survives near the equator.
    float t = exp(GLOBE_PI - (mercator_pos.y * GLOBE_PI * 2.0));
    float t2 = t * t;
    float denom = t2 + 1.0;
    float sin_sy = (t2 - 1.0) / denom;
    float cos_sy = (2.0 * t) / denom;
    vec3 pos = vec3(sin(spherical_x) * cos_sy, sin_sy, cos(spherical_x) * cos_sy);
    if (rawPos.y < GLOBE_POLE_NORTH_Y) {
        pos = vec3(0.0, 1.0, 0.0);
    }
    if (rawPos.y > GLOBE_POLE_SOUTH_Y) {
        pos = vec3(0.0, -1.0, 0.0);
    }
    return pos;
}

vec3 globeRotateVector(vec3 vec, vec2 angles) {
    vec3 axisRight = vec3(vec.z, 0.0, -vec.x);
    vec3 axisUp = cross(axisRight, vec);
    axisRight = normalize(axisRight);
    axisUp = normalize(axisUp);
    vec2 t = tan(angles);
    return normalize(vec + axisRight * t.x + axisUp * t.y);
}

// cos(latitude) at a tile Y, from the same exp() form as projectToSphere.
float circumferenceRatioAtTileY(float tileY) {
    float mercator_pos_y = u_projection_tile_mercator_coords.y + u_projection_tile_mercator_coords.w * tileY;
    float t = exp(GLOBE_PI - (mercator_pos_y * GLOBE_PI * 2.0));
    return (2.0 * t) / (t * t + 1.0);
}

float globeComputeClippingZ(vec3 spherePos) {
    return 1.0 - (dot(spherePos, u_projection_clipping_plane.xyz) + u_projection_clipping_plane.w);
}

vec4 interpolateProjection(vec2 posInTile, vec3 spherePos, float elevation) {
    vec3 elevatedPos = spherePos * (1.0 + elevation / GLOBE_RADIUS);
    vec4 globePosition = u_projection_matrix * vec4(elevatedPos, 1.0);
    // Clip the far side of the globe through Z; the layer's depth shift keeps layer order.
    globePosition.z = globeComputeClippingZ(elevatedPos) * globePosition.w - u_projection_depth_offset;

    if (u_projection_transition > 0.999) {
        return globePosition;
    }

    vec4 flatPosition = u_projection_fallback_matrix * vec4(posInTile, elevation, 1.0);
    const float z_globeness_threshold = 0.2;
    vec4 result = globePosition;
    result.z = mix(0.0, globePosition.z, clamp((u_projection_transition - z_globeness_threshold) / (1.0 - z_globeness_threshold), 0.0, 1.0));
    result.xyw = mix(flatPosition.xyw, globePosition.xyw, u_projection_transition);
    if ((posInTile.y < GLOBE_POLE_NORTH_Y) || (posInTile.y > GLOBE_POLE_SOUTH_Y)) {
        result = globePosition;
        const float poles_hidden_anim_percentage = 0.02;
        result.z = mix(globePosition.z, 100.0, pow(max((1.0 - u_projection_transition) / poles_hidden_anim_percentage, 0.0), 8.0));
    }
    return result;
}

// Keeps the matrix Z, for geometry that needs the depth buffer.
vec4 interpolateProjectionFor3D(vec2 posInTile, vec3 spherePos, float elevation) {
    vec3 elevatedPos = spherePos * (1.0 + elevation / GLOBE_RADIUS);
    vec4 globePosition = u_projection_matrix * vec4(elevatedPos, 1.0);
    if (u_projection_transition > 0.999) {
        return globePosition;
    }
    vec4 fallbackPosition = u_projection_fallback_matrix * vec4(posInTile, elevation, 1.0);
    return mix(fallbackPosition, globePosition, u_projection_transition);
}

vec4 projectTile(vec2 pos) {
    return interpolateProjection(pos, projectToSphere(pos + u_projection_translate, vec2(0.0, 0.0)), 0.0);
}

// The variant for geometry that can carry pole vertices; rawPos is the untranslated position.
vec4 projectTile(vec2 pos, vec2 rawPos) {
    return interpolateProjection(pos, projectToSphere(pos + u_projection_translate, rawPos), 0.0);
}

vec4 projectTileWithElevation(vec2 pos, float elevation) {
    return interpolateProjection(pos, projectToSphere(pos + u_projection_translate, vec2(0.0, 0.0)), elevation);
}

vec4 projectTileFor3D(vec2 pos, float elevation) {
    return interpolateProjectionFor3D(pos, projectToSphere(pos + u_projection_translate, pos), elevation);
}

#else

vec4 projectTile(vec2 pos) {
    return u_projection_matrix * vec4(pos, 0.0, 1.0);
}

// Pole vertices only exist on the globe; put them behind the near plane so their triangles are clipped.
vec4 projectTile(vec2 pos, vec2 rawPos) {
    vec4 result = u_projection_matrix * vec4(pos, 0.0, 1.0);
    if (rawPos.y < GLOBE_POLE_NORTH_Y || rawPos.y > GLOBE_POLE_SOUTH_Y) {
        result.z = -10000000.0;
    }
    return result;
}

vec4 projectTileWithElevation(vec2 pos, float elevation) {
    return u_projection_matrix * vec4(pos, elevation, 1.0);
}

vec4 projectTileFor3D(vec2 pos, float elevation) {
    return u_projection_matrix * vec4(pos, elevation, 1.0);
}

#endif
";

/// Whether `vertex`, a vertex stage, projects: whether it names a member of
/// the projection block or a helper of [`PROJECTION_PRELUDE`].
fn projects(vertex: &HashSet<&str>) -> bool {
    // A helper is a macro its `#define` names, or a function its definition
    // names before the `(`, at the start of a line.
    let helper = |line: &'static str| match line.strip_prefix("#define ") {
        Some(define) => define.split_whitespace().next(),
        None if !line.starts_with([' ', '/', '#']) => line.split_once('(')?.0.split_whitespace().last(),
        None => None,
    };
    vertex.iter().any(|name| name.starts_with("u_projection_"))
        || PROJECTION_PRELUDE.lines().filter_map(helper).any(|name| vertex.contains(name))
}

/// The vertex and fragment shaders of `key`'s program: `#version 300 es`,
/// or `#version 330 core` on `desktop` OpenGL, the key's defines, the
/// stage's precisions, the blocks the stage reads, the prelude of the stage
/// and the program's GLSL ES of the stage with its properties written out
/// ([`glsl_es::expand`]). A fragment shader defaults to highp floats on
/// OpenGL ES if `highp_fragment`, and to mediump otherwise. Fails with
/// [`GpuError::Unsupported`] for a program without GLSL ES, and as
/// [`glsl_es::expand`] does.
///
/// WebGL2 rejects a draw whose program has a block without a buffer, so a
/// stage declares only the blocks it names a member of: the view's and the
/// program's own. A vertex stage that projects ([`projects`]) also has the
/// projection block and the helpers that read it. A fragment shader declares the blocks it shares with the
/// vertex shader first, in the vertex shader's order, which SwiftShader
/// needs (maplibre/maplibre-native#4625).
pub(crate) fn stage_sources(key: &ShaderKey, desktop: bool, highp_fragment: bool) -> Result<[String; 2], GpuError> {
    let program = key.program();
    let source =
        program.sources.glsl_es.ok_or_else(|| GpuError::Unsupported(format!("no GLSL ES for {}", program.name)))?;
    // Desktop OpenGL compiles GLSL ES 3.00 only with
    // `GL_ARB_ES3_compatibility`, and every 3.3 context the GLSL 3.30 that
    // the shaders are as well: what they use of GLSL ES is in it, and it
    // ignores their precision qualifiers.
    let version = if desktop { "#version 330 core\n" } else { "#version 300 es\n" };
    let defines: String = key.define_values().flat_map(|(name, value)| ["#define ", name, " ", value, "\n"]).collect();
    let fragment_defines = if highp_fragment { "#define HIGHP_FRAGMENT\n" } else { "" };

    let [vertex, fragment] = glsl_es::expand(program, source)?;
    let projects = projects(&identifiers(&vertex).collect());
    let projection = PROJECTION_BUFFER.block().filter(|_| projects);
    let vertex_blocks: Vec<&Block> = projection.into_iter().chain(glsl_es::stage_blocks(program, &vertex)).collect();
    let mut fragment_blocks = glsl_es::stage_blocks(program, &fragment);
    fragment_blocks
        .sort_by_key(|block| vertex_blocks.iter().position(|shared| ptr::eq(*shared, *block)).unwrap_or(usize::MAX));
    let stage = |stage_defines, precision, blocks: &[&Block], prelude, glsl| {
        [version, &defines, stage_defines, precision, &glsl_es::blocks(blocks), prelude, glsl].concat()
    };
    let vertex_prelude =
        if projects { [PRELUDE.vertex, PROJECTION_PRELUDE].concat() } else { PRELUDE.vertex.to_owned() };
    Ok([
        stage("", PRECISION.vertex, &vertex_blocks, &vertex_prelude, &vertex),
        stage(fragment_defines, PRECISION.fragment, &fragment_blocks, PRELUDE.fragment, &fragment),
    ])
}
