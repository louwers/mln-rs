use std::rc::Rc;

use crate::drawable::Drawable;
use crate::paint_parameters::PaintParameters;

/// Base class for drawable tweakers, which manipulate drawables per frame.
/// `&self`: tweakers are shared (`DrawableTweakerPtr`); implementations keep
/// mutable state in cells.
pub trait DrawableTweaker {
    /// Called as the drawable gets added to the layer
    fn init(&self, drawable: &mut dyn Drawable);
    /// Called just before rendering
    fn execute(&self, drawable: &mut dyn Drawable, parameters: &mut dyn PaintParameters);
}

pub type DrawableTweakerPtr = Rc<dyn DrawableTweaker>;
