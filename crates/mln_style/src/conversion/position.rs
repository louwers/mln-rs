use super::{Converter, Convertible, Error, convert};
use crate::types::Position;

impl Converter for Position {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Position> {
        let spherical = convert::<[f32; 3]>(value, error)?;
        Some(Position::new(spherical))
    }
}
