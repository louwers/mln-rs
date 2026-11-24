//! The stacks of draped layers: the runs of the frame's layers that drape,
//! each drawn into its own drapes and shown by its own surface draw.

use smallvec::SmallVec;

/// How a layer that drapes draws its drapes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Draping {
    /// The same at every bearing.
    Fixed,
    /// By the map's bearing: something of the layer is anchored to the
    /// viewport, so every frame that turns the map draws its drapes again.
    ByBearing,
    /// Not its colors but what the terrain's surface lights with the
    /// frame's lights, the same at every bearing: a hillshade lit from the
    /// viewport, on a backend whose surface lights it. It has a stack to
    /// itself, whose surface draw lights its drapes.
    Lit,
}

/// A frame layer that drapes.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Draped {
    pub(crate) draping: Draping,
    /// Whether it blends with what is under it otherwise than by covering
    /// it. Its backdrop is its drape's own texels, so it shares a stack
    /// with the layers of its run under it.
    pub(crate) blends: bool,
}

/// A frame's stacks of draped layers, as GL JS's `getRenderStacks` splits
/// them: each a run of consecutive frame layers that drape, ended by a frame
/// layer that doesn't. A stack also ends where the next layer drapes
/// otherwise ([`Draping`]), so that a turn of the map draws again only the
/// layers that draw by the bearing, and around a layer that the surface
/// lights, but not under a layer of its run that blends with what is under
/// it. Stacks are numbered bottom up from 0.
#[derive(Default)]
pub(crate) struct DrapeStacks {
    /// The draped layers' style indexes, stack after stack, each stack in
    /// drawing order.
    layers: SmallVec<[u32; 16]>,
    /// Where each stack's layers end in `layers`, and whether the surface
    /// lights the stack.
    ends: SmallVec<[(usize, bool); 4]>,
}

impl DrapeStacks {
    /// The stacks of `layers`, the frame's layers in drawing order, each
    /// with how it drapes, or `None` for a layer that doesn't.
    pub(crate) fn of(layers: impl IntoIterator<Item = (u32, Option<Draped>)>) -> DrapeStacks {
        let mut stacks = DrapeStacks::default();
        // The layers of the run in progress, the last of `stacks.layers`.
        let mut run: SmallVec<[Draped; 16]> = SmallVec::new();
        for (index, draped) in layers {
            match draped {
                Some(draped) => {
                    stacks.layers.push(index);
                    run.push(draped);
                }
                None => stacks.end_run(&mut run),
            }
        }
        stacks.end_run(&mut run);
        stacks
    }

    /// Ends the stacks of `run`, the last layers of `layers`, and empties
    /// it.
    fn end_run(&mut self, run: &mut SmallVec<[Draped; 16]>) {
        let start = self.layers.len() - run.len();
        // The layers up to the last that blends share a stack, which draws
        // by the bearing if one of them does: a layer that the surface could
        // light then drapes its colors there, for the blend.
        let shared = run.iter().rposition(|layer| layer.blends).map_or(1, |last| last + 1).min(run.len());
        let mut stack = match &run[..shared] {
            [alone] => alone.draping,
            shared if shared.iter().any(|layer| layer.draping != Draping::Fixed) => Draping::ByBearing,
            _ => Draping::Fixed,
        };
        for (at, layer) in run.iter().enumerate().skip(shared) {
            if layer.draping != stack || stack == Draping::Lit {
                self.ends.push((start + at, stack == Draping::Lit));
                stack = layer.draping;
            }
        }
        if !run.is_empty() {
            self.ends.push((self.layers.len(), stack == Draping::Lit));
        }
        run.clear();
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

    /// Whether the surface lights `stack`: it is of one layer, which drapes
    /// what the surface lights ([`Draping::Lit`]).
    pub(crate) fn lit(&self, stack: usize) -> bool {
        self.ends[stack].1
    }

    /// The style index of each stack's first layer, where its surface draws,
    /// bottom up.
    pub(crate) fn starts(&self) -> impl Iterator<Item = u32> + '_ {
        (0..self.len()).map(|stack| self.layers(stack)[0])
    }
}

#[cfg(test)]
mod tests {
    use super::{DrapeStacks, Draped, Draping};

    const FIXED: Option<Draped> = Some(Draped { draping: Draping::Fixed, blends: false });
    const TURNS: Option<Draped> = Some(Draped { draping: Draping::ByBearing, blends: false });
    const BLENDS: Option<Draped> = Some(Draped { draping: Draping::Fixed, blends: true });
    const LIT: Option<Draped> = Some(Draped { draping: Draping::Lit, blends: false });

    /// The stacks of `layers`, each with whether it drapes, the same at
    /// every bearing: each stack a list of style indexes.
    fn stacks(layers: &[(u32, bool)]) -> Vec<Vec<u32>> {
        let draped: Vec<_> = layers.iter().map(|&(index, drapes)| (index, FIXED.filter(|_| drapes))).collect();
        stacks_of(&draped)
    }

    /// The stacks of `layers`, each a list of style indexes.
    fn stacks_of(layers: &[(u32, Option<Draped>)]) -> Vec<Vec<u32>> {
        let stacks = DrapeStacks::of(layers.iter().copied());
        assert_eq!(layers.iter().any(|&(_, draped)| draped.is_some()), !stacks.is_empty());
        let split: Vec<Vec<u32>> = (0..stacks.len()).map(|stack| stacks.layers(stack).to_vec()).collect();
        // The surface lights the stacks of one layer that it lights, alone.
        for (stack, indexes) in split.iter().enumerate() {
            let lit = |draped: Option<Draped>| draped.is_some_and(|draped| draped.draping == Draping::Lit);
            let lights = |index: &u32| layers.iter().any(|&(of, draped)| of == *index && lit(draped));
            assert_eq!(indexes.len() == 1 && lights(&indexes[0]) && !blended(layers, indexes[0]), stacks.lit(stack));
        }
        let starts: Vec<u32> = stacks.starts().collect();
        assert_eq!(split.iter().map(|layers| layers[0]).collect::<Vec<_>>(), starts);
        split
    }

    /// Whether a layer of `layers` after the one of style index `index`, in
    /// its run, blends with what is under it.
    fn blended(layers: &[(u32, Option<Draped>)], index: u32) -> bool {
        let after = layers.iter().skip_while(|&&(of, _)| of != index).skip(1);
        after.map_while(|&(_, draped)| draped).any(|draped| draped.blends)
    }

    /// A layer that the surface lights has a stack to itself, and so has
    /// each of two together; under a layer that blends, it shares that
    /// layer's stack, which the surface doesn't light.
    #[test]
    fn a_layer_the_surface_lights_has_a_stack_to_itself() {
        assert_eq!(vec![vec![0], vec![1], vec![2]], stacks_of(&[(0, FIXED), (1, LIT), (2, FIXED)]));
        assert_eq!(vec![vec![0], vec![1]], stacks_of(&[(0, LIT), (1, LIT)]));
        assert_eq!(vec![vec![0], vec![1], vec![2]], stacks_of(&[(0, TURNS), (1, LIT), (2, TURNS)]));
        assert_eq!(vec![vec![0, 1, 2], vec![3]], stacks_of(&[(0, FIXED), (1, LIT), (2, BLENDS), (3, LIT)]));
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

    /// A layer that draws by the bearing has a stack of its own, between
    /// those of the layers under and over it, so a turn of the map draws
    /// only its drapes again.
    #[test]
    fn a_layer_that_draws_by_the_bearing_splits_a_stack_in_three() {
        assert_eq!(
            vec![vec![0, 1], vec![2], vec![3, 4]],
            stacks_of(&[(0, FIXED), (1, FIXED), (2, TURNS), (3, FIXED), (4, FIXED)])
        );
    }

    #[test]
    fn layers_that_draw_by_the_bearing_together_share_a_stack() {
        assert_eq!(vec![vec![0], vec![1, 2]], stacks_of(&[(0, FIXED), (1, TURNS), (2, TURNS)]));
        assert_eq!(vec![vec![0], vec![2]], stacks_of(&[(0, TURNS), (1, None), (2, TURNS)]));
    }

    /// A layer that blends with what is under it keeps the layers of its
    /// run under it in its stack, which no layer that draws by the bearing
    /// splits; the layers over it split as they would.
    #[test]
    fn a_layer_that_blends_shares_a_stack_with_the_layers_under_it() {
        assert_eq!(vec![vec![0, 1, 2]], stacks_of(&[(0, FIXED), (1, TURNS), (2, BLENDS)]));
        // The stack under the last layer draws by the bearing.
        assert_eq!(vec![vec![0, 1, 2], vec![3]], stacks_of(&[(0, FIXED), (1, TURNS), (2, BLENDS), (3, FIXED)]));
        assert_eq!(vec![vec![0, 1], vec![2], vec![3]], stacks_of(&[(0, FIXED), (1, BLENDS), (2, TURNS), (3, FIXED)]));
        // A layer that stands between them is the backdrop's end, as before.
        assert_eq!(vec![vec![0], vec![1], vec![3]], stacks_of(&[(0, FIXED), (1, TURNS), (2, None), (3, BLENDS)]));
    }

    #[test]
    fn no_draped_layer_is_no_stack() {
        let none: Vec<Vec<u32>> = Vec::new();
        assert_eq!(none, stacks(&[(0, false), (1, false)]));
        assert_eq!(none, stacks(&[]));
    }
}
