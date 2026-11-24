//! The terrain's height rule on the GPU, against `mln_util::dem` on the CPU,
//! and the depth of the ground that the terrain's depth program draws.

use std::cell::Cell;

use mln_util::Color;
use mln_util::dem::{self, DemCoords, DemUnpack};
use mln_util::size::Size;
use mln_util::tileset::RasterEncoding;

use super::{EXTENT, buffer, draw, dynamic, fixed, whole};
use crate::color_mode::ColorMode;
use crate::gfx_types::AttributeDataType;
use crate::gpu::{
    BindGroupEntry, BindingsDesc, BindingsId, BufferUsage, ColorAttachment, DepthState, DepthStencilAttachment, Draw,
    DynamicState, FixedState, GeometryDesc, Gpu, GpuError, LoadOp, PassDesc, PipelineDesc, Scissor, ShaderId,
    ShaderKey, StencilState, TargetFormats, TextureDesc, TextureFormat, TextureId, TextureRegion, TextureUsage,
    VertexAttribute, VertexBufferLayout, VertexStep, read_texels,
};
use crate::shaders::layer_ubo::ProjectionUBO;
use crate::shaders::programs;
use crate::shaders::shader_defines::{
    ID_PROJECTION_UBO, ID_TERRAIN_DEM_TEXTURE, ID_TERRAIN_DRAWABLE_UBO, ID_TERRAIN_POS_VERTEX_ATTRIBUTE,
    ID_TERRAIN_PROBE_DEM_COORDS_VERTEX_ATTRIBUTE, ID_TERRAIN_PROBE_POS_VERTEX_ATTRIBUTE, ID_TERRAIN_PROPS_UBO,
};
use crate::shaders::terrain_ubo::{TerrainDrawableUBO, TerrainPropsUBO};
use crate::stencil_mode::StencilMode;
use crate::texture2d::SamplerState;
use crate::types::DepthFunctionType;

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
/// `mln_util::dem` reads on the CPU, to within 1 mm and 4 ulp of the
/// largest height interpolated.
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
    let props = TerrainPropsUBO {
        unpack: dem.unpack.weights_and_base(),
        unpack_scale: dem.unpack.scale,
        exaggeration: dem.exaggeration,
        ..TerrainPropsUBO::default()
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
            BindGroupEntry::Texture { id: ID_TERRAIN_DEM_TEXTURE as u32, texture, sampler },
            whole(ID_TERRAIN_PROPS_UBO, props_buffer, props),
            whole(ID_PROJECTION_UBO, projection_buffer, projection),
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
    let attribute = |id: usize, offset| VertexAttribute { id: id as u32, format: AttributeDataType::Float4, offset };
    let layouts = [VertexBufferLayout {
        stride: 32,
        step: VertexStep::Vertex,
        attributes: vec![
            attribute(ID_TERRAIN_PROBE_POS_VERTEX_ATTRIBUTE, 0),
            attribute(ID_TERRAIN_PROBE_DEM_COORDS_VERTEX_ATTRIBUTE, 16),
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
        state: fixed(ColorMode::unblended(), StencilState::from(&StencilMode::disabled())),
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

/// The size of the ground's depth that [`ground_depth`] draws.
const DEPTH_SIZE: Size = Size { width: 8, height: 4 };

/// A tile of the ground that [`ground_depth`] draws: the part of the target
/// it covers, in NDC x0, y0, x1, y1, its tile's y 0 at y1, its ground, the
/// eye distance there, and its NDC depth.
struct DepthTile {
    ndc: [f32; 4],
    dem: Dem,
    /// The eye distance is `eye[0] + eye[1]` times the height.
    eye: [f32; 2],
    depth: f32,
}

impl DepthTile {
    /// The ground's height under the tile, the same everywhere.
    fn height(&self) -> f32 {
        if self.dem.dim == 0 { 0.0 } else { self.dem.sample(DemCoords::OWN, [0.0, 0.0]).0 }
    }

    /// The eye distance of the tile's ground.
    fn eye_distance(&self) -> f32 {
        self.eye[0] + self.eye[1] * self.height()
    }

    /// Its draw's matrix: clip w is the eye distance, which the height
    /// raises, and clip x, y and z place the tile at `ndc` and `depth` for
    /// the height it stands at.
    fn matrix(&self) -> [f32; 16] {
        let [x0, y0, x1, y1] = self.ndc;
        let w = self.eye_distance();
        let e = EXTENT;
        [
            (x1 - x0) * w / e,
            0.0,
            0.0,
            0.0,
            0.0,
            (y0 - y1) * w / e,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            self.eye[1],
            x0 * w,
            y1 * w,
            self.depth * w,
            self.eye[0],
        ]
    }
}

/// A DEM of `dim` pixels whose texels all decode to `height` meters, by
/// Mapbox's encoding.
fn level_dem(dim: u32, height: f32) -> Dem {
    let code = ((height + 10_000.0) * 10.0).round() as u32;
    let rgb = [(code >> 16) as u8, (code >> 8) as u8, code as u8];
    let side = dim + 2;
    let texels = (0..side * side).flat_map(|_| [rgb[0], rgb[1], rgb[2], 255]).collect();
    Dem { unpack: DemUnpack::of(RasterEncoding::Mapbox), exaggeration: 1.0, dim, texels }
}

/// Checks that the terrain's depth program on `gpu` writes the eye distance
/// of the nearest ground, as its f32's bits, least significant byte first,
/// into the rows that a readback gives from the top: a tile over the top
/// half, at an eye distance of 2, nearer than one over the left three
/// quarters, at 3, raised by its DEM, which draws after it; and no ground,
/// 0, where no tile draws. Each texel holds its eye distance to within 4
/// ulp. Returns the error of a call that should succeed.
///
/// # Panics
///
/// Where a texel doesn't hold its tile's eye distance.
pub fn ground_depth(gpu: &mut impl Gpu) -> Result<(), GpuError> {
    let shader = gpu.create_shader(&ShaderKey::new(&programs::TERRAIN_DEPTH, []))?;
    let target = gpu.create_texture(&TextureDesc {
        label: "ground depth",
        size: DEPTH_SIZE,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsage { render_target: true, copy_src: true, ..TextureUsage::default() },
        mipmapped: false,
    })?;
    let depth_test = gpu.create_texture(&TextureDesc {
        label: "ground depth test",
        size: DEPTH_SIZE,
        format: TextureFormat::Depth24PlusStencil8,
        usage: TextureUsage { render_target: true, ..TextureUsage::default() },
        mipmapped: false,
    })?;
    let near = DepthTile { ndc: [-1.0, 0.0, 1.0, 1.0], dem: Dem::flat(), eye: [2.0, 0.0], depth: 0.2 };
    let far = DepthTile { ndc: [-1.0, -1.0, 0.5, 1.0], dem: level_dem(4, 100.0), eye: [2.0, 0.01], depth: 0.3 };

    let e = EXTENT as i16;
    let quad: [[i16; 4]; 4] = [[0, 0, 0, 0], [e, 0, 0, 0], [0, e, 0, 0], [e, e, 0, 0]];
    let vertices = buffer(gpu, BufferUsage::Vertex, bytemuck::cast_slice(&quad))?;
    let indices = buffer(gpu, BufferUsage::Index, bytemuck::cast_slice(&[0u16, 1, 2, 1, 2, 3]))?;
    let layouts = [VertexBufferLayout {
        stride: 8,
        step: VertexStep::Vertex,
        attributes: vec![VertexAttribute {
            id: ID_TERRAIN_POS_VERTEX_ATTRIBUTE as u32,
            format: AttributeDataType::Short4,
            offset: 0,
        }],
    }];
    let geometry = gpu.create_geometry(&GeometryDesc {
        shader,
        layouts: &layouts,
        vertex_buffers: &[(vertices, 0)],
        index_buffer: indices,
    })?;
    let depth = DepthState { compare: DepthFunctionType::Less, write: true };
    let pipeline = gpu.create_pipeline(&PipelineDesc {
        shader,
        vertex_buffers: &layouts,
        state: FixedState { depth, ..fixed(ColorMode::unblended(), StencilState::from(&StencilMode::disabled())) },
        targets: TargetFormats {
            color: Some(TextureFormat::Rgba8Unorm),
            depth_stencil: Some(TextureFormat::Depth24PlusStencil8),
        },
    })?;
    let bindings =
        [&near, &far].map(|tile| depth_bindings(gpu, shader, tile)).into_iter().collect::<Result<Vec<_>, _>>()?;

    gpu.begin_pass(&PassDesc {
        label: "ground depth",
        color: Some(ColorAttachment { texture: target, load: LoadOp::Clear(Color::new(0.0, 0.0, 0.0, 0.0)) }),
        depth_stencil: Some(DepthStencilAttachment {
            texture: depth_test,
            depth: LoadOp::Clear(1.0),
            stencil: LoadOp::Clear(0),
        }),
    })?;
    let scissor = Scissor { x: 0, y: 0, width: DEPTH_SIZE.width, height: DEPTH_SIZE.height };
    let dynamic = DynamicState { scissor, ..dynamic() };
    for bindings in bindings {
        gpu.draw(&Draw { dynamic, ..draw(pipeline, bindings, geometry, 6) })?;
    }
    gpu.end_pass()?;

    let texels = read_texels(gpu, target, TextureRegion::whole(DEPTH_SIZE))?;
    let distances: Vec<f32> = texels.as_chunks::<4>().0.iter().map(|texel| f32::from_le_bytes(*texel)).collect();
    let width = DEPTH_SIZE.width as usize;
    let mut failures = Vec::new();
    for (index, &distance) in distances.iter().enumerate() {
        let (column, row) = (index % width, index / width);
        let expected = if row < 2 {
            near.eye_distance()
        } else if column < 6 {
            far.eye_distance()
        } else {
            0.0
        };
        let ulp = f32::from_bits(expected.to_bits() + 1) - expected;
        if distance.is_nan() || (distance - expected).abs() > 4.0 * ulp {
            failures.push(format!("column {column}, row {row}: {distance}, not {expected}"));
        }
    }
    assert!(failures.is_empty(), "the ground's depth differs:\n{}", failures.join("\n"));
    Ok(())
}

/// The bindings of a draw of `tile` with the depth program `shader`: its
/// matrix and DEM, which it reads at its own coordinates.
fn depth_bindings(gpu: &mut impl Gpu, shader: ShaderId, tile: &DepthTile) -> Result<BindingsId, GpuError> {
    let texture = dem_texture(gpu, &tile.dem)?;
    let sampler = gpu.create_sampler(&SamplerState::default())?;
    let dem_coords = [1.0, 0.0, 0.0, tile.dem.dim as f32];
    let drawable = [TerrainDrawableUBO { dem_coords }];
    let drawable = bytemuck::cast_slice(&drawable);
    let drawable_buffer = buffer(gpu, BufferUsage::Storage, drawable)?;
    let props = TerrainPropsUBO {
        unpack: tile.dem.unpack.weights_and_base(),
        unpack_scale: tile.dem.unpack.scale,
        exaggeration: tile.dem.exaggeration,
        ..TerrainPropsUBO::default()
    };
    let props = bytemuck::bytes_of(&props);
    let props_buffer = buffer(gpu, BufferUsage::Uniform, props)?;
    let projection = [ProjectionUBO { matrix: tile.matrix(), ..ProjectionUBO::default() }];
    let projection = bytemuck::cast_slice(&projection);
    let projection_buffer = buffer(gpu, BufferUsage::Storage, projection)?;
    gpu.create_bindings(&BindingsDesc {
        label: "ground depth",
        shader,
        entries: &[
            BindGroupEntry::Texture { id: ID_TERRAIN_DEM_TEXTURE as u32, texture, sampler },
            whole(ID_TERRAIN_DRAWABLE_UBO, drawable_buffer, drawable),
            whole(ID_TERRAIN_PROPS_UBO, props_buffer, props),
            whole(ID_PROJECTION_UBO, projection_buffer, projection),
        ],
    })
}

/// The texture of `dem`'s texels.
fn dem_texture(gpu: &mut impl Gpu, dem: &Dem) -> Result<TextureId, GpuError> {
    let side = dem.side();
    let texture = gpu.create_texture(&TextureDesc {
        label: "dem",
        size: Size::new(side, side),
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsage { sampled: true, ..TextureUsage::default() },
        mipmapped: false,
    })?;
    gpu.write_texture(texture, TextureRegion::whole(Size::new(side, side)), &dem.texels)?;
    Ok(texture)
}
