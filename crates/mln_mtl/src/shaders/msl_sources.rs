//! The MSL prelude every shader compiles after, and how a program's library
//! composes it with the blocks the program reads and its MSL.

use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::gpu::GpuError;
use mln_gfx::shaders::bindings::DRAW_INDEX_ID;
use mln_gfx::shaders::program::{Binding, BufferKind, ENGINE_BUFFERS, Program};
use mln_gfx::shaders::source::msl::{self, Msl};
use mln_gfx::shaders::source::{Stage, identifiers};
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

/// The parameter of `binding`, by its name: the draw's index at
/// [`DRAW_INDEX_ID`], a buffer at the buffer index of its id, a reference to
/// a block and a pointer to the elements or the vertices of a storage
/// buffer, and a texture at slot `slot` and its sampler at texture and
/// sampler index `slot`.
fn parameter(binding: &Binding<'_>) -> String {
    let name = binding.name();
    match *binding {
        Binding::DrawIndex => format!("device const uint32_t& {name} [[buffer({DRAW_INDEX_ID})]]"),
        Binding::Buffer(buffer) => {
            let (ty, by) = match buffer.kind {
                BufferKind::Uniform(block) | BufferKind::DrawUniform(block) => (block.name, '&'),
                BufferKind::Storage { block, .. } => (block.name, '*'),
                BufferKind::Vertices { format } => (vertex_type(format), '*'),
            };
            format!("device const {ty}{by} {name} [[buffer({})]]", buffer.id)
        }
        Binding::Texture(slot, _) => format!("texture2d<float, access::sample> {name} [[texture({slot})]]"),
        Binding::Sampler(slot, _) => format!("sampler {name} [[sampler({slot})]]"),
    }
}

/// The type of a vertex of `format`.
fn vertex_type(format: AttributeDataType) -> &'static str {
    use AttributeDataType::{
        Byte4, Float, Float2, Float4, Int, Int2, Int4, Short2, Short4, UByte4, UInt, UInt2, UInt4, UShort2, UShort4,
    };
    match format {
        Byte4 => "char4",
        UByte4 => "uchar4",
        Short2 => "short2",
        Short4 => "short4",
        UShort2 => "ushort2",
        UShort4 => "ushort4",
        Int => "int",
        Int2 => "int2",
        Int4 => "int4",
        UInt => "uint",
        UInt2 => "uint2",
        UInt4 => "uint4",
        Float => "float",
        Float2 => "float2",
        Float4 => "float4",
        // `Program::check` rejects a vertex buffer of the others.
        _ => "void",
    }
}

/// The source of `program`'s library, of its MSL `source`: the header, the
/// structs of the engine's blocks, the prelude, the structs of the
/// program's blocks, each a storage buffer's element padded to its stride,
/// and the program's own prelude and source with its properties written
/// out ([`msl::expand`]), and each main function with the parameters of
/// what it names of what its stage may read ([`with_parameters`]). Fails
/// as [`msl::expand`] does.
pub(crate) fn library_source(program: &Program, source: &Msl) -> Result<String, GpuError> {
    let (engine, own) = (msl::blocks(&ENGINE_BUFFERS), msl::blocks(program.buffers));
    let mut functions = msl::expand(program, source)?;
    for (main, stage) in
        [(source.vertex_main_function, Stage::Vertex), (source.fragment_main_function, Stage::Fragment)]
    {
        functions = with_parameters(&functions, main, program, stage);
    }
    Ok([HEADER, &engine, PRELUDE, &own, source.prelude, &functions].concat())
}

/// `functions` with the parameters of what the main function `main` of
/// `stage` names in its body and may read ([`Program::bindings`]) added to
/// its parameters.
fn with_parameters(functions: &str, main: &str, program: &Program, stage: Stage) -> String {
    let Some(name_at) = functions.find(&format!(" {main}(")) else { return functions.to_owned() };
    let Some(close) = matching(functions, name_at, '(', ')') else { return functions.to_owned() };
    let Some(end) = matching(functions, close, '{', '}') else { return functions.to_owned() };
    let (parameters, body) = (&functions[name_at..close], &functions[close..end]);
    let declared: HashSet<&str> = identifiers(parameters).collect();
    let named: HashSet<&str> = identifiers(body).filter(|name| !declared.contains(name)).collect();
    // The function's own parameters, after ` main(`.
    let own = &parameters[main.len() + 2..];
    let mut added = String::new();
    for binding in program.bindings(stage).filter(|binding| named.contains(&*binding.name())) {
        if !added.is_empty() || !own.trim().is_empty() {
            added.push(',');
        }
        added.push_str("\n    ");
        added.push_str(&parameter(&binding));
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
    use mln_gfx::shaders::program::{Buffer, DRAW_PROJECTION_BUFFER, PROJECTION_BUFFER, Sources, Texture, VIEW_BUFFER};

    use super::*;

    #[test]
    fn the_engines_bindings_are_at_their_ids() {
        let parameter = |binding| parameter(&binding);
        assert_eq!(parameter(Binding::DrawIndex), "device const uint32_t& drawIndex [[buffer(1)]]");
        assert_eq!(parameter(Binding::Buffer(&VIEW_BUFFER)), "device const ViewUBO& view [[buffer(0)]]");
        assert_eq!(
            parameter(Binding::Buffer(&PROJECTION_BUFFER)),
            "device const ProjectionUBO* projectionVector [[buffer(4)]]"
        );
        assert_eq!(
            parameter(Binding::Buffer(&DRAW_PROJECTION_BUFFER)),
            "device const ProjectionUBO& drawProjection [[buffer(4)]]"
        );
        assert_eq!(
            parameter(Binding::Buffer(&Buffer::vertices("walls", 0, Short4))),
            "device const short4* walls [[buffer(7)]]"
        );
    }

    #[test]
    fn a_main_function_has_the_parameters_of_what_its_body_names_and_its_stage_may_read() {
        static PROGRAM: Program = Program {
            name: "walls",
            buffers: &[Buffer::vertices("walls", 0, Short4)],
            attributes: &[],
            instance_attributes: &[],
            textures: &[Texture::new("first"), Texture::new("second")],
            properties: &[],
            sources: Sources::NONE,
        };
        let functions = "void helper(int walls) {}\n\
                         float4 vertex main(uint id [[vertex_id]]) {\n    return walls[drawIndex] + view.x;\n}\n\
                         half4 fragment other(int view) {\n    return walls + second.sample(first_sampler, 0);\n}";
        let with = with_parameters(functions, "main", &PROGRAM, Stage::Vertex);
        let with = with_parameters(&with, "other", &PROGRAM, Stage::Fragment);
        assert_eq!(
            with,
            "void helper(int walls) {}\nfloat4 vertex main(uint id [[vertex_id]],\n    \
             device const uint32_t& drawIndex [[buffer(1)]],\n    device const ViewUBO& view [[buffer(0)]],\n    \
             device const short4* walls [[buffer(7)]]) {\n    return walls[drawIndex] + view.x;\n}\n\
             half4 fragment other(int view,\n    sampler first_sampler [[sampler(0)]],\n    \
             texture2d<float, access::sample> second [[texture(1)]]) {\n    return walls + second.sample(first_sampler, 0);\n}"
        );
    }

    #[test]
    fn a_main_function_without_parameters_of_its_own_has_no_leading_comma() {
        static PROGRAM: Program = Program {
            name: "plain",
            buffers: &[],
            attributes: &[],
            instance_attributes: &[],
            textures: &[],
            properties: &[],
            sources: Sources::NONE,
        };
        let functions = "half4 fragment fragmentMain() {\n    return half4(view.x);\n}";
        assert_eq!(
            with_parameters(functions, "fragmentMain", &PROGRAM, Stage::Fragment),
            "half4 fragment fragmentMain(\n    device const ViewUBO& view [[buffer(0)]]) {\n    return half4(view.x);\n}"
        );
    }
}
