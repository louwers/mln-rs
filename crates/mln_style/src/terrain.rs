use crate::conversion::{Convertible, Error};

/// The style's root `terrain`: the `raster-dem` source that gives the
/// ground its elevation, and how much it exaggerates it.
///
/// A style takes a terrain whose source it doesn't have, which waits for a
/// source with that id, but not one whose source isn't a `raster-dem` source
/// or whose exaggeration is below 0 or not finite. The map doesn't draw
/// terrain, so [`Style::set_root_property_json`](crate::Style::set_root_property_json)
/// refuses one.
#[derive(Clone, Debug, PartialEq)]
pub struct Terrain {
    /// The id of the terrain's source.
    pub source: String,
    /// The factor that elevation is multiplied by, at least 0.
    pub exaggeration: f32,
}

impl Terrain {
    /// The exaggeration of a terrain that doesn't set one.
    pub const DEFAULT_EXAGGERATION: f32 = 1.0;

    #[must_use]
    pub fn new(source: impl Into<String>) -> Self {
        Terrain { source: source.into(), exaggeration: Self::DEFAULT_EXAGGERATION }
    }
}

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
