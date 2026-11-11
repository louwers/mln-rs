// Ports vendor/wagyu/include/mapbox/geometry/wagyu/vatti.hpp.

use crate::active_bound_list::insert_local_minima_into_abl;
use crate::bound::{Bound, BoundPtr};
use crate::config::{ClipType, FillType};
use crate::intersect_util::process_intersections;
use crate::local_minimum::{LocalMinimum, local_minimum_sorter};
use crate::process_maxima::process_edges_at_top_of_scanbeam;
use crate::ring::{R, RingManager};
use crate::ring_util::update_current_hp_itr;
use crate::scanbeam::{pop_from_scanbeam, setup_scanbeam};

/// `execute_vatti`.
pub(crate) fn execute_vatti(
    minima: &[LocalMinimum],
    b: &mut [Bound],
    manager: &mut RingManager,
    cliptype: ClipType,
    subject_fill_type: FillType,
    clip_fill_type: FillType,
) -> R<()> {
    let mut active_bounds: Vec<BoundPtr> = Vec::new();
    let mut scanbeam: Vec<i32> = Vec::new();
    let mut scanline_y = i32::MAX;

    let mut minima_sorted: Vec<usize> = (0..minima.len()).collect();
    minima_sorted.sort_by(|&a, &c| local_minimum_sorter(&minima[a], &minima[c]));
    let mut current_lm = 0;

    setup_scanbeam(minima, &mut scanbeam);
    manager.current_hp_itr = 0;

    loop {
        let popped = pop_from_scanbeam(&mut scanline_y, &mut scanbeam);
        if !(popped || current_lm != minima_sorted.len()) {
            break;
        }
        process_intersections(scanline_y, &mut active_bounds, cliptype, subject_fill_type, clip_fill_type, b, manager)?;

        update_current_hp_itr(scanline_y, manager);

        // First we process bounds that has already been added to the active bound list --
        // if the active bound list is empty local minima that are at this scanline_y and
        // have a horizontal edge at the local minima will be processed
        process_edges_at_top_of_scanbeam(
            scanline_y,
            &mut active_bounds,
            &mut scanbeam,
            minima,
            &minima_sorted,
            &mut current_lm,
            b,
            manager,
            cliptype,
            subject_fill_type,
            clip_fill_type,
        )?;

        // Next we will add local minima bounds to the active bounds list that are on the local
        // minima queue at
        // this current scanline_y
        insert_local_minima_into_abl(
            scanline_y,
            minima,
            &minima_sorted,
            &mut current_lm,
            &mut active_bounds,
            b,
            manager,
            &mut scanbeam,
            cliptype,
            subject_fill_type,
            clip_fill_type,
        );
    }
    Ok(())
}
