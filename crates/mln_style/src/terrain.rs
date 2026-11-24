/// The style's root `terrain`: the `raster-dem` source that gives the
/// ground its elevation, and how much it exaggerates it.
///
/// A style takes a terrain whose source it doesn't have, which waits for a
/// source with that id, but not one whose source isn't a `raster-dem` source
/// or whose exaggeration is below 0 or not finite.
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
