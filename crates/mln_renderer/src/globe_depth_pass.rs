// The `MLN_UBO_CONSOLIDATION` path, as for the WebGPU backend: the projection
// blocks live in one array of the pass's, so the `GlobeDepthTweaker` the
// other backends use is not ported.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use mln_gfx::color_mode::ColorMode;
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::gpu::Gpu;
use mln_gfx::index_vector::IndexVectorBase;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE, ID_PROJECTION_UBO};
use mln_gfx::{AttributeDataType, DepthMaskType, DrawMode, ShaderProgramPtr, ShaderRegistry};
use mln_map_types::TransformState;
use mln_util::range::Range;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;

use crate::draw::{DrawGeometry, DrawObjects, raw_vertices};
use crate::draw_cache::{DrawCaches, DrawDepth, FrameDraw, FrameDraws, KeptDraw};
use crate::gfx::context::Context;
use crate::globe_tile_mesh::{RawGlobeTileMesh, raw_globe_tile_mesh};
use crate::paint_parameters::PaintParameters;
use crate::tile_matrix::to_projection_ubo;
use crate::update_parameters::UpdateParameters;
use crate::util::tile_cover::{TileCoverParameters, tile_cover};

const GLOBE_DEPTH_SHADER_NAME: &str = "GlobeDepthShader";

/// Writes the planet into the depth buffer ahead of the translucent pass, so
/// that 3D geometry behind the horizon is hidden by the globe the way the 2D
/// layers are by their clip Z.
#[derive(Default)]
pub(crate) struct GlobeDepthPass {
    /// The planet's draws, one of each tile of the cover.
    tiles: DrawCaches<KeptDraw>,
    /// The frame's draws of the tiles, from the first frame that draws the
    /// planet.
    frame: Option<FrameDraws>,
    shader: Option<ShaderProgramPtr>,
    meshes: BTreeMap<(u8, bool, bool), RawGlobeTileMesh>,
}

impl GlobeDepthPass {
    /// `needed` is whether any layer draws 3D geometry this frame; without
    /// one the planet's depth is never read.
    pub(crate) fn update(
        &mut self,
        shaders: &ShaderRegistry,
        context: &Context,
        state: &TransformState,
        update_parameters: &UpdateParameters,
        needed: bool,
    ) {
        if !state.is_globe_rendering() || !needed {
            self.tiles.clear_tiles();
            if let Some(frame) = &mut self.frame {
                frame.draws.clear();
            }
            if !state.is_globe_rendering() {
                self.meshes.clear();
            }
            return;
        }

        if self.shader.is_none() {
            self.shader = shaders.shader(GLOBE_DEPTH_SHADER_NAME, ProjectionVariant::Globe);
        }
        let Some(shader) = self.shader.clone() else {
            return;
        };
        let frame = self.frame.get_or_insert_with(FrameDraws::new);

        let zoom = state.get_integer_zoom();
        let tile_cover = tile_cover(
            &TileCoverParameters {
                transform_state: state.clone(),
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

        self.tiles.set_tiles(tile_cover.iter().copied());
        frame.draws.clear();
        for tile_id in &tile_cover {
            if self.tiles.cache_mut(tile_id).is_none() {
                // The grid depends only on the zoom and whether the tile
                // touches a pole.
                let canonical = &tile_id.canonical;
                let key = (canonical.z, canonical.y == 0, canonical.y == (1u32 << canonical.z) - 1);
                let mesh = self.meshes.entry(key).or_insert_with(|| {
                    raw_globe_tile_mesh(canonical, true, SubdivisionGranularitySetting::globe().tile)
                });
                let vertices = raw_vertices(
                    ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE,
                    mesh.vertices.clone(),
                    mesh.vertex_count,
                    AttributeDataType::Short2,
                );
                let geometry = DrawGeometry {
                    vertices,
                    vertex_count: mesh.vertex_count,
                    mode: DrawMode::Triangles(),
                    indexes: Rc::new(RefCell::new(IndexVectorBase::new(mesh.indices.clone()))),
                    segments: mesh.segments.iter().map(SegmentBase::copy).collect(),
                };
                // The far hemisphere is clockwise on screen on every backend,
                // Vulkan included, since its shaders flip Y.
                let objects = DrawObjects::new(
                    String::new(),
                    Rc::clone(&shader),
                    ColorMode::disabled(),
                    CullFaceMode::back_ccw(),
                    geometry,
                );
                self.tiles.set_cache(tile_id, KeptDraw::new(objects));
            }
            let Some(draw) = self.tiles.cache_mut(tile_id) else {
                continue;
            };
            let depth = DrawDepth::Volume { mask: DepthMaskType::ReadWrite };
            frame.draws.push(FrameDraw { depth, ..draw.frame_draw(Some(*tile_id)) });
        }
        // The draws draw in the order their GPU objects were made.
        frame.draws.sort_by_key(FrameDraw::order);

        // The projection blocks live in one array, indexed like the draws.
        let projection_ubos: Vec<ProjectionUBO> = frame
            .draws
            .iter()
            .filter_map(|draw| draw.tile)
            .map(|tile_id| to_projection_ubo(&state.get_projection_data(&tile_id.to_unwrapped())))
            .collect();
        if !projection_ubos.is_empty() {
            frame.uniform_buffers.write_storage(ID_PROJECTION_UBO, bytemuck::cast_slice(&projection_ubos), context);
        }
    }

    /// Uploads the GPU objects of the planet's draws, before the frame draws
    /// them.
    pub(crate) fn upload(&mut self, context: &Context) {
        if let Some(frame) = &self.frame {
            self.tiles.upload(frame, context);
        }
    }

    /// Draws the planet's depth into the open pass of `gpu`, the context's,
    /// in the translucent pass.
    pub(crate) fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        if let Some(frame) = &self.frame {
            self.tiles.render(frame, gpu, parameters);
        }
    }
}
