// The globe's depth, which hides 3D geometry behind the horizon: a draw of
// each covering tile's mesh into depth only, before the layers' 3D draws.

use std::collections::BTreeMap;

use mln_gfx::color_mode::ColorMode;
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::gpu::Gpu;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_map_types::View;
use mln_map_types::tile_cover::{TileCoverParameters, tile_cover};
use mln_util::range::Range;

use crate::draw_objects::Mesh;
use crate::encode::{Depth, Draw, Group};
use crate::gfx::context::Resources;
use crate::globe::{grid, surface_granularity};
use crate::handles::TileSurface;
use crate::mesh_desc::MeshDesc;
use crate::paint_parameters::PaintParameters;
use crate::shaders::programs;
use crate::tile_matrix::TileProjection;
use crate::tile_values::TileMap;
use crate::update_parameters::UpdateParameters;

/// Writes the planet into the depth buffer ahead of the translucent pass, so
/// that 3D geometry behind the horizon is hidden by the globe the way the 2D
/// layers are by their clip Z.
pub(crate) struct GlobeDepthPass {
    /// The planet's meshes, one of each tile of the cover.
    tiles: TileMap<Mesh>,
    /// The frame's draws of them, which draw into depth in the translucent
    /// pass.
    draws: Group,
    /// The buffers of each grid, which depends only on the zoom and whether
    /// the tile touches a pole.
    grids: BTreeMap<(u8, bool, bool), TileSurface>,
}

impl Default for GlobeDepthPass {
    fn default() -> Self {
        GlobeDepthPass { tiles: TileMap::default(), draws: Group::new(), grids: BTreeMap::new() }
    }
}

impl GlobeDepthPass {
    /// `needed` is whether any layer draws 3D geometry this frame; without
    /// one the planet's depth is never read.
    pub(crate) fn update(
        &mut self,
        gpu: &mut dyn Gpu,
        context: &mut Resources,
        state: &View,
        update_parameters: &UpdateParameters,
        needed: bool,
    ) {
        if !state.is_globe() || !needed {
            self.tiles.clear_tiles();
            self.draws.clear();
            if !state.is_globe() {
                self.grids.clear();
            }
            return;
        }

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
                let canonical = &tile_id.canonical;
                let key = (canonical.z, canonical.y == 0, canonical.y == (1u32 << canonical.z) - 1);
                let TileSurface { vertices, triangles } = self.grids.entry(key).or_insert_with(|| {
                    let grid = grid(canonical, surface_granularity(canonical.z), true);
                    TileSurface {
                        vertices: context.buffer(&grid.vertices),
                        triangles: context.index_buffer(grid.triangles.as_flattened()),
                    }
                });
                let mut mesh = MeshDesc::new(&programs::GLOBE_DEPTH, triangles);
                // The far hemisphere is clockwise on screen on every backend,
                // Vulkan included, since its shaders flip Y.
                mesh.vertices(vertices)
                    .blend(ColorMode::disabled())
                    .cull(CullFaceMode::back_ccw())
                    .projection(ProjectionVariant::Globe);
                *value = Some(context.mesh(&mesh));
            }
            let Some(mesh) = value.as_ref() else {
                continue;
            };
            let tile = tile_id.to_unwrapped();
            let at = TileProjection::of(&tile, state, &state.projection_data(&tile), None);
            self.draws.push_at(
                mesh,
                &at,
                Draw { depth: Depth::Volume { write: true, once: false }, ..Draw::DEFAULT },
                (),
            );
        }
        self.draws.finish(gpu, context, &(), []);
    }

    /// Uploads the meshes of the frame's draws.
    pub(crate) fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Resources) {
        self.draws.upload(gpu, context);
    }

    /// Draws the frame's draws of the pass into the open pass of `gpu`, the
    /// context's.
    pub(crate) fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        self.draws.render(gpu, parameters);
    }
}
