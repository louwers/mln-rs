// The globe's depth, which hides 3D geometry behind the horizon: a draw of
// each covering tile's mesh into depth only, before the layers' 3D draws.
// The draws' projections are entries of one storage buffer of the pass's.
//
// The planet draws a little farther from the camera than its surface
// ([`ground_setback`], [`DEPTH_SETBACK`]). A layer's geometry on the
// ground, such as an extrusion's roof at height 0, bends onto the sphere
// through a coarser mesh than the planet's, whose chords sag deeper below the
// surface, or through a mesh as fine but split along other diagonals. Drawn
// at the surface, the planet would hide that geometry, in patches that depend
// on the GPU's rounding.

use std::collections::BTreeMap;

use glam::{DMat4, DVec3};
use mln_gfx::color_mode::ColorMode;
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::gpu::Gpu;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::{AttributeDataType, DepthMaskType, DrawMode};
use mln_style::range::Range;
use mln_style::subdivision_granularity::SubdivisionGranularitySetting;

use crate::camera::{VerticalPerspectiveProjection, View};
use crate::draw::context::Context;
use crate::draw::draws::{Depth, Draw, Group};
use crate::draw::indexes::DrawIndexes;
use crate::draw::objects::{DrawGeometry, Mesh, Textures, raw_vertices};
use crate::draw::segment::SegmentBase;
use crate::draw::vertex_attribute::Uniforms;
use crate::layer::layer_drawing::TileMap;
use crate::render::globe::globe_tile_mesh::{RawGlobeTileMesh, raw_globe_tile_mesh};
use crate::render::paint_parameters::PaintParameters;
use crate::render::tile_matrix::to_projection_ubo;
use crate::render::update_parameters::UpdateParameters;
use crate::shaders::programs;
use crate::tile_store::tile_cover::{TileCoverParameters, tile_cover};

/// Writes the planet into the depth buffer ahead of the translucent pass, so
/// that 3D geometry behind the horizon is hidden by the globe the way the 2D
/// layers are by their clip Z.
#[derive(Default)]
pub(crate) struct GlobeDepthPass {
    /// The planet's draws, one of each tile of the cover.
    tiles: TileMap<Mesh>,
    /// The frame's draws of them, which draw into depth in the translucent
    /// pass.
    frame: Group,
    meshes: BTreeMap<(u8, bool, bool), RawGlobeTileMesh>,
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
                let vertices = raw_vertices("pos", mesh.vertices.clone(), mesh.vertex_count, AttributeDataType::Short2);
                let geometry = DrawGeometry {
                    vertices,
                    vertex_count: mesh.vertex_count,
                    mode: DrawMode::Triangles(),
                    indexes: DrawIndexes::new(mesh.indices.clone()),
                    segments: mesh.segments.iter().map(SegmentBase::copy).collect(),
                    instances: None,
                };
                // The far hemisphere is clockwise on screen on every backend,
                // Vulkan included, since its shaders flip Y.
                let mesh = Mesh::new(
                    context,
                    shader.clone(),
                    ColorMode::disabled(),
                    CullFaceMode::back_ccw(),
                    geometry,
                    Textures::default(),
                );
                *entry.value = Some(mesh);
            }
            let Some(draw) = entry.value.as_ref() else {
                continue;
            };
            let depth = Depth::Volume { mask: DepthMaskType::ReadWrite };
            let mut projection = state.projection_data(&tile_id.to_unwrapped());
            let setback = ground_setback(tile_id.canonical.z);
            // Away from the camera, in proportion to the distance, so that the
            // planet's outline stays: every point of it at least `setback`
            // back, as the nearest is.
            let camera = projection.camera_position;
            let nearest = (camera.length() - 1.0).max(f64::EPSILON);
            let away = 1.0 + setback / VerticalPerspectiveProjection::GLOBE_RADIUS_METERS / nearest;
            projection.main_matrix *= DMat4::from_translation(camera)
                * DMat4::from_scale(DVec3::splat(away))
                * DMat4::from_translation(-camera);
            projection.fallback_matrix *= DMat4::from_translation(DVec3::new(0.0, 0.0, -setback));
            for matrix in [&mut projection.main_matrix, &mut projection.fallback_matrix] {
                // z + setback * w: the pixels the planet covers stay.
                matrix.set_row(2, matrix.row(2) + DEPTH_SETBACK * matrix.row(3));
            }
            let projection = to_projection_ubo(&projection);
            // The draws draw in the order their meshes were made.
            let frame_draw = Draw { order: draw.seq(), depth, ..Draw::DEFAULT };
            self.frame.push_at(draw, *tile_id, &projection, frame_draw, ());
        }
        self.frame.finish(gpu, context, None, []);
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

/// How much farther the planet's depth draws, in the clip space depth that
/// runs from 0 to 1: 16 steps of a 24-bit depth buffer, which keep its
/// rounding from mixing the planet's depth with that of geometry on the
/// ground, which [`ground_setback`] leaves within rounding of it where it
/// is small, as at high zooms, and where the globe hands over to the flat
/// map, where both lie on one plane.
const DEPTH_SETBACK: f64 = 1.0 / (1 << 20) as f64;

/// How far, in meters, the planet's depth draws behind its surface along the
/// lines of sight, or below it on the flat map, for a tile at `zoom`: as far
/// as the chords of a tile's geometry on the ground
/// (`SubdivisionGranularitySetting::globe().fill`) sag below the sphere, for
/// tiles up to two zoom levels coarser, such as a source's tiles overscaled
/// or a parent's that shows while a tile loads.
fn ground_setback(zoom: u8) -> f64 {
    let zoom = zoom.saturating_sub(2);
    let granularity = SubdivisionGranularitySetting::globe().fill.granularity_for_zoom_level(zoom);
    // The angle across a cell of the grid that splits the geometry; any
    // triangle inside it is within half its diagonal of a vertex.
    let cell = std::f64::consts::TAU / (2f64.powi(i32::from(zoom)) * f64::from(granularity));
    VerticalPerspectiveProjection::GLOBE_RADIUS_METERS * (1.0 - (cell / std::f64::consts::SQRT_2).cos())
}
