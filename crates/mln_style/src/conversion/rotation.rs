use mln_style_types::Rotation;

use super::{Converter, Convertible, Error};

impl Converter for Rotation {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Rotation> {
        let Some(converted) = value.to_double() else {
            error.message = "value must be a number".to_owned();
            return None;
        };
        Some(Rotation::new(converted))
    }
}
