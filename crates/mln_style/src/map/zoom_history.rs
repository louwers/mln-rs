// Ports src/mln/map/zoom_history.hpp. It lives in mln_style because
// renderer/property_evaluation_parameters.hpp needs it (a cut point of the C++ include cycle).

use mln_util::chrono::{Duration, TimePoint};

#[derive(Clone, Copy, Debug)]
pub struct ZoomHistory {
    pub last_zoom: f32,
    pub last_floor_zoom: f32,
    pub last_integer_zoom: f32,
    pub last_integer_zoom_time: TimePoint,
    pub first: bool,
}

impl Default for ZoomHistory {
    fn default() -> Self {
        ZoomHistory {
            last_zoom: 0.0,
            last_floor_zoom: 0.0,
            last_integer_zoom: 0.0,
            last_integer_zoom_time: TimePoint::default(),
            first: true,
        }
    }
}

impl ZoomHistory {
    pub fn update(&mut self, z: f32, now: &TimePoint) -> bool {
        let zero = TimePoint::new(Duration::zero());
        let floor_z = z.floor();

        if self.first {
            self.first = false;
            self.last_integer_zoom = floor_z;
            self.last_integer_zoom_time = zero;
            self.last_zoom = z;
            self.last_floor_zoom = floor_z;
            return true;
        }

        if self.last_floor_zoom > floor_z {
            self.last_integer_zoom = floor_z + 1.0;
            self.last_integer_zoom_time = if *now == TimePoint::max() { zero } else { *now };
        } else if self.last_floor_zoom < floor_z {
            self.last_integer_zoom = floor_z;
            self.last_integer_zoom_time = if *now == TimePoint::max() { zero } else { *now };
        }

        if z != self.last_zoom {
            self.last_zoom = z;
            self.last_floor_zoom = floor_z;
            return true;
        }

        false
    }
}
