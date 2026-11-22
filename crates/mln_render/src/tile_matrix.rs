//! The matrices and projections of a layer's draws of a tile, which their
//! uniforms hold.

use std::fmt;

use geometry::Point;
use glam::dcamera::rh::proj::opengl::orthographic;
use glam::{DMat4, DVec2, DVec3, Vec2};
use mln_gfx::gpu::Gpu;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::program::PROJECTION_BUFFER;
use mln_map_types::{Projection, ProjectionData, View};
use mln_style_types::TranslateAnchorType;
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::constants::{EXTENT, M2PI};

use crate::gfx::context::Resources;
use crate::gfx::uniform_buffer::UniformBufferArray;
use crate::paint_parameters::{Frame, TransformParameters};

/// Gives `layer_uniforms`, of a layer whose draws draw without a
/// projection, a projection block of zeros if they have none. GL's programs
/// declare the block whether they read it or not, so a draw binds it all the
/// same.
pub(crate) fn bind_no_projection(gpu: &mut dyn Gpu, layer_uniforms: &mut UniformBufferArray, context: &mut Resources) {
    if layer_uniforms.get(PROJECTION_BUFFER.id).is_none() {
        layer_uniforms.write_storage(
            gpu,
            PROJECTION_BUFFER.id,
            bytemuck::bytes_of(&ProjectionUBO::default()),
            context.device(),
        );
    }
}

/// How a draw places a tile's geometry. The default places it as the tile
/// has it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TileTransform {
    /// A layer's `*-translate`, in pixels.
    pub translate: [f32; 2],
    /// What the translation's axes follow: the map or the viewport.
    pub anchor: TranslateAnchorType,
    /// Where the geometry's origin is in the tile, in tile units, if not at
    /// the tile's corner.
    pub origin: Option<DVec2>,
    /// Whether to project with the near plane moved out to a tenth of the
    /// camera's distance to the center, for draws that test the depth of 3D
    /// geometry, which then spends less of its precision near the camera.
    pub near_clipped: bool,
    /// Whether the translation is in viewport pixels rather than tile
    /// units, for geometry in pixels.
    pub viewport_pixels: bool,
    /// Whether to project onto the pixel grid, for geometry that draws
    /// crisp while the camera stands still.
    pub aligned: bool,
    /// Whether the geometry is the viewport's rather than the map's: the
    /// tile's extent spans the viewport, x right and y up from its bottom
    /// left corner, moved by `translate` in tile units, on any projection.
    /// Its meshes draw in Mercator's projection
    /// ([`MeshDesc::screen_space`](crate::draw::MeshDesc::screen_space)).
    pub screen_space: bool,
}

impl TileTransform {
    /// The translation, `translate` anchored as `anchor` says, in the units
    /// of `tile` seen by `camera`.
    #[must_use]
    pub fn tile_units(&self, tile: &UnwrappedTileID, camera: &View) -> [f32; 2] {
        tile_unit_translation(tile, self.translate, self.anchor, camera)
    }
}

/// Where a tile's geometry draws in a frame, as a [`TileTransform`] places
/// it: the matrices a draw's values may hold, and the projection block that
/// the engine writes for a draw pushed with it
/// ([`Group::push_at`](crate::draw::Group::push_at)).
#[derive(Clone, Copy)]
pub struct TileProjection {
    /// The matrix from the tile's units to clip space, of the frame's
    /// projection.
    pub matrix: [f32; 16],
    /// The matrix from the tile's units to clip space on Mercator.
    pub fallback_matrix: [f32; 16],
    /// The translation in the tile's units, which the globe adds on the
    /// sphere; zero for a translation in viewport pixels.
    pub translation: [f32; 2],
    /// The tile units that a logical pixel spans at the camera's zoom.
    pub pixels_to_tile_units: f32,
    /// The radians on the planet that a logical pixel spans at the camera's
    /// zoom, at the center of the view.
    pub globe_extrude_scale: f32,
    /// The projection block of the draws pushed with it.
    pub(crate) block: ProjectionUBO,
    /// The tile whose mask a draw pushed with it clips to: the layer's tile
    /// it was made for.
    pub(crate) tile: Option<OverscaledTileID>,
}

impl TileProjection {
    /// The projection of `data`, the projection of the tile `tile` in a
    /// frame of `view`, to which draws clip as to `clip_to` where they clip.
    pub(crate) fn of(
        tile: &UnwrappedTileID,
        view: &View,
        data: &ProjectionData,
        clip_to: Option<OverscaledTileID>,
    ) -> TileProjection {
        let zoom = view.zoom() as f32;
        TileProjection {
            matrix: cast_matrix(&data.main_matrix),
            fallback_matrix: cast_matrix(&data.fallback_matrix),
            translation: data.translate.as_vec2().to_array(),
            pixels_to_tile_units: tile.pixels_to_tile_units(1.0, zoom),
            globe_extrude_scale: globe_extrude_scale(tile, zoom, view.projection().circle_radius_correction(view)),
            block: to_projection_ubo(data),
            tile: clip_to,
        }
    }
}

impl fmt::Debug for TileProjection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TileProjection").field("matrix", &self.matrix).field("tile", &self.tile).finish_non_exhaustive()
    }
}

/// The matrix that draws geometry of the tile `tile_id` in `frame`:
/// translated by `translation`, anchored as `anchor` says, in tile units or
/// viewport pixels, from `origin` in the tile where it has one, and
/// projected with the near-clipped or pixel-aligned projection matrix where
/// those say so.
#[expect(
    clippy::too_many_arguments,
    reason = "the tile, the frame and the translation, origin, clipping and alignment options of its matrix"
)]
pub(crate) fn tile_matrix(
    tile_id: &UnwrappedTileID,
    frame: &Frame<'_>,
    translation: [f32; 2],
    anchor: TranslateAnchorType,
    near_clipped: bool,
    in_viewport_pixel_units: bool,
    origin: Option<Point<f64>>,
    aligned: bool,
) -> DMat4 {
    tile_matrix_for(
        tile_id,
        frame.camera,
        frame.transform,
        translation,
        anchor,
        origin,
        near_clipped,
        in_viewport_pixel_units,
        aligned,
    )
}

/// The projection of geometry of the tile `tile_id` in `frame`, placed as
/// `how` says: the tile's matrix, translated by its `*-translate` in tile
/// units or viewport pixels, from its origin, and projected with the
/// near-clipped or pixel-aligned projection matrix where it says so, plus
/// the tile's other projection fields.
#[must_use]
pub(crate) fn projection_data(tile_id: &UnwrappedTileID, frame: &Frame<'_>, how: &TileTransform) -> ProjectionData {
    if how.screen_space {
        let extent = f64::from(EXTENT);
        let [x, y] = how.translate.map(f64::from);
        let matrix =
            orthographic(0.0, extent, -extent, 0.0, 0.0, 1.0) * DMat4::from_translation(DVec3::new(x, y - extent, 0.0));
        return Projection::Mercator.projection_data(frame.camera, tile_id, &matrix);
    }
    let origin = how.origin.map(|origin| Point::new(origin.x, origin.y));
    let (translation, anchor) = (how.translate, how.anchor);
    let matrix =
        tile_matrix(tile_id, frame, translation, anchor, how.near_clipped, how.viewport_pixels, origin, how.aligned);
    let mut data = frame.camera.projection_data_for_matrix(tile_id, &matrix);
    if !how.viewport_pixels {
        let translate = tile_unit_translation(tile_id, translation, anchor, frame.camera);
        data.translate = DVec2::new(f64::from(translate[0]), f64::from(translate[1]));
    }
    // A draw's depth range keeps the layers apart, so the shader's depth
    // offset stays 0.
    data
}

/// A layer's `*-translate` of `translation` pixels, anchored as `anchor`
/// says, in the units of the tile `id` in `view`.
#[must_use]
pub(crate) fn tile_unit_translation(
    id: &UnwrappedTileID,
    translation: [f32; 2],
    anchor: TranslateAnchorType,
    view: &View,
) -> [f32; 2] {
    if translation[0] == 0.0 && translation[1] == 0.0 {
        return [0.0, 0.0];
    }
    let angle = if anchor == TranslateAnchorType::Viewport { (-view.angle()) as f32 } else { 0.0f32 };
    let translate = Vec2::from_angle(angle).rotate(Vec2::new(translation[0], translation[1]));
    let zoom = view.zoom() as f32;
    [id.pixels_to_tile_units(translate.x, zoom), id.pixels_to_tile_units(translate.y, zoom)]
}

/// The column-major `f32` matrix of `m`, as a uniform block holds it.
#[must_use]
pub(crate) fn cast_matrix(m: &DMat4) -> [f32; 16] {
    m.as_mat4().to_cols_array()
}

/// The uniform block of `data`, which the programs' projection reads.
#[must_use]
pub(crate) fn to_projection_ubo(data: &ProjectionData) -> ProjectionUBO {
    ProjectionUBO {
        matrix: cast_matrix(&data.main_matrix),
        fallback_matrix: cast_matrix(&data.fallback_matrix),
        tile_mercator_coords: data.tile_mercator_coords.as_vec4().to_array(),
        clipping_plane: data.clipping_plane.as_vec4().to_array(),
        transition: data.projection_transition as f32,
        depth_offset: data.depth_offset as f32,
        translate: data.translate.as_vec2().to_array(),
    }
}

/// Radians per pixel on the sphere for a tile.
/// `latitude_scale` is the cosine of the center latitude on the globe and 1
/// on Mercator.
#[must_use]
pub(crate) fn globe_extrude_scale(tile_id: &UnwrappedTileID, zoom: f32, latitude_scale: f64) -> f32 {
    (f64::from(tile_id.pixels_to_tile_units(1.0, zoom)) / (f64::from(EXTENT) * (1u64 << tile_id.canonical.z) as f64)
        * M2PI
        * latitude_scale) as f32
}

/// [`tile_matrix`] in `view`, projected with the
/// matrices of `transform_params`.
#[expect(
    clippy::too_many_arguments,
    reason = "the tile, the transform and the translation, origin, clipping and alignment options of its matrix"
)]
pub(crate) fn tile_matrix_for(
    tile_id: &UnwrappedTileID,
    view: &View,
    transform_params: &TransformParameters,
    translation: [f32; 2],
    anchor: TranslateAnchorType,
    origin: Option<Point<f64>>,
    near_clipped: bool,
    in_viewport_pixel_units: bool,
    aligned: bool,
) -> DMat4 {
    let mut tile_matrix = view.tile_matrix(tile_id);
    if let Some(origin) = origin {
        tile_matrix *= DMat4::from_translation(DVec3::new(origin.x, origin.y, 0.0));
    }
    let tile_matrix = with_projection_matrix(&tile_matrix, transform_params, near_clipped, aligned);
    translate_vtx_matrix(tile_id, &tile_matrix, translation, anchor, view, in_viewport_pixel_units)
}

/// `matrix` projected with the frame's projection matrix: the pixel-aligned
/// one if `aligned`, else the near-clipped one if `near_clipped`.
pub(crate) fn with_projection_matrix(
    matrix: &DMat4,
    transform_params: &TransformParameters,
    near_clipped: bool,
    aligned: bool,
) -> DMat4 {
    // The near-clipped matrix moves the near plane further out, for the
    // depth buffer's precision.
    let proj_matrix_ref = if aligned {
        &transform_params.aligned_proj_matrix
    } else if near_clipped {
        &transform_params.near_clipped_proj_matrix
    } else {
        &transform_params.proj_matrix
    };
    *proj_matrix_ref * *matrix
}

pub(crate) fn translate_vtx_matrix(
    id: &UnwrappedTileID,
    tile_matrix: &DMat4,
    translation: [f32; 2],
    anchor: TranslateAnchorType,
    state: &View,
    in_viewport_pixel_units: bool,
) -> DMat4 {
    if translation[0] == 0.0 && translation[1] == 0.0 {
        return *tile_matrix;
    }

    let angle: f32 = if in_viewport_pixel_units {
        if anchor == TranslateAnchorType::Map { state.angle() as f32 } else { 0.0 }
    } else if anchor == TranslateAnchorType::Viewport {
        (-state.angle()) as f32
    } else {
        0.0
    };

    let translate = Vec2::from_angle(angle).rotate(Vec2::new(translation[0], translation[1]));

    let offset = if in_viewport_pixel_units {
        DVec3::new(f64::from(translate.x), f64::from(translate.y), 0.0)
    } else {
        let zoom = state.zoom() as f32;
        DVec3::new(
            f64::from(id.pixels_to_tile_units(translate.x, zoom)),
            f64::from(id.pixels_to_tile_units(translate.y, zoom)),
            0.0,
        )
    };
    *tile_matrix * DMat4::from_translation(offset)
}
