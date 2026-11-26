use std::collections::BTreeMap;

use mln_gfx::gpu::{
    BindGroupEntry, BindingsDesc, BindingsId, BufferUsage, DepthState, Draw, DynamicState, FixedState, GeometryDesc,
    GeometryId, Gpu, GpuError, PipelineId, PipelineKey, Scissor, ShaderId, ShaderKey, StencilState, TargetFormats,
    VertexAttribute, VertexBufferLayout, VertexStep,
};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::bindings::{BUFFER_IDS, PROJECTION_ID};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::stencil_mode::Test;
use mln_gfx::types::StencilOpType;
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DepthMode, DrawModeType, StencilMode};
use mln_style::subdivision_granularity::SubdivisionGranularitySetting;
use mln_tile_id::UnwrappedTileID;

use crate::draw::Entries;
use crate::gfx::buffer::Buffer;
use crate::gfx::context::Context;
use crate::gfx::owned::Owned;
use crate::gfx::segment::SegmentBase;
use crate::gfx::vertex_attribute::Uniforms;
use crate::globe_tile_mesh::raw_globe_tile_mesh;
use crate::paint_parameters::DrawTarget;
use crate::render_static_data::RenderStaticData;
use crate::shaders::programs;

/// The vertex layout of the masks' meshes: a tile position of two shorts.
fn layout() -> VertexBufferLayout {
    VertexBufferLayout {
        stride: 4,
        step: VertexStep::Vertex,
        attributes: vec![VertexAttribute {
            location: const { programs::CLIPPING_MASK.location("pos") as u32 },
            format: AttributeDataType::Short2,
            offset: 0,
        }],
    }
}

/// What a mask's mesh is of: on the globe the zoom, whether the tile is on
/// the top or the bottom row (with a pole cap) and whether it has a border;
/// elsewhere [`SQUARE`].
type MeshKey = (u8, bool, bool, bool);

/// The key of the square, which every mask off the globe draws.
const SQUARE: MeshKey = (0, false, false, false);

/// A mesh that masks draw, and its geometry, made at its first draw.
struct Mesh {
    vertices: Buffer,
    indices: Buffer,
    segments: Vec<SegmentBase>,
    geometry: Option<Owned<GeometryId>>,
}

impl Mesh {
    fn new(
        gpu: &mut dyn Gpu,
        context: &mut Context,
        vertices: &[u8],
        indices: &[u16],
        segments: Vec<SegmentBase>,
    ) -> Option<Mesh> {
        let device = context.device();
        let made = device.buffer(gpu, BufferUsage::Vertex, vertices).and_then(|vertices| {
            Ok((vertices, device.buffer(gpu, BufferUsage::Index, bytemuck::cast_slice(indices))?))
        });
        match made {
            Ok((vertices, indices)) => Some(Mesh { vertices, indices, segments, geometry: None }),
            Err(error) => {
                context.fail(format!("Making a clipping mask's mesh failed: {error}"));
                None
            }
        }
    }

    fn geometry(&mut self, gpu: &mut dyn Gpu, context: &Context, shader: ShaderId) -> Result<GeometryId, GpuError> {
        if let Some(geometry) = &self.geometry {
            return Ok(geometry.id());
        }
        let geometry = gpu.create_geometry(&GeometryDesc {
            shader,
            layouts: &[layout()],
            vertex_buffers: &[(self.vertices.id(), 0)],
            index_buffer: self.indices.id(),
        })?;
        self.geometry = Some(context.own(geometry, Vec::new()));
        Ok(geometry)
    }
}

/// The tile clipping masks: for each tile that the frame's draw groups can
/// clip to, a mesh of the clipping mask shader that writes a stencil value
/// over the tile and nothing else.
#[derive(Default)]
pub(crate) struct ClipMasks {
    /// Whether the masks are the globe's.
    globe: bool,
    shader: Option<ShaderKey>,
    shader_id: Option<ShaderId>,
    /// The meshes of the masks.
    meshes: BTreeMap<MeshKey, Mesh>,
    /// Each tile's slot of `projections` and the keys of its meshes: on the
    /// globe with a border and then without, elsewhere the square twice.
    masks: BTreeMap<UnwrappedTileID, (usize, [MeshKey; 2])>,
    /// The masks' projections, each at an offset the GPU can bind, in the
    /// order of their tiles: one write a frame.
    projections: Option<Buffer>,
    /// The bytes of `projections`, kept between frames.
    bytes: Vec<u8>,
    /// The bindings of each slot of `projections`, which go with the
    /// projections' buffer, and the epoch of the global buffers they were
    /// made at.
    bindings: Vec<Owned<BindingsId>>,
    bindings_made: Option<u64>,
    pipelines: Vec<(TargetFormats, PipelineId)>,
}

impl ClipMasks {
    /// Makes and uploads the masks of `tiles`, sorted and without repeats,
    /// that `projection` projects, and drops the others. Each mask is drawn
    /// over the area that `projection` gives its tile.
    pub(crate) fn update(
        &mut self,
        gpu: &mut dyn Gpu,
        context: &mut Context,
        globe: bool,
        tiles: &[UnwrappedTileID],
        projection: impl Fn(&UnwrappedTileID) -> ProjectionUBO,
    ) {
        if globe != self.globe {
            *self = ClipMasks { globe, ..ClipMasks::default() };
        }
        self.shader.get_or_insert_with(|| {
            let variant = if globe { ProjectionVariant::Globe } else { ProjectionVariant::Mercator };
            context.shader_key(&programs::CLIPPING_MASK, Uniforms::default(), variant)
        });
        if tiles.is_empty() {
            self.masks.clear();
            self.meshes.clear();
            self.projections = None;
            self.bindings.clear();
            self.bindings_made = None;
            return;
        }
        let size = size_of::<ProjectionUBO>();
        let stride = size.next_multiple_of(context.caps().buffer_offset_alignment as usize);
        self.bytes.clear();
        self.bytes.resize(tiles.len() * stride, 0);
        let tiles_before = self.masks.len();
        self.masks.retain(|tile, _| tiles.binary_search(tile).is_ok());
        let tiles_gone = self.masks.len() < tiles_before;
        for (slot, tile) in tiles.iter().enumerate() {
            self.bytes[slot * stride..][..size].copy_from_slice(bytemuck::bytes_of(&projection(tile)));
            if let Some((kept, _)) = self.masks.get_mut(tile) {
                *kept = slot;
                continue;
            }
            let keys = if globe {
                let canonical = &tile.canonical;
                [true, false]
                    .map(|borders| (canonical.z, canonical.y == 0, canonical.y == (1u32 << canonical.z) - 1, borders))
            } else {
                [SQUARE; 2]
            };
            for key in keys {
                if self.meshes.contains_key(&key) {
                    continue;
                }
                let mesh = if globe {
                    let (_, _, _, borders) = key;
                    let raw =
                        raw_globe_tile_mesh(&tile.canonical, borders, SubdivisionGranularitySetting::globe().stencil);
                    Mesh::new(gpu, context, &raw.vertices, &raw.indices, raw.segments)
                } else {
                    let vertices = RenderStaticData::tile_vertices();
                    let indices = RenderStaticData::quad_triangle_indices();
                    let segment = SegmentBase::with_lengths(0, 0, vertices.len(), indices.len());
                    Mesh::new(gpu, context, bytemuck::cast_slice(&vertices), &indices, vec![segment])
                };
                if let Some(mesh) = mesh {
                    self.meshes.insert(key, mesh);
                }
            }
            self.masks.insert(*tile, (slot, keys));
        }
        // Those of the tiles gone go with them.
        if tiles_gone {
            let masks = &self.masks;
            self.meshes.retain(|key, _| masks.values().any(|(_, keys)| keys.contains(key)));
        }
        if context.device().write_uniforms(gpu, &mut self.projections, &self.bytes, false) {
            self.bindings.clear();
            self.bindings_made = None;
        }
    }

    /// Draws the mask of each tile of `masks` with its stencil value, which
    /// it writes over the tile whatever the stencil buffer holds there.
    /// Every tile must be one `update` made a mask for.
    pub(crate) fn draw(
        &mut self,
        gpu: &mut dyn Gpu,
        masks: &[(UnwrappedTileID, i32)],
        target: DrawTarget,
        context: &mut Context,
    ) {
        if let Err(error) = self.draw_in(gpu, masks, target, context) {
            context.fail(format!("Drawing the clipping masks failed: {error}"));
        }
    }

    fn draw_in(
        &mut self,
        gpu: &mut dyn Gpu,
        masks: &[(UnwrappedTileID, i32)],
        target: DrawTarget,
        context: &mut Context,
    ) -> Result<(), GpuError> {
        let (Some(shader), Some(projections)) = (&self.shader, &self.projections) else {
            return Ok(());
        };
        let DrawTarget { size, targets, scissor } = target;
        let shader_id = match self.shader_id {
            Some(id) => id,
            None => *self.shader_id.insert(context.shader(gpu, shader)?),
        };

        let made = context.global_uniform_buffers().epoch();
        if self.bindings_made != Some(made) || self.bindings.len() < self.masks.len() {
            self.bindings.clear();
            self.bindings_made = None;
            let global = context.global_uniform_buffers();
            let size = size_of::<ProjectionUBO>();
            let stride = size.next_multiple_of(context.caps().buffer_offset_alignment as usize);
            for slot in 0..projections.size() / stride {
                let mut entries = Entries::new();
                for id in 0..BUFFER_IDS {
                    let (buffer, offset, size) = if id == PROJECTION_ID {
                        (projections, (slot * stride) as u64, size as u64)
                    } else if let Some(buffer) = global.get(id) {
                        (buffer, 0, buffer.size() as u64)
                    } else {
                        continue;
                    };
                    entries.push(BindGroupEntry::Buffer { id: id as u32, buffer: buffer.id(), offset, size });
                }
                let bindings = gpu.create_bindings(&BindingsDesc {
                    label: "clipping mask",
                    shader: shader_id,
                    entries: entries.as_slice(),
                })?;
                self.bindings.push(context.own(bindings, Vec::new()));
            }
            self.bindings_made = Some(made);
        }

        let stencil = StencilMode {
            test: Test::Always,
            ref_: 0,
            mask: 0xFF,
            fail: StencilOpType::Keep,
            depth_fail: StencilOpType::Keep,
            pass: StencilOpType::Replace,
        };
        let pipeline = if let Some(&(_, id)) = self.pipelines.iter().find(|(cached, _)| *cached == targets) {
            id
        } else {
            let disabled = ColorMode::disabled();
            let key = PipelineKey {
                shader: shader_id,
                vertex_buffers: vec![layout()],
                state: FixedState {
                    primitive: DrawModeType::Triangles,
                    // The far side of a limb tile projects inside the disc;
                    // cull it or it overwrites the tile in front.
                    cull: if self.globe { CullFaceMode::back_ccw() } else { CullFaceMode::disabled() },
                    blend: disabled.blend_function,
                    color_mask: disabled.mask,
                    depth: DepthState::from(&DepthMode::disabled()),
                    stencil: StencilState::from(&stencil),
                },
                targets,
            };
            let id = context.pipeline(gpu, &key)?;
            self.pipelines.push((targets, id));
            id
        };

        let full = Scissor { x: 0, y: 0, width: size.width, height: size.height };
        let dynamic = DynamicState {
            depth_range: DepthMode::disabled().range,
            scissor: scissor.unwrap_or(full),
            stencil_reference: 0,
            blend_constant: ColorMode::disabled().blend_color,
            line_width: 1.0,
        };
        // On the globe each mask is drawn with a border first, then exactly:
        // where a tile meets finer ones, its coarser edge leaves a sliver
        // between the masks that only a neighbor's border covers.
        let passes = if self.globe { 2 } else { 1 };
        let mut draws = 0;
        for pass in 0..passes {
            for (tile, stencil_ref) in masks {
                let Some((slot, keys)) = self.masks.get(tile) else {
                    debug_assert!(false, "no clipping mask for {tile:?}");
                    continue;
                };
                let Some(mesh) = self.meshes.get_mut(&keys[pass]) else {
                    continue;
                };
                let geometry = mesh.geometry(gpu, context, shader_id)?;
                let bindings = self.bindings[*slot].id();
                for segment in &mesh.segments {
                    gpu.draw(&Draw {
                        pipeline,
                        bindings,
                        geometry,
                        first_index: segment.index_offset as u32,
                        index_count: segment.index_length as u32,
                        base_vertex: segment.vertex_offset as i32,
                        first_instance: 0,
                        instance_count: 1,
                        global_index: 0,
                        dynamic: DynamicState { stencil_reference: *stencil_ref as u32, ..dynamic },
                    })?;
                    draws += 1;
                }
            }
        }
        let stats = context.rendering_stats();
        stats.num_draw_calls += draws;
        stats.total_draw_calls += draws;
        Ok(())
    }
}
