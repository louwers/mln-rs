use mln_style::tile_id::CanonicalTileID;
use mln_tile_id::UnwrappedTileID;

use crate::tile_mask::TileMask;

fn compute_tile_masks(
    root: &CanonicalTileID,
    ref_: &UnwrappedTileID,
    renderables: &[(UnwrappedTileID, bool)],
    mask: &mut TileMask,
) {
    // If the reference or any of its children is found in the list, we need
    // to recurse.
    for (i, (id, needs_mask)) in renderables.iter().enumerate() {
        if !needs_mask {
            continue;
        }
        if ref_ == id {
            // The current tile is masked out, so we don't need to add them
            // to the mask set.
            return;
        }
        if id.is_child_of(ref_) {
            // There's at least one child tile that is masked out, so
            // recursively descend.
            for child in ref_.children() {
                compute_tile_masks(root, &child, &renderables[i..], mask);
            }
            return;
        }
    }

    // We couldn't find a child, so it's definitely a masked part. Compute
    // the difference between the root tile ID and the reference tile ID,
    // since TileMask elements are always relative.
    let diff_z = ref_.canonical.z - root.z;
    mask.insert(CanonicalTileID::new(
        diff_z,
        ref_.canonical.x.wrapping_sub(root.x.wrapping_shl(u32::from(diff_z))),
        ref_.canonical.y.wrapping_sub(root.y.wrapping_shl(u32::from(diff_z))),
    ));
}

/// The `TileMask`s of the renderable tiles, sorted by id, each with whether
/// it needs one (`None` for those that don't). A tile's mask describes the
/// regions of the tile that are *not* covered by other renderable tiles, as
/// tile ids relative to the tile.
pub(crate) fn update_tile_masks(renderables: &[(UnwrappedTileID, bool)]) -> Vec<Option<TileMask>> {
    let mut masks = Vec::with_capacity(renderables.len());
    for (i, (id, needs_mask)) in renderables.iter().enumerate() {
        if !needs_mask {
            masks.push(None);
            continue;
        }
        // Try to add all remaining ids as children. We sorted the tile list
        // by z earlier, so all preceding items cannot be children of the
        // current tile. We also compute the lower bound of the next wrap,
        // because items of the next wrap can never be children of the
        // current wrap.
        let rest = &renderables[i + 1..];
        let next_wrap = UnwrappedTileID::with_wrap(id.wrap.wrapping_add(1), CanonicalTileID::new(0, 0, 0));
        let children_end = rest.partition_point(|(a, _)| *a < next_wrap);
        let mut mask = TileMask::new();
        compute_tile_masks(&id.canonical, id, &rest[..children_end], &mut mask);
        masks.push(Some(mask));
    }
    masks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(z: u8, x: u32, y: u32) -> CanonicalTileID {
        CanonicalTileID::new(z, x, y)
    }

    fn u(z: u8, x: i64, y: i64) -> UnwrappedTileID {
        UnwrappedTileID::new(z, x, y)
    }

    fn validate(expected: Vec<(UnwrappedTileID, Vec<CanonicalTileID>)>) {
        let mut expected = expected;
        // std::map order
        expected.sort_by_key(|a| a.0);
        // `usedByRenderedLayers = true`
        let renderables: Vec<(UnwrappedTileID, bool)> = expected.iter().map(|(id, _)| (*id, true)).collect();
        let masks = update_tile_masks(&renderables);
        for ((id, mask), actual) in expected.iter().zip(masks) {
            let expected_mask: TileMask = mask.iter().copied().collect();
            assert_eq!(Some(expected_mask), actual, "mask of {id:?}");
        }
    }

    // gtest: UpdateTileMasks.NoChildren
    #[test]
    fn no_children() {
        validate(vec![(u(0, 0, 0), vec![c(0, 0, 0)])]);
        validate(vec![(u(4, 3, 8), vec![c(0, 0, 0)])]);
        validate(vec![(u(1, 0, 0), vec![c(0, 0, 0)]), (u(1, 1, 1), vec![c(0, 0, 0)])]);
        validate(vec![(u(1, 0, 0), vec![c(0, 0, 0)]), (u(2, 2, 3), vec![c(0, 0, 0)])]);
    }

    // gtest: UpdateTileMasks.ParentAndFourChildren
    #[test]
    fn parent_and_four_children() {
        validate(vec![
            // Mask is empty (== not rendered!) because we have four covering
            // children.
            (u(0, 0, 0), vec![]),
            // All four covering children
            (u(1, 0, 0), vec![c(0, 0, 0)]),
            (u(1, 0, 1), vec![c(0, 0, 0)]),
            (u(1, 1, 0), vec![c(0, 0, 0)]),
            (u(1, 1, 1), vec![c(0, 0, 0)]),
        ]);
    }

    // gtest: UpdateTileMasks.OneChild
    #[test]
    fn one_child() {
        validate(vec![
            // Only render the three children that aren't covering the other
            // tile.
            (u(0, 0, 0), vec![c(1, 0, 1), c(1, 1, 0), c(1, 1, 1)]),
            (u(1, 0, 0), vec![c(0, 0, 0)]),
        ]);
    }

    // gtest: UpdateTileMasks.Complex
    #[test]
    fn complex() {
        validate(vec![
            (u(0, 0, 0), vec![c(1, 0, 1), c(1, 1, 0), c(2, 2, 3), c(2, 3, 2), c(3, 6, 7), c(3, 7, 6)]),
            (UnwrappedTileID::with_wrap(0, c(1, 0, 0)), vec![c(0, 0, 0)]),
            (UnwrappedTileID::with_wrap(0, c(2, 2, 2)), vec![c(0, 0, 0)]),
            (UnwrappedTileID::with_wrap(0, c(3, 7, 7)), vec![c(0, 0, 0)]),
            (UnwrappedTileID::with_wrap(0, c(3, 6, 6)), vec![c(0, 0, 0)]),
        ]);

        validate(vec![
            (
                u(0, 0, 0),
                vec![
                    c(1, 0, 1),
                    c(1, 1, 0),
                    c(1, 1, 1),
                    c(2, 0, 0),
                    c(2, 0, 1),
                    c(2, 1, 0),
                    c(3, 2, 3),
                    c(3, 3, 2),
                    c(3, 3, 3),
                    c(4, 4, 5),
                    c(4, 5, 4),
                    c(4, 5, 5),
                ],
            ),
            (u(4, 4, 4), vec![c(0, 0, 0)]),
        ]);

        validate(vec![
            (u(12, 1028, 1456), vec![c(1, 1, 1), c(2, 3, 0), c(2, 3, 1)]),
            (u(13, 2056, 2912), vec![c(1, 0, 1), c(1, 1, 0), c(1, 1, 1)]),
            (u(13, 2056, 2913), vec![c(1, 0, 0), c(1, 1, 0), c(1, 1, 1)]),
            (u(14, 4112, 5824), vec![c(0, 0, 0)]),
            (u(14, 4112, 5827), vec![c(0, 0, 0)]),
            (u(14, 4114, 5824), vec![c(0, 0, 0)]),
            (u(14, 4114, 5825), vec![c(0, 0, 0)]),
        ]);
    }
}
