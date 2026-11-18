// The globe's depth, which hides 3D geometry behind the horizon: a draw of
// each covering tile's mesh into depth only, before the layers' 3D draws.
// The draws' projections are entries of one storage buffer of the pass's.

use std::collections::BTreeMap;
use std::rc::Rc;

use mln_gfx::color_mode::ColorMode;
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::gpu::Gpu;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE, ID_PROJECTION_UBO};
use mln_gfx::{AttributeDataType, DepthMaskType, DrawMode, ShaderProgramPtr, ShaderRegistry};
use mln_map_types::TransformState;
use mln_util::range::Range;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;

use crate::draw::{DrawGeometry, DrawObjects, Textures, raw_vertices};
use crate::gfx::context::Context;
use crate::gfx::index_vector::IndexVector;
use crate::globe_tile_mesh::{RawGlobeTileMesh, raw_globe_tile_mesh};
use crate::layer_drawing::{DrawDepth, DrawList, FrameDraw, KeptDraw, LayerDraws, TileMap, tile_objects};
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
    tiles: TileMap<KeptDraw>,
    /// The frame's draws of them, which draw into depth in the translucent
    /// pass.
    frame: LayerDraws,
    /// The frame's draws as the update makes them, with their projections.
    draw_list: DrawList<ProjectionUBO>,
    shader: Option<ShaderProgramPtr>,
    meshes: BTreeMap<(u8, bool, bool), RawGlobeTileMesh>,
}

impl GlobeDepthPass {
    /// `needed` is whether any layer draws 3D geometry this frame; without
    /// one the planet's depth is never read.
    pub(crate) fn update(
        &mut self,
        shaders: &ShaderRegistry,
        context: &mut Context,
        state: &TransformState,
        update_parameters: &UpdateParameters,
        needed: bool,
    ) {
        if !state.is_globe_rendering() || !needed {
            self.tiles.clear_tiles();
            self.frame.clear();
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
        let caches = &mut self.tiles;

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

        caches.set_tiles(tile_cover.iter().copied());
        for tile_id in &tile_cover {
            let Some(entry) = caches.entry(tile_id) else {
                continue;
            };
            if entry.value.is_none() {
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
                    indexes: IndexVector::new(mesh.indices.clone()),
                    segments: mesh.segments.iter().map(SegmentBase::copy).collect(),
                    instances: None,
                };
                // The far hemisphere is clockwise on screen on every backend,
                // Vulkan included, since its shaders flip Y.
                let objects = DrawObjects::new(
                    Rc::clone(&shader),
                    ColorMode::disabled(),
                    CullFaceMode::back_ccw(),
                    geometry,
                    Textures::default(),
                );
                *entry.value = Some(KeptDraw::new(objects));
            }
            let Some(draw) = entry.value.as_ref() else {
                continue;
            };
            let depth = DrawDepth::Volume { mask: DepthMaskType::ReadWrite };
            let projection = to_projection_ubo(&state.get_projection_data(&tile_id.to_unwrapped()));
            self.draw_list.push(FrameDraw { depth, ..draw.frame_draw(Some(*tile_id)) }, projection);
        }
        // The draws draw in the order their GPU objects were made.
        self.draw_list.finish(&mut self.frame, context, |writer| {
            writer.storage(ID_PROJECTION_UBO, |projection| *projection);
        });
    }

    /// Uploads the GPU objects of the frame's draws.
    pub(crate) fn upload(&mut self, context: &mut Context) {
        self.frame.upload(&mut self.tiles, tile_objects, context);
    }

    /// Draws the frame's draws of the pass into the open pass of `gpu`, the
    /// context's.
    pub(crate) fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        self.frame.render(&mut self.tiles, tile_objects, gpu, parameters);
    }
}
