//! The placement schedule: when the renderer places again, and the clock
//! that the current placement's fade runs on.

use std::time::Duration;

use mln_map_types::MapMode;
use mln_style::TransitionOptions;
use mln_util::constants::DEFAULT_TRANSITION_DURATION;
use mln_util::time::Instant;

use super::Placed;

/// How long after `since` `now` is: zero if it is before, and
/// `Duration::MAX` without either time, as for a time long ago.
fn elapsed(now: Option<Instant>, since: Option<Instant>) -> Duration {
    match (now, since) {
        (Some(now), Some(since)) => now.saturating_duration_since(since),
        _ => Duration::MAX,
    }
}

/// The clock of a placement: when it was made, and how what it shows fades.
#[derive(Clone, Copy, Debug)]
struct Clock {
    /// Whether a pass made the placement, rather than it being the empty
    /// placement before the first or after a reset.
    placed: bool,
    /// Whether what shows fades between placements: only in continuous
    /// mode, where the style enables placement transitions.
    fades: bool,
    fade_duration: Duration,
    /// The time of the update that placed; `None` for long ago.
    commit_time: Option<Instant>,
    /// When what shows last began to fade; `None` for long ago.
    fade_start_time: Option<Instant>,
    /// The zoom placed at.
    zoom: f32,
    /// How far ahead of its time the fade starts, for a placement made
    /// zoomed out from the one before.
    prev_zoom_adjustment: f32,
    /// Everything that shows or hides was at the end of its fade when the
    /// placement was made.
    settled: bool,
}

impl Clock {
    /// The clock of a placement made at `time_point`, at `zoom`, with the
    /// style's `transition_options`, in `mode`.
    fn new(mode: MapMode, transition_options: TransitionOptions, zoom: f32, time_point: Option<Instant>) -> Self {
        Clock {
            placed: false,
            fades: mode == MapMode::Continuous && transition_options.enable_placement_transitions,
            fade_duration: transition_options.duration.unwrap_or(DEFAULT_TRANSITION_DURATION),
            commit_time: time_point,
            fade_start_time: None,
            zoom,
            prev_zoom_adjustment: 0.0,
            settled: false,
        }
    }

    /// How much quicker what shows fades, and placements run, while the map
    /// zooms out from the placement's zoom to `zoom`, where what shows comes
    /// to overlap quickly.
    fn zoom_adjustment(&self, zoom: f32) -> f32 {
        ((self.zoom - zoom) / 1.5).max(0.0)
    }
}

impl Default for Clock {
    /// The clock of the empty placement.
    fn default() -> Self {
        Clock::new(MapMode::Continuous, TransitionOptions::default(), 0.0, None)
    }
}

/// When the renderer places, and the clock of its current placement, which
/// the placement's fade and the next placement's time read.
#[derive(Default)]
pub(crate) struct Schedule {
    clock: Clock,
    commits: u64,
}

impl Schedule {
    /// How many placements the schedule has committed, each reset to the
    /// empty placement included.
    #[must_use]
    pub(crate) fn commits(&self) -> u64 {
        self.commits
    }

    /// Whether everything the current placement shows or hides was at the
    /// end of its fade when it was made.
    #[must_use]
    pub(crate) fn is_settled(&self) -> bool {
        self.clock.settled
    }

    /// Commits a placement made at `time_point`, at `zoom`, with the style's
    /// `transition_options`, in `mode`, as `placed` describes it. In
    /// continuous mode, its fade follows on from the placement it replaces.
    pub(crate) fn commit(
        &mut self,
        mode: MapMode,
        transition_options: TransitionOptions,
        zoom: f32,
        time_point: Option<Instant>,
        placed: Placed,
    ) {
        let prev = self.clock;
        let mut clock = Clock::new(mode, transition_options, zoom, time_point);
        clock.placed = true;
        if mode == MapMode::Continuous {
            clock.prev_zoom_adjustment = prev.zoom_adjustment(zoom);
            clock.fade_start_time = if placed.changed { time_point } else { prev.fade_start_time };
            clock.settled = placed.settled;
        } else {
            clock.fade_start_time = time_point;
        }
        self.clock = clock;
        self.commits += 1;
    }

    /// Commits the empty placement; returns whether a placement of a pass
    /// was discarded.
    pub(crate) fn reset(&mut self) -> bool {
        let changed = self.clock.placed;
        if changed {
            self.clock = Clock::default();
        }
        self.commits += 1;
        changed
    }

    /// How far the current placement's fade has come at `now`, from 0 to 1
    /// and past it once it is done: always 1 where nothing fades.
    #[must_use]
    pub(crate) fn fade_change(&self, now: Option<Instant>) -> f32 {
        let clock = &self.clock;
        if clock.fades && clock.fade_duration > Duration::ZERO {
            return elapsed(now, clock.commit_time).div_duration_f32(clock.fade_duration) + clock.prev_zoom_adjustment;
        }
        1.0
    }

    /// When the next placement may run, at `zoom`: an update period, or
    /// `period_override`, after the current one; `None` for at once, as
    /// where placements don't fade.
    #[must_use]
    pub(crate) fn next_placement_at(&self, zoom: f32, period_override: Option<Duration>) -> Option<Instant> {
        let clock = &self.clock;
        if !clock.fades {
            return None;
        }
        let update_period = period_override.unwrap_or_else(|| {
            // Even with a transition shorter than the default, placement
            // waits for the default before it runs again; zero once the
            // zoom adjustment passes 1.
            let fade_duration = clock.fade_duration.max(DEFAULT_TRANSITION_DURATION);
            fade_duration.mul_f64(f64::from(1.0 - clock.zoom_adjustment(zoom)).max(0.0))
        });
        clock.commit_time?.checked_add(update_period)
    }

    /// Whether the current placement's fade still runs at `now`.
    #[must_use]
    pub(crate) fn has_transitions(&self, now: Option<Instant>) -> bool {
        let clock = &self.clock;
        clock.fades && elapsed(now, clock.fade_start_time) < clock.fade_duration
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(schedule: &mut Schedule, time: Instant, changed: bool, settled: bool) {
        let options = TransitionOptions::default();
        schedule.commit(MapMode::Continuous, options, 0.0, Some(time), Placed { changed, settled });
    }

    #[test]
    fn a_fade_starts_when_what_shows_changes() {
        let mut schedule = Schedule::default();
        let start = Instant::now();
        commit(&mut schedule, start, true, false);
        assert_eq!(schedule.fade_change(Some(start)), 0.0);
        assert!(schedule.has_transitions(Some(start + Duration::from_millis(100))));

        commit(&mut schedule, start + Duration::from_millis(150), false, false);
        assert!(schedule.has_transitions(Some(start + Duration::from_millis(200))), "the fade goes on");
        assert!(!schedule.has_transitions(Some(start + Duration::from_secs(1))), "the fade ended");
        assert_eq!(schedule.next_placement_at(0.0, None), Some(start + Duration::from_millis(450)));
        assert_eq!(schedule.commits(), 2);
    }

    #[test]
    fn a_reset_discards_only_a_placement() {
        let mut schedule = Schedule::default();
        assert!(!schedule.reset());
        commit(&mut schedule, Instant::now(), true, true);
        assert!(schedule.is_settled());
        assert!(schedule.reset());
        assert!(!schedule.is_settled());
        assert_eq!(schedule.commits(), 3);
    }

    #[test]
    fn a_still_image_shows_its_placement_at_once() {
        let mut schedule = Schedule::default();
        let options = TransitionOptions::default();
        schedule.commit(MapMode::Static, options, 0.0, None, Placed { changed: true, settled: true });
        assert_eq!(schedule.fade_change(None), 1.0);
        assert!(!schedule.has_transitions(None));
        assert_eq!(schedule.next_placement_at(0.0, None), None);
        assert!(!schedule.is_settled());
    }
}
