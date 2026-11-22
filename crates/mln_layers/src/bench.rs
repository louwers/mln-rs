//! Entry points to the layer types' internal stages, as functions over
//! data, for the benchmarks (`tools/bench`) to measure. Built only with the
//! `bench` feature, and hidden from the documentation: they aren't API.
//!
//! Each entry point takes and returns public types only, so that the
//! stages' internals can change without changing what the benchmarks call.
//! The engine's entry points, which lay tiles out and show them to a frame's
//! layers, are `mln_render::bench`'s.

use mln_render::bench::{LaidOut, PlacementFrame};
use mln_style::Style;
use mln_util::error::Error;
use mln_util::geo::{LatLng, WrapMode};

use crate::symbol::SymbolLayer;
use crate::symbol::placement::cross_tile_index::CrossTileSymbolIndex;
use crate::symbol::placement::{LayerBuckets, SymbolPlacement, leaders};

/// The symbols of laid-out tiles in a view, as a renderer finds which to
/// show: the view's symbol layers, each updated with the tiles as a frame
/// updates it, and the tiles' buckets.
pub struct SymbolView(PlacementFrame<SymbolLayer>);

impl SymbolView {
    /// A view of `size` pixels centered on `center` at `zoom`, of `tiles`
    /// laid out by the visible symbol layers on `source` in `style`, for a
    /// continuous map at pixel ratio 1.
    pub fn new(
        style: &Style,
        source: &str,
        tiles: &[&LaidOut],
        center: LatLng,
        zoom: f64,
        size: (u32, u32),
    ) -> Result<SymbolView, Error> {
        PlacementFrame::new(style, source, tiles, center, zoom, size).map(SymbolView)
    }

    /// Adds the view's symbol buckets to `index`, as a renderer does before
    /// it places them: each symbol takes the id of the same symbol in a
    /// tile of another zoom that `index` holds, or a new one, and `index`
    /// drops the buckets of each layer that the view no longer has. Returns
    /// how many layers added a bucket.
    pub fn index(&mut self, index: &mut SymbolIndex) -> usize {
        self.0.with_context(|cx| {
            let view = &cx.update_parameters.view;
            let longitude = view.center(WrapMode::Unwrapped).longitude() as f32;
            let mut layers = Vec::new();
            LayerBuckets::fill(&mut layers, cx);
            let mut added = 0;
            for layer in leaders(&layers).rev() {
                let placed = &layer.placement_data;
                if index.0.index_layer(&layer.id, placed, longitude, view, &mut cx.buckets).buckets_added {
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
        self.0.with_context(|cx| participant.prepare_inputs(cx));
        // The placement it replaces showed nothing, so it has nothing to
        // fade from.
        self.0.place(&mut participant);
        participant.placement.shown()
    }
}

/// The renderer's cross-tile symbol index: the symbols of the tiles that
/// it shows, by layer, with the ids that keep a symbol the same across
/// tiles of different zooms.
#[derive(Default)]
pub struct SymbolIndex(CrossTileSymbolIndex);
