//! The color ramps of the layer types that color by one, and their
//! textures.

use std::rc::Rc;
use std::sync::Arc;

use mln_render::draw::{Resources, Texture};
use mln_style::color_ramp_property_value::ColorRampPropertyValue;
use mln_style::expression::{Dependency, GlobalStateMap, global_state_refs_intersect};
use mln_style::{PropertyEvaluationParameters, StyleValue as _};
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

/// A layer's color ramp: a row of the colors of a ramp property's value
/// from 0 to 1, and the texture of it that the layer's meshes sample.
pub(crate) struct ColorRamp {
    image: Rc<PremultipliedImage>,
    /// The global state the image was made with.
    state: Option<Arc<GlobalStateMap>>,
    texture: Texture,
    /// The image the texture holds.
    written: Rc<PremultipliedImage>,
}

impl ColorRamp {
    /// A ramp of `width` transparent colors, with its texture in `res`.
    pub(crate) fn new(res: &mut Resources, width: u32) -> ColorRamp {
        let image = Rc::new(PremultipliedImage::new(Size::new(width, 1)));
        let texture = res.texture(|texture| texture.set_image(Some(Rc::clone(&image))));
        ColorRamp { written: Rc::clone(&image), image, state: None, texture }
    }

    /// Makes the ramp of `value` again where it may have changed in a frame
    /// of `parameters`: for a new or restyled layer, or a change of the
    /// global state that it reads. An undefined value leaves the ramp.
    pub(crate) fn evaluate(&mut self, value: &ColorRampPropertyValue, parameters: &PropertyEvaluationParameters) {
        let state_differs = match (&self.state, &parameters.global_state) {
            (None, None) => false,
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            _ => true,
        };
        let affected = parameters.layer_changed
            || (state_differs
                && value.get_dependencies().intersects(Dependency::GLOBAL_STATE)
                && global_state_refs_intersect(
                    value.get_global_state_refs(),
                    parameters.changed_global_state_keys.as_deref(),
                ));
        self.state.clone_from(&parameters.global_state);
        if affected && !value.is_undefined() {
            let mut image = (*self.image).clone();
            fill(value, &mut image, self.state.as_deref());
            self.image = Rc::new(image);
        }
    }

    /// Writes the ramp into its texture through `res` where it changed.
    pub(crate) fn update(&mut self, res: &mut Resources) {
        if !Rc::ptr_eq(&self.written, &self.image) {
            if let Some(mut texture) = res.texture_mut(&self.texture) {
                texture.set_image(Some(Rc::clone(&self.image)));
            }
            self.written = Rc::clone(&self.image);
        }
    }

    /// The ramp's texture, which holds the ramp from its layer's last
    /// [`update`](Self::update) on.
    pub(crate) fn texture(&self) -> &Texture {
        &self.texture
    }
}

/// Fills `image`, a row of pixels, with `value`'s ramp from 0 to 1, with
/// `global_state` for its `global-state` expressions.
fn fill(value: &ColorRampPropertyValue, image: &mut PremultipliedImage, global_state: Option<&GlobalStateMap>) {
    let length = image.bytes();
    for (i, pixel) in image.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let color = value.evaluate_ramp((i * 4) as f64 / length as f64, global_state);
        *pixel = [color.r, color.g, color.b, color.a].map(|channel| (channel * 255.0).floor() as u8);
    }
}
