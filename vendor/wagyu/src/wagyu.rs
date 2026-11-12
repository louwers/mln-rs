// Ports vendor/wagyu/include/mapbox/geometry/wagyu/wagyu.hpp (the `wagyu<T>`
// class, for T = int32_t).

use geometry::{LinearRing, MultiPolygon, Polygon};

use crate::bound::Bound;
use crate::build_result::{OutputCoordinate, build_result};
use crate::config::{ClipType, FillType, PolygonType};
use crate::local_minimum::LocalMinimum;
use crate::local_minimum_util::add_linear_ring;
use crate::ring::RingManager;
use crate::snap_rounding::build_hot_pixels;
use crate::topology_correction::correct_topology;
use crate::vatti::execute_vatti;

/// `mapbox::geometry::wagyu::wagyu<int32_t>`: a polygon clipper (Vatti with
/// snap rounding and topology correction). Errors are the C++
/// `std::runtime_error` messages.
#[derive(Debug, Default)]
pub struct Wagyu {
    minima_list: Vec<LocalMinimum>,
    /// The bounds of all local minima (C++ stores them inside the deque).
    bounds: Vec<Bound>,
    reverse_output: bool,
}

impl Wagyu {
    #[must_use]
    pub fn new() -> Self {
        Wagyu::default()
    }

    /// `add_ring`: false if the ring is degenerate (fewer than three
    /// distinct, non-collinear points).
    pub fn add_ring(&mut self, pg: &LinearRing<i32>, p_type: PolygonType) -> Result<bool, String> {
        add_linear_ring(pg, &mut self.minima_list, &mut self.bounds, p_type)
    }

    /// `add_polygon`.
    pub fn add_polygon(&mut self, ppg: &Polygon<i32>, p_type: PolygonType) -> Result<bool, String> {
        let mut result = false;
        for r in ppg {
            if self.add_ring(r, p_type)? {
                result = true;
            }
        }
        Ok(result)
    }

    /// `reverse_rings`.
    pub fn reverse_rings(&mut self, value: bool) {
        self.reverse_output = value;
    }

    /// `clear`.
    pub fn clear(&mut self) {
        self.minima_list.clear();
        self.bounds.clear();
    }

    // TODO(port): get_bounds (unused by the C++ core).

    /// `execute<T2>`: false (and no output) without rings.
    pub fn execute<T2: OutputCoordinate>(
        &mut self,
        cliptype: ClipType,
        solution: &mut MultiPolygon<T2>,
        subject_fill_type: FillType,
        clip_fill_type: FillType,
    ) -> Result<bool, String> {
        if self.minima_list.is_empty() {
            return Ok(false);
        }

        let mut manager = RingManager::default();

        build_hot_pixels(&self.minima_list, &mut self.bounds, &mut manager)?;

        execute_vatti(&self.minima_list, &mut self.bounds, &mut manager, cliptype, subject_fill_type, clip_fill_type)?;

        correct_topology(&mut manager)?;

        build_result(solution, &mut manager, self.reverse_output);

        Ok(true)
    }
}
