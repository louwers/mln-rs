// Ports src/mln/map/zoom_history.hpp. It lives in mln_style because
// renderer/property_evaluation_parameters.hpp needs it (a cut point of the C++ include cycle).

use mln_util::time::Instant;

#[derive(Clone, Copy, Debug)]
pub struct ZoomHistory {
    pub last_zoom: f32,
    pub last_floor_zoom: f32,
    pub last_integer_zoom: f32,
    /// When the zoom last crossed an integer; `None` for long ago.
    pub last_integer_zoom_time: Option<Instant>,
    pub first: bool,
}

impl Default for ZoomHistory {
    fn default() -> Self {
        ZoomHistory {
            last_zoom: 0.0,
            last_floor_zoom: 0.0,
            last_integer_zoom: 0.0,
            last_integer_zoom_time: None,
            first: true,
        }
    }
}

impl ZoomHistory {
    /// Records zoom `z` at `now`, or at no time in particular for `None`.
    pub fn update(&mut self, z: f32, now: Option<Instant>) -> bool {
        let floor_z = z.floor();

        if self.first {
            self.first = false;
            self.last_integer_zoom = floor_z;
            self.last_integer_zoom_time = None;
            self.last_zoom = z;
            self.last_floor_zoom = floor_z;
            return true;
        }

        if self.last_floor_zoom > floor_z {
            self.last_integer_zoom = floor_z + 1.0;
            self.last_integer_zoom_time = now;
        } else if self.last_floor_zoom < floor_z {
            self.last_integer_zoom = floor_z;
            self.last_integer_zoom_time = now;
        }

        if z != self.last_zoom {
            self.last_zoom = z;
            self.last_floor_zoom = floor_z;
            return true;
        }

        false
    }
}
