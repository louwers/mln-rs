//! The matrices and projections of a layer's draws of a tile, which their
//! uniforms hold.

use geometry::Point;
use glam::{DMat4, DVec2, DVec3, Vec2};
use mln_gfx::gpu::Gpu;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::shader_defines::ID_PROJECTION_UBO;
use mln_map_types::{ProjectionData, View};
use mln_style_types::TranslateAnchorType;
use mln_tile_id::UnwrappedTileID;
use mln_util::constants::{EXTENT, M2PI};

use crate::gfx::context::Context;
use crate::gfx::uniform_buffer::UniformBufferArray;
use crate::paint_parameters::{DrawFrame, TransformParameters};

/// Gives `layer_uniforms`, of a layer whose draws draw without a
/// projection, a projection block of zeros if they have none. GL's programs
/// declare the block whether they read it or not, so a draw binds it all the
/// same.
pub(crate) fn bind_no_projection(gpu: &mut dyn Gpu, layer_uniforms: &mut UniformBufferArray, context: &mut Context) {
    if layer_uniforms.get(ID_PROJECTION_UBO).is_none() {
        layer_uniforms.write_storage(
            gpu,
            ID_PROJECTION_UBO,
            bytemuck::bytes_of(&ProjectionUBO::default()),
            context.device(),
        );
    }
}

/// The matrix that draws geometry of the tile `tile_id` in `frame`: translated by `translation`, anchored as `anchor` says, in
/// tile units or viewport pixels, from `origin` in the tile where it has
/// one, and projected with the near-clipped or pixel-aligned projection
/// matrix where those say so.
#[expect(
    clippy::too_many_arguments,
    reason = "the tile, the frame and the translation, origin, clipping and alignment options of its matrix"
)]
pub(crate) fn tile_matrix(
    tile_id: &UnwrappedTileID,
    frame: &DrawFrame<'_>,
    translation: [f32; 2],
    anchor: TranslateAnchorType,
    near_clipped: bool,
    in_viewport_pixel_units: bool,
    origin: Option<Point<f64>>,
    aligned: bool,
) -> DMat4 {
    tile_matrix_for(
        tile_id,
        frame.view,
        frame.transform,
        translation,
        anchor,
        origin,
        near_clipped,
        in_viewport_pixel_units,
        aligned,
    )
}

/// The projection of geometry of the tile `tile_id` in `frame`, at `origin`
/// in the tile where it has one: the tile's matrix, translated by
/// `translation` anchored as `anchor` says, in tile units or viewport pixels,
/// and projected with the near-clipped or pixel-aligned projection matrix
/// where those say so, plus the tile's other projection fields.
#[expect(
    clippy::too_many_arguments,
    reason = "the tile, the frame and the translation, origin, clipping and alignment options of its projection"
)]
#[must_use]
pub fn projection_data(
    tile_id: &UnwrappedTileID,
    frame: &DrawFrame<'_>,
    translation: [f32; 2],
    anchor: TranslateAnchorType,
    near_clipped: bool,
    in_viewport_pixel_units: bool,
    origin: Option<Point<f64>>,
    aligned: bool,
) -> ProjectionData {
    let mut data = frame.view.projection_data_for_matrix(
        tile_id,
        &tile_matrix(tile_id, frame, translation, anchor, near_clipped, in_viewport_pixel_units, origin, aligned),
    );
    if !in_viewport_pixel_units {
        let translate = tile_unit_translation(tile_id, translation, anchor, frame.view);
        data.translate = DVec2::new(f64::from(translate[0]), f64::from(translate[1]));
    }
    // A draw's depth range keeps the layers apart, so the shader's depth
    // offset stays 0.
    data
}

/// A layer's `*-translate` of `translation` pixels, anchored as `anchor`
/// says, in the units of the tile `id` in `view`.
#[must_use]
pub fn tile_unit_translation(
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
pub fn cast_matrix(m: &DMat4) -> [f32; 16] {
    m.as_mat4().to_cols_array()
}

/// The uniform block of `data`, which the programs' projection reads.
#[must_use]
pub fn to_projection_ubo(data: &ProjectionData) -> ProjectionUBO {
    ProjectionUBO {
        matrix: cast_matrix(&data.main_matrix),
        fallback_matrix: cast_matrix(&data.fallback_matrix),
        tile_mercator_coords: data.tile_mercator_coords.as_vec4().to_array(),
        clipping_plane: data.clipping_plane.as_vec4().to_array(),
        projection_transition: data.projection_transition as f32,
        depth_offset: data.depth_offset as f32,
        translate: data.translate.as_vec2().to_array(),
    }
}

/// Radians per pixel on the sphere for a tile.
/// `latitude_scale` is the cosine of the center latitude on the globe and 1
/// on Mercator.
#[must_use]
pub fn globe_extrude_scale(tile_id: &UnwrappedTileID, zoom: f32, latitude_scale: f64) -> f32 {
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
