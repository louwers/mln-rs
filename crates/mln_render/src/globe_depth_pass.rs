// The globe's depth, which hides 3D geometry behind the horizon: a draw of
// each covering tile's mesh into depth only, before the layers' 3D draws.
// The draws' projections are entries of one storage buffer of the pass's.

use std::collections::BTreeMap;

use mln_gfx::color_mode::ColorMode;
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::gpu::Gpu;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::program::PROJECTION_BUFFER;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::{AttributeDataType, DepthMaskType, DrawMode};
use mln_map_types::View;
use mln_map_types::tile_cover::{TileCoverParameters, tile_cover};
use mln_util::range::Range;

use crate::draw_objects::{DrawGeometry, DrawObjects, Mesh, Textures, raw_vertices};
use crate::gfx::context::Context;
use crate::gfx::indexes::DrawIndexes;
use crate::gfx::vertex_attribute::Uniforms;
use crate::globe::{TileGrid, grid, surface_granularity};
use crate::layer_drawing::{DrawDepth, DrawList, FrameDraw, FrameDraws};
use crate::paint_parameters::PaintParameters;
use crate::shaders::programs;
use crate::shaders::programs::ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE;
use crate::tile_matrix::to_projection_ubo;
use crate::tile_values::TileMap;
use crate::update_parameters::UpdateParameters;

/// Writes the planet into the depth buffer ahead of the translucent pass, so
/// that 3D geometry behind the horizon is hidden by the globe the way the 2D
/// layers are by their clip Z.
#[derive(Default)]
pub(crate) struct GlobeDepthPass {
    /// The planet's meshes, one of each tile of the cover.
    tiles: TileMap<Mesh>,
    /// The frame's draws of them, which draw into depth in the translucent
    /// pass.
    frame: FrameDraws,
    /// The frame's draws as the update makes them, with their projections.
    draw_list: DrawList<ProjectionUBO>,
    meshes: BTreeMap<(u8, bool, bool), TileGrid>,
}

impl GlobeDepthPass {
    /// `needed` is whether any layer draws 3D geometry this frame; without
    /// one the planet's depth is never read.
    pub(crate) fn update(
        &mut self,
        gpu: &mut dyn Gpu,
        context: &mut Context,
        state: &View,
        update_parameters: &UpdateParameters,
        needed: bool,
    ) {
        if !state.is_globe() || !needed {
            self.tiles.clear_tiles();
            self.frame.clear();
            if !state.is_globe() {
                self.meshes.clear();
            }
            return;
        }

        let shader = context.shader_key(&programs::GLOBE_DEPTH, Uniforms::default(), ProjectionVariant::Globe);
        let caches = &mut self.tiles;

        let zoom = state.integer_zoom();
        let tile_cover = tile_cover(
            &TileCoverParameters {
                view: state.clone(),
                tile_lod_min_radius: update_parameters.tile_lod_min_radius,
                tile_lod_scale: update_parameters.tile_lod_scale,
                tile_lod_pitch_threshold: update_parameters.tile_lod_pitch_threshold,
                tile_lod_mode: update_parameters.tile_lod_mode,
                requested_zoom: None,
                round_zoom: false,
                max_content_elevation: 0.0,
            },
            zoom,
            Range::new(0u8, zoom),
            None,
        );

        caches.set_tiles(tile_cover.iter().copied());
        for (index, tile_id) in tile_cover.iter().enumerate() {
            let value = caches.get_mut(index);
            if value.is_none() {
                // The grid depends only on the zoom and whether the tile
                // touches a pole.
                let canonical = &tile_id.canonical;
                let key = (canonical.z, canonical.y == 0, canonical.y == (1u32 << canonical.z) - 1);
                let mesh =
                    self.meshes.entry(key).or_insert_with(|| grid(canonical, surface_granularity(canonical.z), true));
                let vertex_count = mesh.vertices.len();
                let indices = mesh.triangles.as_flattened().to_vec();
                let vertices = raw_vertices(
                    ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE,
                    bytemuck::cast_slice(&mesh.vertices).to_vec(),
                    vertex_count,
                    AttributeDataType::Short2,
                );
                let geometry = DrawGeometry {
                    vertices,
                    vertex_count,
                    mode: DrawMode::Triangles,
                    segments: vec![SegmentBase::with_lengths(0, 0, vertex_count, indices.len())],
                    indexes: DrawIndexes::new(indices),
                    instances: None,
                };
                // The far hemisphere is clockwise on screen on every backend,
                // Vulkan included, since its shaders flip Y.
                let objects = DrawObjects::new(
                    shader.clone(),
                    ColorMode::disabled(),
                    CullFaceMode::back_ccw(),
                    geometry,
                    Textures::default(),
                );
                *value = Some(context.add_mesh(objects));
            }
            let Some(mesh) = value.as_ref() else {
                continue;
            };
            let mut draw = FrameDraw::new(Some(*tile_id), mesh);
            draw.depth = DrawDepth::Volume { mask: DepthMaskType::ReadWrite };
            let projection = to_projection_ubo(&state.projection_data(&tile_id.to_unwrapped()));
            self.draw_list.push(draw, projection);
        }
        // The draws draw in the order their meshes were made.
        self.draw_list.finish(&mut self.frame, gpu, context, |writer| {
            writer.storage(PROJECTION_BUFFER.id, |projection| *projection);
        });
    }

    /// Uploads the meshes of the frame's draws.
    pub(crate) fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        self.frame.upload(gpu, context);
    }

    /// Draws the frame's draws of the pass into the open pass of `gpu`, the
    /// context's.
    pub(crate) fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        self.frame.render(gpu, parameters);
    }
}
