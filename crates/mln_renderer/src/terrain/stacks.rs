//! The stacks of draped layers: the runs of the frame's layers that drape,
//! each drawn into its own drapes and shown by its own surface draw.

use smallvec::SmallVec;

/// A frame's stacks of draped layers, as GL JS's `getRenderStacks` splits
/// them: each a run of consecutive frame layers that drape, ended by a frame
/// layer that doesn't. Stacks are numbered bottom up from 0.
#[derive(Default)]
pub(crate) struct DrapeStacks {
    /// The draped layers' style indexes, stack after stack, each stack in
    /// drawing order.
    layers: SmallVec<[u32; 16]>,
    /// Where each stack's layers end in `layers`.
    ends: SmallVec<[usize; 4]>,
}

impl DrapeStacks {
    /// The stacks of `layers`, the frame's layers in drawing order, each
    /// with whether it drapes.
    pub(crate) fn of(layers: impl IntoIterator<Item = (u32, bool)>) -> DrapeStacks {
        let mut stacks = DrapeStacks::default();
        for (index, drapes) in layers {
            if drapes {
                stacks.layers.push(index);
            } else {
                stacks.end_run();
            }
        }
        stacks.end_run();
        stacks
    }

    /// Ends the run of draped layers in progress, if it has any.
    fn end_run(&mut self) {
        if self.ends.last().copied().unwrap_or(0) < self.layers.len() {
            self.ends.push(self.layers.len());
        }
    }

    /// The number of stacks.
    pub(crate) fn len(&self) -> usize {
        self.ends.len()
    }

    /// Whether there is no stack: no frame layer drapes.
    pub(crate) fn is_empty(&self) -> bool {
        self.ends.is_empty()
    }

    /// The style indexes of the layers of `stack`, in drawing order.
    pub(crate) fn layers(&self, stack: usize) -> &[u32] {
        let start = stack.checked_sub(1).map_or(0, |previous| self.ends[previous]);
        &self.layers[start..self.ends[stack]]
    }

    /// The style index of each stack's first layer, where its surface draws,
    /// bottom up.
    pub(crate) fn starts(&self) -> impl Iterator<Item = u32> + '_ {
        (0..self.len()).map(|stack| self.layers(stack)[0])
    }
}

#[cfg(test)]
mod tests {
    use super::DrapeStacks;

    /// The stacks of `layers`, each with whether it drapes: each stack a list
    /// of style indexes.
    fn stacks(layers: &[(u32, bool)]) -> Vec<Vec<u32>> {
        let stacks = DrapeStacks::of(layers.iter().copied());
        assert_eq!(layers.iter().any(|&(_, drapes)| drapes), !stacks.is_empty());
        let split: Vec<Vec<u32>> = (0..stacks.len()).map(|stack| stacks.layers(stack).to_vec()).collect();
        let starts: Vec<u32> = stacks.starts().collect();
        assert_eq!(split.iter().map(|layers| layers[0]).collect::<Vec<_>>(), starts);
        split
    }

    #[test]
    fn layers_that_all_drape_are_one_stack() {
        assert_eq!(vec![vec![0, 1, 2]], stacks(&[(0, true), (1, true), (2, true)]));
    }

    #[test]
    fn a_layer_that_stands_splits_a_stack_in_two() {
        assert_eq!(vec![vec![0, 1], vec![3]], stacks(&[(0, true), (1, true), (2, false), (3, true)]));
    }

    #[test]
    fn a_leading_layer_that_stands_starts_no_stack() {
        assert_eq!(vec![vec![1, 2]], stacks(&[(0, false), (1, true), (2, true)]));
    }

    #[test]
    fn a_trailing_layer_that_stands_ends_the_last_stack() {
        assert_eq!(vec![vec![0, 1]], stacks(&[(0, true), (1, true), (2, false), (3, false)]));
    }

    #[test]
    fn layers_that_stand_together_split_once() {
        assert_eq!(vec![vec![0], vec![3]], stacks(&[(0, true), (1, false), (2, false), (3, true)]));
    }

    /// The frame's layers skip those it doesn't draw, which don't split a
    /// stack.
    #[test]
    fn the_indexes_need_not_be_consecutive() {
        assert_eq!(vec![vec![2, 5]], stacks(&[(2, true), (5, true)]));
    }

    #[test]
    fn no_draped_layer_is_no_stack() {
        let none: Vec<Vec<u32>> = Vec::new();
        assert_eq!(none, stacks(&[(0, false), (1, false)]));
        assert_eq!(none, stacks(&[]));
    }
}
