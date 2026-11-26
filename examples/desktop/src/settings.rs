// The camera and options the app starts with, which the command line
// overrides.

use mln_util::constants::DEFAULT_FOV;

use crate::network::Network;

/// The camera and options the app starts with: central Amsterdam, as the
/// web demo opens.
#[derive(Clone, Debug)]
pub(crate) struct Settings {
    pub longitude: f64,
    pub latitude: f64,
    pub altitude: f64,
    pub zoom: f64,
    pub bearing: f64,
    pub pitch: f64,
    pub roll: f64,
    pub fov: f64,
    pub max_pitch: f64,
    pub network: Network,
    /// Don't keep what the map loads in the cache database.
    pub cache_disabled: bool,
    /// Drags and zooms on the globe turn the bearing with the planet.
    pub free_bearing: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            longitude: 4.89,
            latitude: 52.37,
            altitude: 0.0,
            zoom: 13.0,
            bearing: 0.0,
            pitch: 0.0,
            roll: 0.0,
            fov: mln_style::math::rad2deg(DEFAULT_FOV),
            // Past the map's default of 60°, to look across terrain.
            max_pitch: 85.0,
            network: Network::Online,
            cache_disabled: false,
            free_bearing: false,
        }
    }
}
