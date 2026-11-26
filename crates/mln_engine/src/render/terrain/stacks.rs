//! The stacks of draped layers: the runs of the frame's layers that drape,
//! each drawn into its own drapes and shown by its own surface draw.

use smallvec::SmallVec;

use super::SurfaceLight;

/// How a frame layer that drapes takes part in its stack.
#[derive(Clone, Copy, Debug, PartialEq)]
#[expect(clippy::large_enum_variant, reason = "one a frame layer, which the stacks take once; a box would allocate it")]
pub(crate) enum Draped {
    /// It draws its colors over the layers under it.
    Plain,
    /// It blends with the layers under it otherwise than by covering them
    /// (a layer blend other than `normal`), so they must be in its drapes.
    Blends,
    /// Its shading can come from the surface, with the frame's `light`: a
    /// hillshade lit from the viewport, whose drapes then hold its slopes,
    /// the same at every bearing.
    Lit(SurfaceLight),
}

/// A frame's stacks of draped layers, as GL JS's `getRenderStacks` splits
/// them: each a run of consecutive frame layers that drape, ended by a frame
/// layer that doesn't. A layer that the surface can light ([`Draped::Lit`])
/// has a stack to itself, which the surface lights, unless a layer above it
/// in its run blends with what is under it, whose drapes then hold its
/// colors. Stacks are numbered bottom up from 0.
#[derive(Default)]
pub(crate) struct DrapeStacks {
    /// The draped layers' style indexes, stack after stack, each stack in
    /// drawing order.
    layers: SmallVec<[u32; 16]>,
    /// Where each stack's layers end in `layers`, and the light the surface
    /// shades the stack's drapes with, if it does.
    ends: SmallVec<[(usize, Option<SurfaceLight>); 4]>,
}

impl DrapeStacks {
    /// The stacks of `layers`, the frame's layers in drawing order, each
    /// with how it drapes, or `None` if it doesn't.
    pub(crate) fn of(layers: impl IntoIterator<Item = (u32, Option<Draped>)>) -> DrapeStacks {
        let mut stacks = DrapeStacks::default();
        let mut run: SmallVec<[(u32, Draped); 16]> = SmallVec::new();
        for (index, draped) in layers {
            match draped {
                Some(draped) => run.push((index, draped)),
                None => stacks.end_run(&mut run),
            }
        }
        stacks.end_run(&mut run);
        stacks
    }

    /// Adds the stacks of `run`, a run of draped layers, and empties it.
    fn end_run(&mut self, run: &mut SmallVec<[(u32, Draped); 16]>) {
        // The layers under the last that blends are in its drapes.
        let blended = run.iter().rposition(|(_, draped)| *draped == Draped::Blends).map_or(0, |last| last + 1);
        for (at, (index, draped)) in run.drain(..).enumerate() {
            match draped {
                Draped::Lit(light) if at >= blended => {
                    self.end_stack(None);
                    self.layers.push(index);
                    self.end_stack(Some(light));
                }
                _ => self.layers.push(index),
            }
        }
        self.end_stack(None);
    }

    /// Ends the stack in progress, if it has layers, which the surface
    /// shades with `light`.
    fn end_stack(&mut self, light: Option<SurfaceLight>) {
        if self.ends.last().map_or(0, |&(end, _)| end) < self.layers.len() {
            self.ends.push((self.layers.len(), light));
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
        let start = stack.checked_sub(1).map_or(0, |previous| self.ends[previous].0);
        &self.layers[start..self.ends[stack].0]
    }

    /// The light that the surface shades the drapes of `stack` with, whose
    /// one layer draws its slopes there; `None` for a stack whose drapes
    /// hold its layers' colors.
    pub(crate) fn light(&self, stack: usize) -> Option<&SurfaceLight> {
        self.ends[stack].1.as_ref()
    }

    /// The style index of each stack's first layer, where its surface draws,
    /// bottom up.
    pub(crate) fn starts(&self) -> impl Iterator<Item = u32> + '_ {
        (0..self.len()).map(|stack| self.layers(stack)[0])
    }
}

#[cfg(test)]
mod tests {
    use super::{DrapeStacks, Draped, SurfaceLight};

    const PLAIN: Option<Draped> = Some(Draped::Plain);
    const BLENDS: Option<Draped> = Some(Draped::Blends);
    const STANDS: Option<Draped> = None;

    fn lit() -> Draped {
        Draped::Lit(SurfaceLight::default())
    }

    /// The stacks of `layers`, each with whether it drapes: each stack a list
    /// of style indexes.
    fn stacks(layers: &[(u32, bool)]) -> Vec<Vec<u32>> {
        let layers: Vec<_> = layers.iter().map(|&(index, drapes)| (index, PLAIN.filter(|_| drapes))).collect();
        let (split, lit) = stacks_of(&layers);
        assert!(lit.iter().all(|lit| !lit));
        split
    }

    /// The stacks of `layers`, each a list of style indexes, and whether the
    /// surface lights each.
    fn stacks_of(layers: &[(u32, Option<Draped>)]) -> (Vec<Vec<u32>>, Vec<bool>) {
        let stacks = DrapeStacks::of(layers.iter().copied());
        assert_eq!(layers.iter().any(|&(_, draped)| draped.is_some()), !stacks.is_empty());
        let split: Vec<Vec<u32>> = (0..stacks.len()).map(|stack| stacks.layers(stack).to_vec()).collect();
        let starts: Vec<u32> = stacks.starts().collect();
        assert_eq!(split.iter().map(|layers| layers[0]).collect::<Vec<_>>(), starts);
        let lit = (0..stacks.len()).map(|stack| stacks.light(stack).is_some()).collect();
        (split, lit)
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

    /// A layer that the surface lights has a stack to itself, between the
    /// layers under and over it.
    #[test]
    fn a_lit_layer_stands_alone() {
        let layers = [(0, PLAIN), (1, Some(lit())), (2, PLAIN), (3, PLAIN)];
        assert_eq!((vec![vec![0], vec![1], vec![2, 3]], vec![false, true, false]), stacks_of(&layers));
        let layers = [(0, Some(lit())), (1, Some(lit())), (2, STANDS), (3, Some(lit()))];
        assert_eq!((vec![vec![0], vec![1], vec![3]], vec![true, true, true]), stacks_of(&layers));
    }

    /// A layer that blends over a lit one blends over its colors, in one
    /// stack with it, which the surface doesn't light; a lit layer over it
    /// still stands alone.
    #[test]
    fn a_blend_over_a_lit_layer_keeps_it_in_its_stack() {
        let layers = [(0, PLAIN), (1, Some(lit())), (2, PLAIN), (3, BLENDS), (4, Some(lit()))];
        assert_eq!((vec![vec![0, 1, 2, 3], vec![4]], vec![false, true]), stacks_of(&layers));
    }

    /// A blend under a lit layer, or past a layer that stands, doesn't keep
    /// it in its stack.
    #[test]
    fn a_blend_elsewhere_lets_a_lit_layer_stand_alone() {
        let layers = [(0, BLENDS), (1, Some(lit())), (2, STANDS), (3, BLENDS)];
        assert_eq!((vec![vec![0], vec![1], vec![3]], vec![false, true, false]), stacks_of(&layers));
    }
}
