// Ports src/mln/renderer/transition_parameters.hpp (a cut point of the C++ include cycle).

use std::sync::Arc;

use mln_util::chrono::TimePoint;
use mln_vec_map::VecSet;

use crate::expression::GlobalStateMap;
use crate::transition_options::TransitionOptions;

/// A change of the style's global state, across which the values that read it
/// transition from what they showed before.
#[derive(Clone, Debug)]
pub struct GlobalStateChange {
    /// The state before the change, which transitions already running keep reading.
    pub prior_state: Arc<GlobalStateMap>,
    /// The keys whose values differ between the prior and the current state.
    pub changed_keys: Arc<VecSet<String>>,
    /// Whether the global state is all that changed, so only the values that
    /// read a changed key start a new transition.
    pub state_only: bool,
}

#[derive(Clone, Debug, Default)]
pub struct TransitionParameters {
    pub now: TimePoint,
    pub transition: TransitionOptions,
    /// Set when the global state changed since the last transition.
    pub global_state_change: Option<GlobalStateChange>,
}

impl TransitionParameters {
    /// Parameters without a global-state change.
    pub fn new(now: TimePoint, transition: TransitionOptions) -> Self {
        TransitionParameters { now, transition, global_state_change: None }
    }

    pub fn is_global_state_only(&self) -> bool {
        self.global_state_change.as_ref().is_some_and(|change| change.state_only)
    }
}
