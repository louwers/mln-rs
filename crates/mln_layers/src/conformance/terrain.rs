//! The terrain's height rule on the GPU, against `mln_util::dem` on the CPU.

use std::cell::Cell;

use mln_gfx::color_mode::ColorMode;
use mln_gfx::gfx_types::AttributeDataType::Float4;
use mln_gfx::gpu::{
    BindGroupEntry, BindingsDesc, BufferUsage, ColorAttachment, Draw, DynamicState, GeometryDesc, Gpu, GpuError,
    LoadOp, PassDesc, PipelineDesc, Scissor, ShaderId, ShaderKey, StencilState, TargetFormats, TextureDesc,
    TextureFormat, TextureRegion, TextureUsage, VertexAttribute, VertexBufferLayout, VertexStep, read_texels,
};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::program::{Attribute, PROJECTION_BUFFER, Program, ProgramBuffer, Projection, Sources};
use mln_gfx::shaders::source::glsl::{self, Glsl};
use mln_gfx::shaders::source::glsl_es::{self, Glsl as GlslEs};
use mln_gfx::shaders::source::msl::{self, Msl};
use mln_gfx::shaders::source::wgsl::{self, Wgsl};
use mln_gfx::shaders::terrain_ubo::{GROUND_DEM, TerrainPropsUBO};
use mln_gfx::texture2d::SamplerState;
use mln_util::Color;
use mln_util::dem::{self, DemCoords, DemUnpack};
use mln_util::size::Size;
use mln_util::tileset::RasterEncoding;

use super::{EXTENT, attribute_id, buffer, buffer_id, draw, dynamic, fixed, whole};

/// Probes of the terrain's height rule, whose vertex stage reads a height
/// by the `ground_height!` snippet of its language, and whose fragment stage
/// writes its bits. No layer draws with it. Each probe is a quad at clip
/// positions whose vertices read the height at a place of a tile, and whose
/// fragments write that height's bits, least significant byte first, as the
/// texel's channels. A vertex is its clip x and y, then where it reads the
/// height in tile units (`pos`), then the DEM uv's scale and offset that
/// map the tile's uv to the DEM's (`uv * scale + offset`), then the DEM's
/// width in pixels, 0 for flat ground (`dem_coords`).
pub static TERRAIN_PROBE: Program = Program {
    name: "conformance-terrain-probe",
    buffers: &[ProgramBuffer::uniform::<TerrainPropsUBO>("terrainProps", 0)],
    attributes: &[Attribute::new("pos", Float4), Attribute::new("dem_coords", Float4)],
    instance_attributes: &[],
    textures: &[GROUND_DEM],
    properties: &[],
    projection: Projection::None,
    sources: Sources::NONE.wgsl(&WGSL).msl(&MSL).glsl_es(&GLSL_ES).glsl(&GLSL),
};

const WGSL: Wgsl = Wgsl {
    prelude: "",
    vertex: concat!(
        wgsl::ground_height!(),
        r"
struct VertexInput {
    @location(0) pos: vec4<f32>,
    @location(1) dem_coords: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) height: f32,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(in.pos.xy, 0.0, 1.0);
    out.height = ground_height(in.pos.zw, in.dem_coords, terrainProps);
    return out;
}
"
    ),
    fragment: r"
@fragment
fn main(@location(0) @interpolate(flat) height: f32) -> @location(0) vec4<f32> {
    let bits = bitcast<u32>(height);
    let bytes = vec4<u32>(bits, bits >> 8u, bits >> 16u, bits >> 24u) & vec4<u32>(255u);
    return vec4<f32>(bytes) / 255.0;
}
",
};

const MSL: Msl = Msl {
    vertex_main_function: "vertexMain",
    fragment_main_function: "fragmentMain",
    // Fast math would let the compiler fuse or reorder the f32 operations of
    // the height rule, which the CPU does one at a time, in order.
    fast_math: false,
    prelude: "",
    source: concat!(
        msl::ground_height!(),
        r"
struct VertexStage {
    float4 pos [[attribute(0)]];
    float4 dem_coords [[attribute(1)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float height [[flat]];
};

FragmentStage vertex vertexMain(thread const VertexStage vertex_in [[stage_in]]) {
    return {
        .position = float4(vertex_in.pos.xy, 0.0, 1.0),
        .height = ground_height(vertex_in.pos.zw, vertex_in.dem_coords, ground_dem, terrainProps),
    };
}

float4 fragment fragmentMain(FragmentStage in [[stage_in]]) {
    const uint bits = as_type<uint>(in.height);
    const uint4 bytes = uint4(bits, bits >> 8, bits >> 16, bits >> 24) & uint4(255);
    return float4(bytes) / 255.0;
}
"
    ),
};

const GLSL: Glsl = Glsl {
    prelude: "",
    vertex: concat!(
        glsl::ground_height!(),
        r"
layout(location = 0) in vec4 in_pos;
layout(location = 1) in vec4 in_dem_coords;

layout(location = 0) flat out float frag_height;

void main() {
    frag_height = ground_height(in_pos.zw, in_dem_coords, terrainProps);
    gl_Position = vec4(in_pos.xy, 0.0, 1.0);
}
"
    ),
    fragment: r"
layout(location = 0) flat in float frag_height;

layout(location = 0) out vec4 out_color;

void main() {
    const uint bits = floatBitsToUint(frag_height);
    const uvec4 bytes = uvec4(bits, bits >> 8u, bits >> 16u, bits >> 24u) & uvec4(255u);
    out_color = vec4(bytes) / 255.0;
}
",
};

const GLSL_ES: GlslEs = GlslEs {
    vertex: concat!(
        glsl_es::ground_height!(),
        r"layout (location = 0) in vec4 a_pos;
layout (location = 1) in vec4 a_dem_coords;

flat out highp float v_height;

void main() {
    v_height = ground_height(a_pos.zw, a_dem_coords, u_unpack, u_unpack_scale, u_exaggeration);
    gl_Position = vec4(a_pos.xy, 0.0, 1.0);
}
"
    ),
    fragment: r"flat in highp float v_height;

void main() {
    // A fragment shader's integers are mediump unless said otherwise, which
    // Mesa lowers to 16 bits.
    highp uint bits = floatBitsToUint(v_height);
    highp uvec4 bytes = uvec4(bits, bits >> 8u, bits >> 16u, bits >> 24u) & uvec4(255u);
    fragColor = vec4(bytes) / 255.0;
}
",
};

/// The most probes a frame draws, one per texel of a one-row target, which
/// every backend's textures are wide enough for.
const MAX_PROBES: usize = 1024;

/// A DEM the probes read: how its texels decode, the exaggeration, and its
/// texels behind a one-texel border.
struct Dem {
    unpack: DemUnpack,
    exaggeration: f32,
    /// Its width in pixels, 0 for flat ground.
    dim: u32,
    /// RGBA, `dim + 2` texels a side (1 for flat ground), row by row from
    /// the top.
    texels: Vec<u8>,
}

impl Dem {
    /// A DEM of `dim` pixels, whose texels `ordinary` makes from a random
    /// number, but for a few of the lowest and highest heights that
    /// `encoding` decodes to: in the corners, next to each other in the
    /// middle, and in the border.
    fn new(encoding: RasterEncoding, exaggeration: f32, dim: u32, ordinary: impl Fn(u32) -> [u8; 3]) -> Dem {
        let side = dim + 2;
        // xorshift32, so that every backend's case reads the same texels.
        let mut state = 0x2545_f491_u32 ^ dim;
        let mut texels = Vec::with_capacity((side * side * 4) as usize);
        for _ in 0..side * side {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let [r, g, b] = ordinary(state);
            texels.extend([r, g, b, 255]);
        }
        let (low, high) = ([0, 0, 0], [255, 255, 255]);
        let half = dim / 2;
        for (x, y, rgb) in [
            (1, 1, high),
            (dim, dim, low),
            (half, half, low),
            (half + 1, half, high),
            (0, half, high),
            (dim + 1, 0, low),
        ] {
            let i = ((y * side + x) * 4) as usize;
            texels[i..i + 3].copy_from_slice(&rgb);
        }
        Dem { unpack: DemUnpack::of(encoding), exaggeration, dim, texels }
    }

    /// Flat ground, which a terrain tile without a DEM reads: one texel,
    /// which no height reads.
    fn flat() -> Dem {
        let unpack = DemUnpack::of(RasterEncoding::Mapbox);
        Dem { unpack, exaggeration: 1.0, dim: 0, texels: vec![255; 4] }
    }

    fn side(&self) -> u32 {
        if self.dim == 0 { 1 } else { self.dim + 2 }
    }

    /// The height in meters of the bordered texel at `x`, `y`.
    fn height(&self, x: u32, y: u32) -> f32 {
        let i = ((y * self.side() + x) * 4) as usize;
        self.unpack.decode([self.texels[i], self.texels[i + 1], self.texels[i + 2], self.texels[i + 3]])
    }

    /// The height at `pos`, in tile units, of a tile that reads this DEM
    /// through `coords`, by the CPU's rule, and the largest of the heights
    /// it interpolates, both exaggerated.
    fn sample(&self, coords: DemCoords, pos: [f32; 2]) -> (f32, f32) {
        let largest = Cell::new(0.0_f32);
        let height = dem::sample(self.dim, coords, pos.map(|p| p / EXTENT), |x, y| {
            let height = self.height(x, y);
            largest.set(largest.get().max(height.abs()));
            height
        });
        (height * self.exaggeration, largest.get() * self.exaggeration)
    }
}

/// Where along one axis of a tile the probes read, in tile units, for a
/// tile that reads a DEM of `dim` pixels at `scale` and `offset` on that
/// axis: the tile's edges and the texel next to them, mesh vertices, and
/// the centres of the DEM pixels at the tile's ends and middle and the
/// edges between them.
fn axis(dim: u32, scale: f32, offset: f32) -> Vec<f32> {
    let mut axis = vec![0.0, 1.0, 64.0, 4096.0, 4160.0, 8128.0, 8191.0, 8192.0];
    if dim > 0 {
        let dim = f64::from(dim);
        let (scale, offset) = (f64::from(scale), f64::from(offset));
        // The tile at DEM uv `u`.
        let at = |u: f64| ((u - offset) / scale * f64::from(EXTENT)) as f32;
        let first = (offset * dim).floor();
        let last = ((offset + scale) * dim).ceil() - 1.0;
        let middle = f64::midpoint(first, last).floor();
        for pixel in [first, middle, last] {
            axis.push(at((pixel + 0.5) / dim));
        }
        for edge in [first + 1.0, middle + 1.0, last] {
            axis.push(at(edge / dim));
        }
    }
    axis.retain(|pos| (0.0..=EXTENT).contains(pos));
    axis.sort_by(f32::total_cmp);
    axis.dedup();
    axis
}

/// Checks that the terrain's height rule on `gpu`, the `ground_height!`
/// snippet of its shading language that [`TERRAIN_PROBE`] reads with, reads
/// the heights that `mln_util::dem` reads on the CPU, to within 1 mm and 4
/// ulp of the largest height interpolated.
///
/// The probes read Mapbox, Terrarium and custom DEMs, each with its lowest
/// and highest heights next to ordinary ones and border texels unlike the
/// pixels at their edge, and flat ground. Each DEM is read through its own
/// tile, a quadrant of it, and a sub-rect of an ancestor, at the places of
/// [`axis`]: the tile's edges (where the height reads the border), mesh
/// vertices, DEM pixel centres and the edges between pixels. Returns the
/// error of a call that should succeed.
///
/// # Panics
///
/// Where a height doesn't agree.
pub fn terrain_heights(gpu: &mut impl Gpu) -> Result<(), GpuError> {
    let mapbox = |random: u32| [1, random as u8, (random >> 8) as u8];
    let terrarium = |random: u32| [120 + (random % 16) as u8, (random >> 8) as u8, (random >> 16) as u8];
    let custom =
        RasterEncoding::Custom { red_factor: 256.0, green_factor: 1.0, blue_factor: 1.0 / 256.0, base_shift: 32768.0 };
    let dems = [
        Dem::new(RasterEncoding::Mapbox, 1.0, 256, mapbox),
        Dem::new(RasterEncoding::Terrarium, 2.0, 16, terrarium),
        Dem::new(custom, 1.5, 16, terrarium),
        Dem::flat(),
    ];
    let shader = gpu.create_shader(&ShaderKey::new(&TERRAIN_PROBE, []))?;
    // A tile's own DEM, a quadrant of it, and a sub-rect of an ancestor's.
    let coords =
        [DemCoords::OWN, DemCoords { scale: 0.5, offset: [0.5, 0.0] }, DemCoords { scale: 0.25, offset: [0.25, 0.75] }];
    let mut failures = Vec::new();
    let mut probed = 0;
    for dem in &dems {
        let probes: Vec<(DemCoords, [f32; 2])> = coords
            .into_iter()
            .take(if dem.dim == 0 { 1 } else { coords.len() })
            .flat_map(|coords| {
                let ys = axis(dem.dim, coords.scale, coords.offset[1]);
                axis(dem.dim, coords.scale, coords.offset[0])
                    .into_iter()
                    .flat_map(move |x| ys.clone().into_iter().map(move |y| (coords, [x, y])))
            })
            .collect();
        for probes in probes.chunks(MAX_PROBES) {
            let heights = probe(gpu, shader, dem, probes)?;
            for (&(coords, pos), height) in probes.iter().zip(heights) {
                let (expected, largest) = dem.sample(coords, pos);
                let ulp = f32::from_bits(largest.to_bits() + 1) - largest;
                let tolerance = 1e-3 + 4.0 * f64::from(ulp);
                let off = (f64::from(height) - f64::from(expected)).abs();
                if off.is_nan() || off > tolerance {
                    failures.push(format!(
                        "dim {} at {pos:?} through {coords:?}: {height} on the GPU, {expected} on the CPU",
                        dem.dim
                    ));
                }
            }
            probed += probes.len();
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {probed} heights disagree, such as:\n{}",
        failures.len(),
        failures[..failures.len().min(10)].join("\n")
    );
    Ok(())
}

/// The heights that [`TERRAIN_PROBE`] reads on `gpu` from `dem` at each of
/// `probes`, each a tile's DEM coords and a place in tile units: a quad
/// over one texel each of a one-row target, whose texel's channels are the
/// height's bits, least significant byte first.
fn probe(
    gpu: &mut impl Gpu,
    shader: ShaderId,
    dem: &Dem,
    probes: &[(DemCoords, [f32; 2])],
) -> Result<Vec<f32>, GpuError> {
    let size = Size::new(probes.len() as u32, 1);
    let target = gpu.create_texture(&TextureDesc {
        label: "terrain probes",
        size,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsage { render_target: true, copy_src: true, ..TextureUsage::default() },
        mipmapped: false,
    })?;
    let side = dem.side();
    let texture = gpu.create_texture(&TextureDesc {
        label: "dem",
        size: Size::new(side, side),
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsage { sampled: true, ..TextureUsage::default() },
        mipmapped: false,
    })?;
    gpu.write_texture(texture, TextureRegion::whole(Size::new(side, side)), &dem.texels)?;
    let sampler = gpu.create_sampler(&SamplerState::default())?;
    let props = TerrainPropsUBO {
        unpack: dem.unpack.weights_and_base(),
        unpack_scale: dem.unpack.scale,
        exaggeration: dem.exaggeration,
        ..TerrainPropsUBO::default()
    };
    let props = bytemuck::bytes_of(&props);
    let props_buffer = buffer(gpu, BufferUsage::Uniform, props)?;
    // A draw binds a projection block though the program reads none.
    let projection = [ProjectionUBO::default()];
    let projection = bytemuck::cast_slice(&projection);
    let projection_buffer = buffer(gpu, BufferUsage::Storage, projection)?;
    let bindings = gpu.create_bindings(&BindingsDesc {
        label: "terrain probes",
        shader,
        entries: &[
            // The DEM, at the slot of its index among the program's textures.
            BindGroupEntry::Texture { id: 0, texture, sampler },
            whole(buffer_id(&TERRAIN_PROBE, "terrainProps")?, props_buffer, props),
            whole(PROJECTION_BUFFER.id, projection_buffer, projection),
        ],
    })?;

    // Each vertex is its clip position and the place it reads, then the
    // DEM coords.
    let width = probes.len() as f32;
    let mut vertices: Vec<[f32; 8]> = Vec::with_capacity(probes.len() * 4);
    let mut indices: Vec<u16> = Vec::with_capacity(probes.len() * 6);
    for (column, &(coords, [x, y])) in probes.iter().enumerate() {
        let first = vertices.len() as u16;
        indices.extend([0, 1, 2, 1, 2, 3].map(|i| first + i));
        let (left, right) = (2.0 * column as f32 / width - 1.0, 2.0 * (column + 1) as f32 / width - 1.0);
        for [clip_x, clip_y] in [[left, -1.0], [right, -1.0], [left, 1.0], [right, 1.0]] {
            let [dx, dy] = coords.offset;
            vertices.push([clip_x, clip_y, x, y, coords.scale, dx, dy, dem.dim as f32]);
        }
    }
    let vertices = buffer(gpu, BufferUsage::Vertex, bytemuck::cast_slice(&vertices))?;
    let index_count = indices.len() as u32;
    let indices = buffer(gpu, BufferUsage::Index, bytemuck::cast_slice(&indices))?;
    let attribute = |id: usize, offset| VertexAttribute { id: id as u32, format: Float4, offset };
    let layouts = [VertexBufferLayout {
        stride: 32,
        step: VertexStep::Vertex,
        attributes: vec![
            attribute(attribute_id(&TERRAIN_PROBE, "pos")?, 0),
            attribute(attribute_id(&TERRAIN_PROBE, "dem_coords")?, 16),
        ],
    }];
    let geometry = gpu.create_geometry(&GeometryDesc {
        shader,
        layouts: &layouts,
        vertex_buffers: &[(vertices, 0)],
        index_buffer: indices,
    })?;
    let pipeline = gpu.create_pipeline(&PipelineDesc {
        shader,
        vertex_buffers: &layouts,
        state: fixed(ColorMode::unblended(), StencilState::DISABLED),
        targets: TargetFormats { color: Some(TextureFormat::Rgba8Unorm), depth_stencil: None },
    })?;
    gpu.begin_pass(&PassDesc {
        label: "terrain probes",
        color: Some(ColorAttachment { texture: target, load: LoadOp::Clear(Color::black()) }),
        depth_stencil: None,
    })?;
    let scissor = Scissor { x: 0, y: 0, width: size.width, height: size.height };
    let dynamic = DynamicState { scissor, ..dynamic() };
    gpu.draw(&Draw { dynamic, ..draw(pipeline, bindings, geometry, index_count) })?;
    gpu.end_pass()?;
    let texels = read_texels(gpu, target, TextureRegion::whole(size))?;
    Ok(texels
        .as_chunks::<4>()
        .0
        .iter()
        .map(|texel| f32::from_le_bytes([texel[0], texel[1], texel[2], texel[3]]))
        .collect())
}
