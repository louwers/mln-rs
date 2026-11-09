// Ports src/mln/renderer/transition_parameters.hpp (a cut point; see
// PLAN.md, "Crate map").

use mln_util::chrono::TimePoint;

use crate::transition_options::TransitionOptions;

#[derive(Clone, Copy, Debug, Default)]
pub struct TransitionParameters {
    pub now: TimePoint,
    pub transition: TransitionOptions,
}
