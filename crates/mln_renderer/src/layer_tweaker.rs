// Ports include/mln/renderer/layer_tweaker.hpp and src/mln/renderer/layer_tweaker.cpp
// (+ RenderTile::translateVtxMatrix from src/mln/renderer/render_tile.cpp,
// which getTileMatrix calls).
//
// C++ `LayerTweaker` is an abstract class with data members: the data is
// `LayerTweakerBase`, the virtual `execute` is the `LayerTweaker` trait.
// Tweakers are shared (`std::shared_ptr`) by the render layer and, weakly, by
// its layer group: `Rc<RefCell<dyn LayerTweaker>>`.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use geometry::Point;
use mln_gfx::context::Context;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::shader_defines::ID_PROJECTION_UBO;
use mln_gfx::uniform_buffer::{UniformBufferArray, UniformBufferPtr};
use mln_gfx::{Drawable, LayerGroupBase};
use mln_map_types::{ProjectionData, TransformState};
use mln_style::layer_properties::LayerProperties;
use mln_style_types::TranslateAnchorType;
use mln_tile_id::UnwrappedTileID;
use mln_util::constants::{EXTENT, M2PI};
use mln_util::immutable::Immutable;
use mln_util::mat4::{Mat4, matrix};

use crate::layers::cast_matrix;
use crate::paint_parameters::{PaintParameters, TransformParameters};
use crate::render_tile::RenderTile;

pub type LayerTweakerPtr = Rc<RefCell<dyn LayerTweaker>>;

/// The data members of `LayerTweaker`.
pub struct LayerTweakerBase {
    pub id: String,
    pub evaluated_properties: Immutable<dyn LayerProperties>,
    pub properties_updated: bool,
    pub projection_uniform_buffer: Option<UniformBufferPtr>,
}

impl LayerTweakerBase {
    pub fn new(id: String, properties: Immutable<dyn LayerProperties>) -> Self {
        LayerTweakerBase {
            id,
            evaluated_properties: properties,
            properties_updated: true,
            projection_uniform_buffer: None,
        }
    }

    /// `uploadProjectionUBOs(layerUniforms, ubos, context)`: upload the
    /// per-drawable projection blocks of this layer, indexed like the
    /// drawable UBO array.
    pub fn upload_projection_ubos(
        &mut self,
        layer_uniforms: &mut UniformBufferArray,
        ubos: &[ProjectionUBO],
        context: &dyn Context,
    ) {
        let bytes: &[u8] = bytemuck::cast_slice(ubos);
        match &self.projection_uniform_buffer {
            Some(buffer) if buffer.get_size() >= bytes.len() => buffer.update(bytes),
            _ => self.projection_uniform_buffer = Some(context.create_uniform_buffer(bytes, false, true)),
        }
        layer_uniforms.set(ID_PROJECTION_UBO, self.projection_uniform_buffer.clone());
    }
}

/// Base class for layer tweakers, which manipulate layer group per frame.
pub trait LayerTweaker {
    fn base(&self) -> &LayerTweakerBase;
    fn base_mut(&mut self) -> &mut LayerTweakerBase;

    fn execute(&mut self, layer_group: &mut dyn LayerGroupBase, parameters: &PaintParameters<'_>);

    fn get_id(&self) -> &str {
        &self.base().id
    }

    fn update_properties(&mut self, new_props: Immutable<dyn LayerProperties>) {
        let base = self.base_mut();
        base.evaluated_properties = new_props;
        base.properties_updated = true;
    }

    /// `checkTweakDrawable`: apply to a drawable if it references us, or if
    /// it doesn't reference anything. The drawable's tweaker slot is opaque
    /// in `mln_gfx` (`Rc<dyn Any>`); renderer layers store a `LayerTweakerPtr`
    /// in it (see `layer_tweaker_slot`).
    fn check_tweak_drawable(&self, drawable: &dyn Drawable) -> bool {
        match drawable.get_layer_tweaker().and_then(|slot| slot.downcast_ref::<LayerTweakerPtr>()) {
            None => true,
            Some(tweaker) => std::ptr::addr_eq(tweaker.as_ptr(), std::ptr::from_ref::<Self>(self)),
        }
    }
}

/// The value for `Drawable::set_layer_tweaker` (C++
/// `drawable->setLayerTweaker(layerTweaker)`).
pub fn layer_tweaker_slot(tweaker: &LayerTweakerPtr) -> Option<Rc<dyn Any>> {
    Some(Rc::new(tweaker.clone()) as Rc<dyn Any>)
}

/// `LayerTweaker::getTileMatrix(tileID, parameters, translation, anchor,
/// nearClipped, inViewportPixelUnits, drawable, aligned = false)`.
#[allow(clippy::too_many_arguments)]
pub fn get_tile_matrix(
    tile_id: &UnwrappedTileID,
    parameters: &PaintParameters<'_>,
    translation: &[f32; 2],
    anchor: TranslateAnchorType,
    near_clipped: bool,
    in_viewport_pixel_units: bool,
    drawable: &dyn Drawable,
    aligned: bool,
) -> Mat4 {
    get_tile_matrix_for(
        tile_id,
        parameters.state,
        parameters.transform_params,
        translation,
        anchor,
        drawable.get_origin(),
        near_clipped,
        in_viewport_pixel_units,
        aligned,
    )
}

/// `LayerTweaker::getProjectionData(tileID, parameters, translation, anchor,
/// nearClipped, inViewportPixelUnits, drawable, aligned = false)`: the
/// projection contract for this tile, `getTileMatrix` plus the per-tile
/// projection fields.
#[allow(clippy::too_many_arguments)]
pub fn get_projection_data(
    tile_id: &UnwrappedTileID,
    parameters: &PaintParameters<'_>,
    translation: &[f32; 2],
    anchor: TranslateAnchorType,
    near_clipped: bool,
    in_viewport_pixel_units: bool,
    drawable: &dyn Drawable,
    aligned: bool,
) -> ProjectionData {
    let mut data = parameters.state.get_projection_data_for_matrix(
        tile_id,
        &get_tile_matrix(
            tile_id,
            parameters,
            translation,
            anchor,
            near_clipped,
            in_viewport_pixel_units,
            drawable,
            aligned,
        ),
    );
    if !in_viewport_pixel_units {
        let translate = RenderTile::tile_unit_translation(tile_id, translation, anchor, parameters.state);
        data.translate = [f64::from(translate[0]), f64::from(translate[1])];
    }
    // MLN_GLOBE_DEPTH_OFFSET_IN_SHADER is off, as on OpenGL: a drawable's
    // depth range keeps the layers apart, so the shader's depth shift stays 0.
    data
}

/// `LayerTweaker::toProjectionUBO`.
#[must_use]
pub fn to_projection_ubo(data: &ProjectionData) -> ProjectionUBO {
    ProjectionUBO {
        matrix: cast_matrix(&data.main_matrix),
        fallback_matrix: cast_matrix(&data.fallback_matrix),
        tile_mercator_coords: data.tile_mercator_coords.map(|v| v as f32),
        clipping_plane: data.clipping_plane.map(|v| v as f32),
        projection_transition: data.projection_transition as f32,
        depth_offset: data.depth_offset as f32,
        translate: data.translate.map(|v| v as f32),
    }
}

/// `LayerTweaker::globeExtrudeScale`: radians per pixel on the sphere for a
/// tile (GL JS `globeExtrudeScale`). `latitude_scale` is the cosine of the
/// center latitude on the globe and 1 on Mercator.
#[must_use]
pub fn globe_extrude_scale(tile_id: &UnwrappedTileID, zoom: f32, latitude_scale: f64) -> f32 {
    (f64::from(tile_id.pixels_to_tile_units(1.0, zoom)) / (f64::from(EXTENT) * (1u64 << tile_id.canonical.z) as f64)
        * M2PI
        * latitude_scale) as f32
}

/// The static `getTileMatrix(tileID, transformState, transformParams, ...)`
/// overload.
#[allow(clippy::too_many_arguments)]
pub fn get_tile_matrix_for(
    tile_id: &UnwrappedTileID,
    transform_state: &TransformState,
    transform_params: &TransformParameters,
    translation: &[f32; 2],
    anchor: TranslateAnchorType,
    origin: Option<Point<f64>>,
    near_clipped: bool,
    in_viewport_pixel_units: bool,
    aligned: bool,
) -> Mat4 {
    // from RenderTile::prepare
    let mut tile_matrix: Mat4 = [0.0; 16];
    transform_state.matrix_for(&mut tile_matrix, tile_id);
    if let Some(origin) = origin {
        let a = tile_matrix;
        matrix::translate(&mut tile_matrix, &a, origin.x, origin.y, 0.0);
    }
    multiply_with_projection_matrix(&mut tile_matrix, transform_params, near_clipped, aligned);
    translate_vtx_matrix(tile_id, &tile_matrix, translation, anchor, transform_state, in_viewport_pixel_units)
}

/// `multiplyWithProjectionMatrix(mat4&, const TransformParameters&, nearClipped, aligned)`.
pub fn multiply_with_projection_matrix(
    matrix: &mut Mat4,
    transform_params: &TransformParameters,
    near_clipped: bool,
    aligned: bool,
) {
    // nearClippedMatrix has near plane moved further, to enhance depth buffer
    // precision
    let proj_matrix_ref = if aligned {
        &transform_params.aligned_proj_matrix
    } else if near_clipped {
        &transform_params.near_clipped_proj_matrix
    } else {
        &transform_params.proj_matrix
    };
    let b = *matrix;
    matrix::multiply(matrix, proj_matrix_ref, &b);
}

/// `RenderTile::translateVtxMatrix(id, tileMatrix, translation, anchor,
/// state, inViewportPixelUnits)` (`src/mln/renderer/render_tile.cpp`).
pub fn translate_vtx_matrix(
    id: &UnwrappedTileID,
    tile_matrix: &Mat4,
    translation: &[f32; 2],
    anchor: TranslateAnchorType,
    state: &TransformState,
    in_viewport_pixel_units: bool,
) -> Mat4 {
    if translation[0] == 0.0 && translation[1] == 0.0 {
        return *tile_matrix;
    }

    let mut vtx_matrix: Mat4 = [0.0; 16];

    let angle: f32 = if in_viewport_pixel_units {
        if anchor == TranslateAnchorType::Map { state.get_bearing() as f32 } else { 0.0 }
    } else if anchor == TranslateAnchorType::Viewport {
        (-state.get_bearing()) as f32
    } else {
        0.0
    };

    // util::rotate(Point<float>, float): computed in float.
    let cos = angle.cos();
    let sin = angle.sin();
    let translate =
        Point::new(cos * translation[0] - sin * translation[1], sin * translation[0] + cos * translation[1]);

    if in_viewport_pixel_units {
        matrix::translate(&mut vtx_matrix, tile_matrix, f64::from(translate.x), f64::from(translate.y), 0.0);
    } else {
        let zoom = state.get_zoom() as f32;
        matrix::translate(
            &mut vtx_matrix,
            tile_matrix,
            f64::from(id.pixels_to_tile_units(translate.x, zoom)),
            f64::from(id.pixels_to_tile_units(translate.y, zoom)),
            0.0,
        );
    }

    vtx_matrix
}
