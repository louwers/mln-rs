use std::collections::BTreeMap;
use std::rc::Rc;

use mln_gfx::gpu::Gpu;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE, ID_PROJECTION_UBO};
use mln_gfx::stencil_mode::Test;
use mln_gfx::types::StencilOpType;
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode, ShaderProgramPtr, ShaderRegistry, StencilMode};
use mln_tile_id::UnwrappedTileID;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;

use crate::drawable::UniqueDrawable;
use crate::drawable_builder::DrawableBuilder;
use crate::gfx::context::Context;
use crate::gfx::uniform_buffer::UniformBufferPtr;
use crate::globe_tile_mesh::{RawGlobeTileMesh, raw_globe_tile_mesh};
use crate::paint_parameters::PaintParameters;
use crate::render_static_data::RenderStaticData;

/// The tile clipping masks: for each tile that the frame's layer groups can
/// clip to, drawables of the clipping mask shader that write a stencil value
/// over the tile and nothing else.
#[derive(Default)]
pub(crate) struct ClipMasks {
    /// Whether the masks are the globe's.
    globe: bool,
    shader: Option<ShaderProgramPtr>,
    /// The drawables of each tile's mask: on the globe the tile's mesh with
    /// a border and then without, elsewhere its square.
    masks: BTreeMap<UnwrappedTileID, Vec<UniqueDrawable>>,
    /// The masks' projections, each at an offset the GPU can bind, in the
    /// order of their tiles: one write a frame.
    projections: Option<UniformBufferPtr>,
    /// The globe's meshes, by zoom, whether the tile is on the top or the
    /// bottom row (with a pole cap) and whether it has a border.
    globe_meshes: BTreeMap<(u8, bool, bool, bool), RawGlobeTileMesh>,
}

impl ClipMasks {
    /// Makes and uploads the masks of `tiles`, sorted and without repeats,
    /// that `projection` projects, and drops the others. Each mask is drawn over the area that
    /// `projection` gives its tile.
    pub(crate) fn update(
        &mut self,
        context: &Context,
        shaders: &ShaderRegistry,
        globe: bool,
        tiles: &[UnwrappedTileID],
        projection: impl Fn(&UnwrappedTileID) -> ProjectionUBO,
    ) {
        if globe != self.globe {
            *self = ClipMasks { globe, ..ClipMasks::default() };
        }
        if self.shader.is_none() {
            let variant = if globe { ProjectionVariant::Globe } else { ProjectionVariant::Mercator };
            self.shader = shaders.shader("ClippingMaskProgram", variant);
        }
        let ClipMasks { shader: Some(shader), masks, globe_meshes, projections, .. } = self else {
            return;
        };
        let size = size_of::<ProjectionUBO>();
        let stride = size.next_multiple_of(context.caps().buffer_offset_alignment as usize);
        let mut bytes = vec![0; tiles.len() * stride];

        masks.retain(|tile, _| tiles.binary_search(tile).is_ok());
        let mut builder = DrawableBuilder::boxed();
        builder.set_shader(Some(Rc::clone(shader)));
        builder.set_color_mode(&ColorMode::disabled());
        builder.set_enable_depth(false);
        builder.set_vertex_attr_id(ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE);
        // The far side of a limb tile projects inside the disc; cull it or it
        // overwrites the tile in front.
        builder.set_cull_face_mode(if globe { CullFaceMode::back_ccw() } else { CullFaceMode::disabled() });

        for (slot, tile) in tiles.iter().enumerate() {
            masks.entry(*tile).or_insert_with(|| {
                if globe {
                    [true, false]
                        .into_iter()
                        .flat_map(|borders| {
                            let canonical = &tile.canonical;
                            let key =
                                (canonical.z, canonical.y == 0, canonical.y == (1u32 << canonical.z) - 1, borders);
                            let mesh = globe_meshes.entry(key).or_insert_with(|| {
                                raw_globe_tile_mesh(canonical, borders, SubdivisionGranularitySetting::globe().stencil)
                            });
                            builder.set_raw_vertices(
                                mesh.vertices.clone(),
                                mesh.vertex_count,
                                AttributeDataType::Short2,
                            );
                            builder.set_segments(DrawMode::Triangles(), mesh.indices.clone(), &mesh.segments);
                            builder.flush();
                            builder.clear_drawables()
                        })
                        .collect()
                } else {
                    let vertices = RenderStaticData::tile_vertices();
                    let indices = RenderStaticData::quad_triangle_indices();
                    let segment = SegmentBase::with_lengths(0, 0, vertices.len(), indices.len());
                    builder.set_raw_vertices(
                        bytemuck::cast_slice(&vertices).to_vec(),
                        vertices.len(),
                        AttributeDataType::Short2,
                    );
                    builder.set_segments(DrawMode::Triangles(), indices, &[segment]);
                    builder.flush();
                    builder.clear_drawables()
                }
            });
            bytes[slot * stride..][..size].copy_from_slice(bytemuck::bytes_of(&projection(tile)));
        }
        if tiles.is_empty() {
            return;
        }
        context.write_uniform_buffer(projections, &bytes);
        let Some(buffer) = projections else {
            return;
        };
        // The masks are those of `tiles`, in the same order.
        for (slot, drawables) in masks.values_mut().enumerate() {
            for drawable in drawables {
                let offset = (slot * stride) as u64;
                drawable.mutable_uniform_buffers().set_range(ID_PROJECTION_UBO, buffer, offset, size as u64);
                drawable.upload(context);
            }
        }
    }

    /// Draws the mask of each tile of `masks` with its stencil value, which
    /// it writes over the tile whatever the stencil buffer holds there.
    /// Every tile must be one `update` made a mask for.
    pub(crate) fn draw(
        &mut self,
        gpu: &mut dyn Gpu,
        masks: &[(UnwrappedTileID, i32)],
        parameters: &PaintParameters<'_>,
    ) {
        // On the globe each mask is drawn with a border first, then exactly,
        // as GL JS does: where a tile meets finer ones, its coarser edge
        // leaves a sliver between the masks that only a neighbor's border
        // covers.
        let passes = if self.globe { 2 } else { 1 };
        for pass in 0..passes {
            for (tile, stencil_ref) in masks {
                let Some(drawable) = self.masks.get_mut(tile).and_then(|drawables| drawables.get_mut(pass)) else {
                    debug_assert!(false, "no clipping mask for {tile:?}");
                    continue;
                };
                drawable.stencil_mode = Some(StencilMode {
                    test: Test::Always,
                    ref_: *stencil_ref,
                    mask: 0xFF,
                    fail: StencilOpType::Keep,
                    depth_fail: StencilOpType::Keep,
                    pass: StencilOpType::Replace,
                });
                drawable.draw(gpu, parameters, None);
            }
        }
    }
}
