//! Where a layer's draws of a tile put its geometry, worked out on the CPU as
//! the shaders work it out: the tile's [`ProjectionData`] in a frame
//! ([`tile_projection`]), the block that programs read it from
//! ([`ProjectionData::ubo`]), the scale of extrusions on the globe
//! ([`globe_extrude_scale`]), and the clip depth of 3D layers on OpenGL
//! ([`zero_to_one_clip_z`]).

use glam::{DMat4, DVec2, DVec3, Vec2};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_style::constants::EXTENT;
use mln_style::types::TranslateAnchorType;

pub use crate::camera::ProjectionData;
use crate::camera::View;
use crate::draw::draws::LayerFrame;
use crate::geo::constants::M2PI;
use crate::render::paint_parameters::TransformParameters;
use crate::render::tile::RenderTile;
use crate::tile::UnwrappedTileID;

/// How a draw places a tile's geometry: moved by a layer's translate
/// property, and projected with one of the frame's projection matrices.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileTransform {
    /// The pixels the geometry moves by (a `*-translate` property), along
    /// the axes that `anchor` says (its `*-translate-anchor`).
    pub translate: [f32; 2],
    pub anchor: TranslateAnchorType,
    /// Projects with the matrix whose near plane is further out, for the
    /// depth precision of 3D layers.
    pub near_clipped: bool,
    /// Projects with the matrix that aligns the tile to whole pixels, for
    /// crisp images while the camera stands still. It wins over
    /// `near_clipped`.
    pub pixel_aligned: bool,
}

impl TileTransform {
    /// The tile as it is, projected with the frame's matrix.
    pub const NONE: TileTransform = TileTransform {
        translate: [0.0, 0.0],
        anchor: TranslateAnchorType::Viewport,
        near_clipped: false,
        pixel_aligned: false,
    };
}

impl Default for TileTransform {
    fn default() -> Self {
        TileTransform::NONE
    }
}

/// The projection of the tile `tile_id` in `frame`, with its geometry placed
/// as `transform` says.
#[must_use]
pub fn tile_projection(tile_id: &UnwrappedTileID, frame: &LayerFrame<'_>, transform: TileTransform) -> ProjectionData {
    let matrix = tile_matrix(tile_id, frame.view, frame.transform_params, &transform);
    let mut data = frame.view.projection_data_for_matrix(tile_id, &matrix);
    let translate = RenderTile::tile_unit_translation(tile_id, transform.translate, transform.anchor, frame.view);
    data.translate = DVec2::new(f64::from(translate[0]), f64::from(translate[1]));
    // A draw's depth range keeps the layers apart, so the shader's depth
    // offset stays 0.
    data
}

impl ProjectionData {
    /// The block that a program reads the projection from.
    #[must_use]
    pub fn ubo(&self) -> ProjectionUBO {
        // The anchor's offset goes into the matrix in f64, where the camera's
        // large translation cancels it exactly; the anchor is zero on
        // Mercator.
        let anchor = self.sphere_anchor;
        let plane = self.clipping_plane;
        ProjectionUBO {
            matrix: cast_matrix(&(self.main_matrix * DMat4::from_translation(anchor))),
            fallback_matrix: cast_matrix(&self.fallback_matrix),
            tile_mercator_coords: self.tile_mercator_coords.as_vec4().to_array(),
            clipping_plane: self.clipping_plane.as_vec4().to_array(),
            projection_transition: self.projection_transition as f32,
            depth_offset: self.depth_offset as f32,
            translate: self.translate.as_vec2().to_array(),
            anchor: anchor.extend(anchor.dot(plane.truncate()) + plane.w).as_vec4().to_array(),
            anchor_angles: self.sphere_anchor_angles.as_vec4().to_array(),
        }
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

/// Makes a 3D layer's OpenGL matrix clip and write depth as Metal, Vulkan,
/// D3D12 and WebGPU do with it: clip z from -w to w becomes `2z - w`, so that
/// OpenGL clips what was below 0 and writes the depth range times the old z
/// over w.
pub fn zero_to_one_clip_z(matrix: &mut DMat4) {
    matrix.set_row(2, 2.0 * matrix.row(2) - matrix.row(3));
}

/// The matrix that draws geometry of the tile `tile_id` in `view`, placed
/// as `transform` says, projected with the matrices of `transform_params`.
fn tile_matrix(
    tile_id: &UnwrappedTileID,
    view: &View,
    transform_params: &TransformParameters,
    transform: &TileTransform,
) -> DMat4 {
    // The near-clipped matrix moves the near plane further out, for the
    // depth buffer's precision.
    let projection = if transform.pixel_aligned {
        &transform_params.aligned_proj_matrix
    } else if transform.near_clipped {
        &transform_params.near_clipped_proj_matrix
    } else {
        &transform_params.proj_matrix
    };
    let matrix = *projection * view.tile_matrix(tile_id);
    let [x, y] = transform.translate;
    if x == 0.0 && y == 0.0 {
        return matrix;
    }
    let angle = if transform.anchor == TranslateAnchorType::Viewport { (-view.angle()) as f32 } else { 0.0 };
    let translate = Vec2::from_angle(angle).rotate(Vec2::new(x, y));
    let zoom = view.zoom() as f32;
    let offset = DVec3::new(
        f64::from(tile_id.pixels_to_tile_units(translate.x, zoom)),
        f64::from(tile_id.pixels_to_tile_units(translate.y, zoom)),
        0.0,
    );
    matrix * DMat4::from_translation(offset)
}

/// `m` as the column-major `f32`s that a block's matrix holds.
fn cast_matrix(m: &DMat4) -> [f32; 16] {
    m.as_mat4().to_cols_array()
}
