// Ports platform/glfw/settings_json.hpp (the fields and their defaults).
//
// TODO(port): `load()`/`save()` of /tmp/mbgl-native.cfg (C++ restores the
// last camera at startup and saves it at exit); the port always starts from
// the defaults and the command line.

use mln_util::constants::{DEFAULT_FOV, DEFAULT_PITCH_MAX};

/// `Settings_JSON`
#[derive(Clone, Debug)]
pub struct SettingsJSON {
    pub longitude: f64,
    pub latitude: f64,
    pub altitude: f64,
    pub zoom: f64,
    pub bearing: f64,
    pub pitch: f64,
    pub roll: f64,
    pub fov: f64,
    pub max_pitch: f64,
    pub axonometric: bool,
    pub x_skew: f64,
    pub y_skew: f64,
    pub debug: u32,
    pub online: bool,
}

impl Default for SettingsJSON {
    fn default() -> Self {
        SettingsJSON {
            longitude: 0.0,
            latitude: 0.0,
            altitude: 0.0,
            zoom: 0.0,
            bearing: 0.0,
            pitch: 0.0,
            roll: 0.0,
            fov: mln_math::rad2deg(DEFAULT_FOV),
            max_pitch: mln_math::rad2deg(DEFAULT_PITCH_MAX),
            axonometric: false,
            x_skew: 0.0,
            y_skew: 1.0,
            debug: 0,
            online: true,
        }
    }
}
