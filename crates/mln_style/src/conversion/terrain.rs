use super::{Convertible, Error};
use crate::terrain::Terrain;

/// Converts the style's `terrain`.
pub fn convert_terrain(value: &Convertible<'_>, error: &mut Error) -> Option<Terrain> {
    if !value.is_object() {
        error.message = "terrain must be an object".to_owned();
        return None;
    }

    let Some(source) = value.object_member("source") else {
        error.message = "terrain must have a source".to_owned();
        return None;
    };
    let Some(source) = source.to_string() else {
        error.message = "terrain source must be a string".to_owned();
        return None;
    };
    let mut terrain = Terrain::new(source);

    if let Some(exaggeration) = value.object_member("exaggeration") {
        let Some(exaggeration) = exaggeration.to_number() else {
            error.message = "terrain exaggeration must be a number".to_owned();
            return None;
        };
        if exaggeration < 0.0 {
            error.message = "terrain exaggeration must be at least 0".to_owned();
            return None;
        }
        terrain.exaggeration = exaggeration;
    }

    Some(terrain)
}
