/// The style's root `terrain`: the `raster-dem` source whose elevation the
/// map shows, and how much it exaggerates it.
///
/// The source isn't checked against the style's sources here: a style can
/// name a source it doesn't have, or add it later.
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
