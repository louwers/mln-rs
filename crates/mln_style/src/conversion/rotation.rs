use super::{Converter, Convertible, Error};
use crate::types::Rotation;

impl Converter for Rotation {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Rotation> {
        let Some(converted) = value.to_double() else {
            error.message = "value must be a number".to_owned();
            return None;
        };
        Some(Rotation::new(converted))
    }
}
