//! Entry points to the layer types' internal stages, as functions over
//! data, for the benchmarks (`tools/bench`) to measure. Built only with the
//! `bench` feature, and hidden from the documentation: they aren't API.
//!
//! Each entry point takes and returns public types only, so that the
//! stages' internals can change without changing what the benchmarks call.
//! The engine's entry points, which lay tiles out and show them to a frame's
//! layers, are `mln_render::bench`'s.

use mln_render::UpdateParameters;
use mln_render::bench::{LaidOut, PlacementFrame};
use mln_util::error::Error;
use mln_util::geo::WrapMode;

use crate::symbol::SymbolLayer;
use crate::symbol::placement::cross_tile_index::CrossTileSymbolIndex;
use crate::symbol::placement::{LayerBuckets, SymbolPlacement};

/// The symbols of laid-out tiles in a view, as a renderer finds which to
/// show: the view's symbol layers, each updated with the tiles as a frame
/// updates it, the tiles' buckets, and the buckets each layer leads.
pub struct SymbolView {
    frame: PlacementFrame<SymbolLayer>,
    /// Each of the frame's layers' buckets, bottom to top, as the symbol
    /// participant finds them when it prepares a frame.
    layers: Vec<LayerBuckets>,
}

impl SymbolView {
    /// The view that `update` shows of `tiles`, laid out by the visible
    /// symbol layers on `source` in `update`'s style. The harness makes the
    /// update ([`PlacementFrame::new`]).
    pub fn new(source: &str, tiles: &[&LaidOut], update: UpdateParameters) -> Result<SymbolView, Error> {
        let mut frame = PlacementFrame::new(source, tiles, update)?;
        let mut layers = Vec::new();
        frame.with_context(|cx| LayerBuckets::fill(&mut layers, cx));
        Ok(SymbolView { frame, layers })
    }

    /// Adds the view's symbol buckets to `index`, as a renderer does before
    /// it places them: each symbol takes the id of the same symbol in a
    /// tile of another zoom that `index` holds, or a new one, and `index`
    /// drops the buckets of each layer that the view no longer has. Returns
    /// how many layers added a bucket.
    pub fn index(&mut self, index: &mut SymbolIndex) -> usize {
        let layers = &self.layers;
        self.frame.with_context(|cx| {
            let view = cx.view;
            let longitude = view.center(WrapMode::Unwrapped).longitude() as f32;
            let mut added = 0;
            for (layer, placing) in layers.iter().zip(cx.layers).rev().filter(|(layer, _)| !layer.placed.is_empty()) {
                let tiles = layer.placed.iter().map(|placed| &placing.tiles[placed.tile]);
                if index.0.index_layer(&layer.id, tiles, longitude, view, &mut cx.buckets).buckets_added {
                    added += 1;
                }
            }
            added
        })
    }

    /// Places the view's symbols, topmost layer first, each where it
    /// collides with none placed before it, after a placement that showed
    /// none: what a renderer does when the view changes. Returns how many
    /// symbols it shows. Their ids come from [`SymbolView::index`]: a symbol
    /// without one shows nowhere.
    pub fn place(&mut self) -> usize {
        let mut participant = SymbolPlacement::default();
        self.frame.with_context(|cx| participant.prepare_layers(cx));
        // The placement it replaces showed nothing, so it has nothing to
        // fade from.
        self.frame.place(&mut participant);
        participant.placement.current.count()
    }
}

/// The renderer's cross-tile symbol index: the symbols of the tiles that
/// it shows, by layer, with the ids that keep a symbol the same across
/// tiles of different zooms.
#[derive(Default)]
pub struct SymbolIndex(CrossTileSymbolIndex);
