//! The terrain's height rule on the GPU, against `mln_engine::tile::dem` on the CPU.

use std::cell::Cell;

use mln_engine::shaders::terrain::ubo::TerrainSurfaceUBO;
use mln_engine::tile::dem::{self, DemCoords, DemUnpack};
use mln_gfx::color_mode::ColorMode;
use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::gpu::{
    BindGroupEntry, BindingsDesc, BufferUsage, ColorAttachment, Draw, DynamicState, GeometryDesc, Gpu, GpuError,
    LoadOp, PassDesc, PipelineDesc, Scissor, ShaderId, ShaderKey, StencilState, TargetFormats, TextureDesc,
    TextureFormat, TextureRegion, TextureUsage, VertexAttribute, VertexBufferLayout, VertexStep, read_texels,
};
use mln_gfx::shaders::bindings::PROJECTION_ID;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::stencil_mode::StencilMode;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::{Rgba, Size};
use mln_style::tileset::RasterEncoding;

use super::{EXTENT, buffer, draw, dynamic, fixed, location, slot, whole};
use crate::conformance::programs;

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

/// Checks that the terrain's height rule on `gpu` reads the heights that
/// `mln_engine::tile::dem` reads on the CPU, to within 1 mm and 4 ulp of the
/// largest height interpolated.
///
/// The probes read Mapbox, Terrarium and custom DEMs, each with its lowest
/// and highest heights next to ordinary ones and border texels unlike the
/// pixels at their edge, and flat ground. Each DEM is read through its own
/// tile, a quadrant of it, and a sub-rect of an ancestor, at the places of
/// `axis`: the tile's edges (where the height reads the border), mesh
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
    let shader = gpu.create_shader(&ShaderKey::new(&programs::TERRAIN_PROBE, []))?;
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

/// The heights that the probe shader reads on `gpu` from `dem` at each of
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
    let props = TerrainSurfaceUBO {
        unpack: dem.unpack.weights_and_base(),
        unpack_scale: dem.unpack.scale,
        exaggeration: dem.exaggeration,
        ..TerrainSurfaceUBO::default()
    };
    let props = bytemuck::bytes_of(&props);
    let props_buffer = buffer(gpu, BufferUsage::Uniform, props)?;
    // GL's shaders declare the projection block, which must be bound though
    // the probe doesn't read it.
    let projection = [ProjectionUBO::default()];
    let projection = bytemuck::cast_slice(&projection);
    let projection_buffer = buffer(gpu, BufferUsage::Storage, projection)?;
    let bindings = gpu.create_bindings(&BindingsDesc {
        label: "terrain probes",
        shader,
        entries: &[
            BindGroupEntry::Texture { slot: slot(&programs::TERRAIN_PROBE, "ground_dem"), texture, sampler },
            whole(const { programs::TERRAIN_PROBE.buffer_id("terrain") }, props_buffer, props),
            whole(PROJECTION_ID, projection_buffer, projection),
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
    let attribute = |name, offset| VertexAttribute {
        location: location(&programs::TERRAIN_PROBE, name),
        format: AttributeDataType::Float4,
        offset,
    };
    let layouts = [VertexBufferLayout {
        stride: 32,
        step: VertexStep::Vertex,
        attributes: vec![attribute("pos", 0), attribute("dem_coords", 16)],
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
        state: fixed(ColorMode::unblended(), StencilState::from(&StencilMode::disabled())),
        targets: TargetFormats { color: Some(TextureFormat::Rgba8Unorm), depth_stencil: None },
    })?;
    gpu.begin_pass(&PassDesc {
        label: "terrain probes",
        color: Some(ColorAttachment { texture: target, load: LoadOp::Clear(Rgba::new(0.0, 0.0, 0.0, 1.0)) }),
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
