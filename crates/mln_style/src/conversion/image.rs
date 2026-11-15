use super::{Converter, Convertible, Error};
use crate::expression::Image;

impl Converter for Image {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Image> {
        mln_expression::image::convert_image(value, error)
    }
}
