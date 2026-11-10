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
use mln_gfx::{Drawable, LayerGroupBase};
use mln_map_types::TransformState;
use mln_style::layer_properties::LayerProperties;
use mln_style_types::TranslateAnchorType;
use mln_tile_id::UnwrappedTileID;
use mln_util::immutable::Immutable;
use mln_util::mat4::{Mat4, matrix};

use crate::paint_parameters::{PaintParameters, TransformParameters};

pub type LayerTweakerPtr = Rc<RefCell<dyn LayerTweaker>>;

/// The data members of `LayerTweaker`.
pub struct LayerTweakerBase {
    pub id: String,
    pub evaluated_properties: Immutable<dyn LayerProperties>,
    pub properties_updated: bool,
}

impl LayerTweakerBase {
    pub fn new(id: String, properties: Immutable<dyn LayerProperties>) -> Self {
        LayerTweakerBase { id, evaluated_properties: properties, properties_updated: true }
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
    /// in mln_gfx (`Rc<dyn Any>`); renderer layers store a `LayerTweakerPtr`
    /// in it (see `layer_tweaker_slot`).
    fn check_tweak_drawable(&self, drawable: &dyn Drawable) -> bool {
        match drawable.get_layer_tweaker().and_then(|slot| slot.downcast_ref::<LayerTweakerPtr>()) {
            None => true,
            Some(tweaker) => std::ptr::addr_eq(tweaker.as_ptr(), self as *const Self),
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
/// state, inViewportPixelUnits)` (src/mln/renderer/render_tile.cpp).
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
        matrix::translate(&mut vtx_matrix, tile_matrix, translate.x as f64, translate.y as f64, 0.0);
    } else {
        let zoom = state.get_zoom() as f32;
        matrix::translate(
            &mut vtx_matrix,
            tile_matrix,
            id.pixels_to_tile_units(translate.x, zoom) as f64,
            id.pixels_to_tile_units(translate.y, zoom) as f64,
            0.0,
        );
    }

    vtx_matrix
}
