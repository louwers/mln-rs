//! The MSL prelude every shader compiles after, and how a program's library
//! composes it with the blocks the program reads and its MSL.

use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::gpu::GpuError;
use mln_gfx::shaders::program::{
    Buffer, BufferKind, DRAW_PROJECTION_BUFFER, ENGINE_BUFFERS, PROJECTION_BUFFER, Program, VIEW_BUFFER,
};
use mln_gfx::shaders::source::identifiers;
use mln_gfx::shaders::source::msl::{self, Msl};
use mln_util::hash::HashSet;

/// The start of every library.
const HEADER: &str = "
#include <metal_stdlib>
using namespace metal;
";

/// The helpers every program compiles after.
const PRELUDE: &str = r"

// OpenGL `mod` is `x-y*floor(x/y)` where `floor` rounds down.
// Metal `fmod` is `x-y*trunc(x/y)` where `trunc` rounds toward zero.
// glMod is GLSL's `mod`, for the shaders that rely on it.
template <typename T1, typename T2>
inline auto glMod(T1 x, T2 y) { return x - y * metal::floor(x/y); }

// Unpack a pair of values that have been packed into a single float.
// The packed values are assumed to be 8-bit unsigned integers, and are
// packed like so: packedValue = floor(input[0]) * 256 + input[1],
float2 unpack_float(const float packedValue) {
    const int packedIntValue = int(packedValue);
    const int v0 = packedIntValue / 256;
    return float2(v0, packedIntValue - v0 * 256);
}
// To minimize the number of attributes needed, we encode a 4-component
// color into a pair of floats (i.e. a vec2) as follows:
// [ floor(color.r * 255) * 256 + color.g * 255, floor(color.b * 255) * 256 + color.a * 255 ]
float4 decode_color(const float2 encoded) {
    return float4(unpack_float(encoded[0]) / 255, unpack_float(encoded[1]) / 255);
}
// Unpack a pair of paint values and interpolate between them.
float unpack_mix_float(device const float packedValue[2], const float t) {
    return mix(packedValue[0], packedValue[1], t);
}
float unpack_mix_float(const float2 packedValue, const float t) {
    return mix(packedValue[0], packedValue[1], t);
}
// Unpack a pair of paint values and interpolate between them.
float4 unpack_mix_color(device const float packedColors[4], const float t) {
    return mix(decode_color(float2(packedColors[0], packedColors[1])),
               decode_color(float2(packedColors[2], packedColors[3])), t);
}
float4 unpack_mix_color(const float4 packedColors, const float t) {
    return mix(decode_color(float2(packedColors[0], packedColors[1])),
               decode_color(float2(packedColors[2], packedColors[3])), t);
}

// Pole vertices carry these sentinel Y values in their raw position.
#define GLOBE_POLE_NORTH_Y -32767.5
#define GLOBE_POLE_SOUTH_Y 32766.5

#if defined(PROJECTION_GLOBE)

#define GLOBE_RADIUS 6371008.8

// Tile position (0..EXTENT) to a point on the unit sphere; the pole sentinels in rawPos map to the poles.
inline float3 projectToSphere(float2 translatedPos, float2 rawPos, device const ProjectionUBO& projection) {
    const float2 mercator_pos = projection.tile_mercator_coords.xy + projection.tile_mercator_coords.zw * translatedPos;
    const float spherical_x = mercator_pos.x * M_PI_F * 2.0 + M_PI_F;
    // sin/cos of the latitude from the Mercator Y via the tangent half-angle identities: no atan, and float32 precision survives near the equator.
    const float t = exp(M_PI_F - (mercator_pos.y * M_PI_F * 2.0));
    const float t2 = t * t;
    const float denom = t2 + 1.0;
    const float sin_sy = (t2 - 1.0) / denom;
    const float cos_sy = (2.0 * t) / denom;
    float3 pos = float3(sin(spherical_x) * cos_sy, sin_sy, cos(spherical_x) * cos_sy);
    if (rawPos.y < GLOBE_POLE_NORTH_Y) {
        pos = float3(0.0, 1.0, 0.0);
    }
    if (rawPos.y > GLOBE_POLE_SOUTH_Y) {
        pos = float3(0.0, -1.0, 0.0);
    }
    return pos;
}

inline float3 globeRotateVector(float3 vec, float2 angles) {
    float3 axisRight = float3(vec.z, 0.0, -vec.x);
    float3 axisUp = cross(axisRight, vec);
    axisRight = normalize(axisRight);
    axisUp = normalize(axisUp);
    const float2 t = tan(angles);
    return normalize(vec + axisRight * t.x + axisUp * t.y);
}

// cos(latitude) at a tile Y, from the same exp() form as projectToSphere.
inline float circumferenceRatioAtTileY(float tileY, device const ProjectionUBO& projection) {
    const float mercator_pos_y = projection.tile_mercator_coords.y + projection.tile_mercator_coords.w * tileY;
    const float t = exp(M_PI_F - (mercator_pos_y * M_PI_F * 2.0));
    return (2.0 * t) / (t * t + 1.0);
}

inline float globeComputeClippingZ(float3 spherePos, device const ProjectionUBO& projection) {
    return 1.0 - (dot(spherePos, projection.clipping_plane.xyz) + projection.clipping_plane.w);
}

inline float4 interpolateProjection(float2 posInTile, float3 spherePos, float elevation, device const ProjectionUBO& projection) {
    const float3 elevatedPos = spherePos * (1.0 + elevation / GLOBE_RADIUS);
    float4 globePosition = projection.matrix * float4(elevatedPos, 1.0);
    // Clip the far side of the globe through Z; the layer's depth shift keeps layer order.
    // The layer offset is subtracted in clip space on purpose: the separation it buys grows as the camera nears the
    // sphere, where the hemisphere's Z gradient is steepest; a constant NDC offset z-fights fill against outline.
    globePosition.z = globeComputeClippingZ(elevatedPos, projection) * globePosition.w - projection.depth_offset;

    if (projection.transition > 0.999) {
        return globePosition;
    }

    const float4 flatPosition = projection.fallback_matrix * float4(posInTile, elevation, 1.0);
    const float z_globeness_threshold = 0.2;
    float4 result = globePosition;
    result.z = mix(0.0, globePosition.z, clamp((projection.transition - z_globeness_threshold) / (1.0 - z_globeness_threshold), 0.0, 1.0));
    result.xyw = mix(flatPosition.xyw, globePosition.xyw, projection.transition);
    if ((posInTile.y < GLOBE_POLE_NORTH_Y) || (posInTile.y > GLOBE_POLE_SOUTH_Y)) {
        result = globePosition;
        const float poles_hidden_anim_percentage = 0.02;
        result.z = mix(globePosition.z, 100.0, pow(max((1.0 - projection.transition) / poles_hidden_anim_percentage, 0.0), 8.0));
    }
    return result;
}

// Keeps the matrix Z, for geometry that needs the depth buffer.
inline float4 interpolateProjectionFor3D(float2 posInTile, float3 spherePos, float elevation, device const ProjectionUBO& projection) {
    const float3 elevatedPos = spherePos * (1.0 + elevation / GLOBE_RADIUS);
    const float4 globePosition = projection.matrix * float4(elevatedPos, 1.0);
    if (projection.transition > 0.999) {
        return globePosition;
    }
    const float4 fallbackPosition = projection.fallback_matrix * float4(posInTile, elevation, 1.0);
    return mix(fallbackPosition, globePosition, projection.transition);
}

inline float4 projectTile(float2 pos, device const ProjectionUBO& projection) {
    return interpolateProjection(pos, projectToSphere(pos + projection.translate, float2(0.0, 0.0), projection), 0.0, projection);
}

// The variant for geometry that can carry pole vertices; rawPos is the untranslated position.
inline float4 projectTile(float2 pos, float2 rawPos, device const ProjectionUBO& projection) {
    return interpolateProjection(pos, projectToSphere(pos + projection.translate, rawPos, projection), 0.0, projection);
}

inline float4 projectTileWithElevation(float2 pos, float elevation, device const ProjectionUBO& projection) {
    return interpolateProjection(pos, projectToSphere(pos + projection.translate, float2(0.0, 0.0), projection), elevation, projection);
}

inline float4 projectTileFor3D(float2 pos, float elevation, device const ProjectionUBO& projection) {
    return interpolateProjectionFor3D(pos, projectToSphere(pos + projection.translate, pos, projection), elevation, projection);
}

#else

inline float4 projectTile(float2 pos, device const ProjectionUBO& projection) {
    return projection.matrix * float4(pos, 0.0, 1.0);
}

// Pole vertices only exist on the globe; put them behind the near plane so their triangles are clipped.
inline float4 projectTile(float2 pos, float2 rawPos, device const ProjectionUBO& projection) {
    float4 result = projection.matrix * float4(pos, 0.0, 1.0);
    if (rawPos.y < GLOBE_POLE_NORTH_Y || rawPos.y > GLOBE_POLE_SOUTH_Y) {
        result.z = -10000000.0;
    }
    return result;
}

inline float4 projectTileWithElevation(float2 pos, float elevation, device const ProjectionUBO& projection) {
    return projection.matrix * float4(pos, elevation, 1.0);
}

inline float4 projectTileFor3D(float2 pos, float elevation, device const ProjectionUBO& projection) {
    return projection.matrix * float4(pos, elevation, 1.0);
}

#endif

";

/// The parameter of the draw's index into its layer's storage buffers,
/// which a main function reads as `uboIndex`.
const UBO_INDEX_PARAMETER: &str = "device const uint32_t& uboIndex [[buffer(1)]]";

/// The engine's buffers that a main function may read without declaring
/// them, besides `uboIndex`: the view's block and the projection blocks.
const ENGINE_PARAMETERS: [Buffer; 3] = [VIEW_BUFFER, PROJECTION_BUFFER, DRAW_PROJECTION_BUFFER];

/// The parameter of `buffer` at the buffer index of its id, by its name: a
/// reference to a block, and a pointer to the elements or the vertices of a
/// storage buffer.
fn parameter(buffer: &Buffer) -> String {
    let (ty, by) = match buffer.kind {
        BufferKind::Uniform(block) | BufferKind::DrawUniform(block) => (block.name, '&'),
        BufferKind::Storage { block, .. } => (block.name, '*'),
        BufferKind::Vertices { format } => (vertex_type(format), '*'),
    };
    format!("device const {ty}{by} {} [[buffer({})]]", buffer.name, buffer.id)
}

/// The type of a vertex of `format`.
fn vertex_type(format: AttributeDataType) -> &'static str {
    use AttributeDataType::{
        Byte, Byte2, Byte3, Byte4, Float, Float2, Float3, Float4, Int, Int2, Int3, Int4, Invalid, Short, Short2,
        Short3, Short4, UByte, UByte2, UByte3, UByte4, UByte4Normalized, UInt, UInt2, UInt3, UInt4, UShort, UShort2,
        UShort3, UShort4, UShort8,
    };
    match format {
        Byte => "char",
        Byte2 => "char2",
        Byte3 => "char3",
        Byte4 => "char4",
        UByte => "uchar",
        UByte2 => "uchar2",
        UByte3 => "uchar3",
        UByte4 | UByte4Normalized => "uchar4",
        Short => "short",
        Short2 => "short2",
        Short3 => "short3",
        Short4 => "short4",
        UShort => "ushort",
        UShort2 => "ushort2",
        UShort3 => "ushort3",
        UShort4 => "ushort4",
        Int => "int",
        Int2 => "int2",
        Int3 => "int3",
        Int4 => "int4",
        UInt => "uint",
        UInt2 => "uint2",
        UInt3 => "uint3",
        UInt4 => "uint4",
        Float => "float",
        Float2 => "float2",
        Float3 => "float3",
        Float4 => "float4",
        // `Program::check` rejects a vertex buffer of these.
        UShort8 | Invalid => "void",
    }
}

/// The source of `program`'s library, of its MSL `source`: the header, the
/// structs of the engine's blocks, the prelude, the structs of the
/// program's blocks, each a storage buffer's element padded to its stride,
/// and the program's own prelude and source with its properties written
/// out ([`msl::expand`]), and each main function with the parameters of the
/// buffers that it names, the engine's and the program's. Fails as
/// [`msl::expand`] does.
pub(crate) fn library_source(program: &Program, source: &Msl) -> Result<String, GpuError> {
    let (engine, own) = (msl::blocks(&ENGINE_BUFFERS), msl::blocks(program.buffers));
    let mut functions = msl::expand(program, source)?;
    for main in [source.vertex_main_function, source.fragment_main_function] {
        functions = with_parameters(&functions, main, program.buffers);
    }
    Ok([HEADER, &engine, PRELUDE, &own, source.prelude, &functions].concat())
}

/// `functions` with the parameters of the buffers that the main function
/// `main` names in its body added to its parameters: `uboIndex`, the
/// engine's buffers and `buffers`.
fn with_parameters(functions: &str, main: &str, buffers: &[Buffer]) -> String {
    let Some(name_at) = functions.find(&format!(" {main}(")) else { return functions.to_owned() };
    let Some(close) = matching(functions, name_at, '(', ')') else { return functions.to_owned() };
    let Some(end) = matching(functions, close, '{', '}') else { return functions.to_owned() };
    let (parameters, body) = (&functions[name_at..close], &functions[close..end]);
    let declared: HashSet<&str> = identifiers(parameters).collect();
    let named: HashSet<&str> = identifiers(body).filter(|name| !declared.contains(name)).collect();
    let mut added = String::new();
    if named.contains("uboIndex") {
        added.push_str(",\n    ");
        added.push_str(UBO_INDEX_PARAMETER);
    }
    for buffer in ENGINE_PARAMETERS.iter().chain(buffers).filter(|buffer| named.contains(buffer.name)) {
        added.push_str(",\n    ");
        added.push_str(&parameter(buffer));
    }
    [&functions[..close], &added, &functions[close..]].concat()
}

/// Where in `text` the `close` is that closes the first `open` from `from`.
fn matching(text: &str, from: usize, open: char, close: char) -> Option<usize> {
    let start = from + text[from..].find(open)?;
    let mut depth = 0;
    text[start..].char_indices().find_map(|(at, c)| {
        depth += i32::from(c == open) - i32::from(c == close);
        (depth == 0).then_some(start + at)
    })
}

#[cfg(test)]
mod tests {
    use mln_gfx::gfx_types::AttributeDataType::Short4;
    use mln_gfx::shaders::bindings::DRAW_INDEX_ID;

    use super::*;

    #[test]
    fn the_engines_buffers_bind_at_their_ids() {
        assert!(UBO_INDEX_PARAMETER.ends_with(&format!("[[buffer({DRAW_INDEX_ID})]]")));
        assert_eq!(parameter(&VIEW_BUFFER), "device const ViewUBO& view [[buffer(0)]]");
        assert_eq!(parameter(&PROJECTION_BUFFER), "device const ProjectionUBO* projectionVector [[buffer(4)]]");
        assert_eq!(parameter(&DRAW_PROJECTION_BUFFER), "device const ProjectionUBO& drawProjection [[buffer(4)]]");
        assert_eq!(parameter(&Buffer::vertices("walls", 0, Short4)), "device const short4* walls [[buffer(7)]]");
    }

    #[test]
    fn a_main_function_has_the_parameters_of_the_buffers_its_body_names() {
        let walls = Buffer::vertices("walls", 0, Short4);
        let functions = "void helper(int walls) {}\n\
                         float4 vertex main(uint id [[vertex_id]]) {\n    return walls[uboIndex] + view.x;\n}\n\
                         half4 fragment other(int view) { return walls; }";
        let with = with_parameters(functions, "main", &[walls]);
        assert_eq!(
            with,
            format!(
                "void helper(int walls) {{}}\nfloat4 vertex main(uint id [[vertex_id]],\n    {UBO_INDEX_PARAMETER},\n    {},\n    {}) \
                 {{\n    return walls[uboIndex] + view.x;\n}}\nhalf4 fragment other(int view) {{ return walls; }}",
                parameter(&VIEW_BUFFER),
                parameter(&walls)
            )
        );
    }
}
