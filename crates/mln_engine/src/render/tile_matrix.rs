//! The matrices and projections of a layer's draws of a tile, which their
//! uniforms hold.

use geometry::Point;
use glam::{DMat4, DVec2, DVec3, Vec2};
use mln_gfx::gpu::Gpu;
use mln_gfx::shaders::bindings::PROJECTION_ID;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_style::constants::EXTENT;
use mln_style::types::TranslateAnchorType;

use crate::camera::{ProjectionData, View};
use crate::draw::context::Context;
use crate::draw::objects::DrawBuffers;
use crate::geo::constants::M2PI;
use crate::layers::cast_matrix;
use crate::render::paint_parameters::{PaintParameters, TransformParameters};
use crate::render::tile::RenderTile;
use crate::tile::UnwrappedTileID;

/// Gives `layer_uniforms`, of a layer whose draws draw without a
/// projection, a projection block of zeros if they have none. GL's programs
/// declare the block whether they read it or not, so a draw binds it all the
/// same.
pub(crate) fn bind_no_projection(gpu: &mut dyn Gpu, layer_uniforms: &mut DrawBuffers, context: &mut Context) {
    if layer_uniforms.get(PROJECTION_ID).is_none() {
        layer_uniforms.write_storage(
            gpu,
            PROJECTION_ID,
            bytemuck::bytes_of(&ProjectionUBO::default()),
            context.device(),
        );
    }
}

/// The matrix that draws geometry of the tile `tile_id` in the frame of
/// `parameters`: translated by `translation`, anchored as `anchor` says, in
/// tile units or viewport pixels, from `origin` in the tile where it has
/// one, and projected with the near-clipped or pixel-aligned projection
/// matrix where those say so.
#[expect(
    clippy::too_many_arguments,
    reason = "the tile, the paint parameters and the translation, origin, clipping and alignment options of its matrix"
)]
pub(crate) fn tile_matrix(
    tile_id: &UnwrappedTileID,
    parameters: &PaintParameters<'_>,
    translation: [f32; 2],
    anchor: TranslateAnchorType,
    near_clipped: bool,
    in_viewport_pixel_units: bool,
    origin: Option<Point<f64>>,
    aligned: bool,
) -> DMat4 {
    tile_matrix_for(
        tile_id,
        parameters.view,
        parameters.transform_params,
        translation,
        anchor,
        origin,
        near_clipped,
        in_viewport_pixel_units,
        aligned,
    )
}

/// The projection data for this tile, of geometry at `origin` in it: the
/// tile matrix plus the per-tile projection fields.
#[expect(
    clippy::too_many_arguments,
    reason = "the tile, the paint parameters and the translation, origin, clipping and alignment options of its projection"
)]
pub(crate) fn projection_data(
    tile_id: &UnwrappedTileID,
    parameters: &PaintParameters<'_>,
    translation: [f32; 2],
    anchor: TranslateAnchorType,
    near_clipped: bool,
    in_viewport_pixel_units: bool,
    origin: Option<Point<f64>>,
    aligned: bool,
) -> ProjectionData {
    let mut data = parameters.view.projection_data_for_matrix(
        tile_id,
        &tile_matrix(tile_id, parameters, translation, anchor, near_clipped, in_viewport_pixel_units, origin, aligned),
    );
    if !in_viewport_pixel_units {
        let translate = RenderTile::tile_unit_translation(tile_id, translation, anchor, parameters.view);
        data.translate = DVec2::new(f64::from(translate[0]), f64::from(translate[1]));
    }
    // A draw's depth range keeps the layers apart, so the shader's depth
    // offset stays 0.
    data
}

#[must_use]
pub(crate) fn to_projection_ubo(data: &ProjectionData) -> ProjectionUBO {
    // The anchor's offset goes into the matrix in f64, where the camera's
    // large translation cancels it exactly; the anchor is zero on Mercator.
    let anchor = data.sphere_anchor;
    let plane = data.clipping_plane;
    ProjectionUBO {
        matrix: cast_matrix(&(data.main_matrix * DMat4::from_translation(anchor))),
        fallback_matrix: cast_matrix(&data.fallback_matrix),
        tile_mercator_coords: data.tile_mercator_coords.as_vec4().to_array(),
        clipping_plane: data.clipping_plane.as_vec4().to_array(),
        projection_transition: data.projection_transition as f32,
        depth_offset: data.depth_offset as f32,
        translate: data.translate.as_vec2().to_array(),
        anchor: anchor.extend(anchor.dot(plane.truncate()) + plane.w).as_vec4().to_array(),
        anchor_angles: data.sphere_anchor_angles.as_vec4().to_array(),
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
