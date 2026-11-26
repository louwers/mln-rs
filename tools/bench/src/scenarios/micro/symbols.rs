//! A street tile's labels and icons in a view, as a renderer finds which
//! to show: the cross-tile index, which gives each symbol an id that holds
//! across zooms, and the placement, which shows each symbol where it
//! collides with none shown before it.
//!
//! The view is a map at zoom 15 that shows one whole street tile, 1,024
//! pixels across: the street tiles stop at zoom 14, so the map shows that
//! tile laid out at zoom 15, with the symbol layers that start there.
//!
//! Both stages are the renderer's, reached through its `bench` entry points
//! (`mln_engine::bench`).

use std::sync::Arc;

use mln_engine::UpdateParameters;
use mln_engine::bench::{LaidOut, SymbolIndex, SymbolView};
use mln_map::{MapMode, TileLodMode, TransformState};
use mln_style::Style;
use mln_style::size::Size;
use mln_style::time::Instant;

use super::{DENSE, SPARSE, StreetLayout, StreetTile, loaded_liberty};
use crate::scenario::{Bench, Kind, Scenario};
use crate::support::fixtures::Fixtures;
use crate::support::map::tile_center;

pub(crate) const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "micro/symbols/index/sparse",
        kind: Kind::Micro,
        question: "What does indexing a sparse tile's symbols across zooms cost, as the view zooms in on them?",
        run: index_sparse,
    },
    Scenario {
        name: "micro/symbols/index/dense",
        kind: Kind::Micro,
        question: "What does indexing a dense tile's symbols across zooms cost, as the view zooms in on them?",
        run: index_dense,
    },
    Scenario {
        name: "micro/symbols/place/sparse",
        kind: Kind::Micro,
        question: "What does placing a sparse tile's labels and icons cost, each where it collides with none shown?",
        run: place_sparse,
    },
    Scenario {
        name: "micro/symbols/place/dense",
        kind: Kind::Micro,
        question: "What does placing a dense tile's labels and icons cost, each where it collides with none shown?",
        run: place_dense,
    },
];

/// The zoom of the view.
const ZOOM: u8 = 15;

/// The view's size: the street tile, 512 pixels at its zoom, at twice its
/// size.
const VIEW: (u32, u32) = (1024, 1024);

/// The sparse street tile ([`SPARSE`]) as the view zooms in from 14 to 15:
/// the cross-tile index holds the tile's symbols laid out at zoom 14, and
/// takes them laid out at zoom 15. Each symbol at 15 takes the id of the
/// same symbol at 14 if there is one, or a new one, and the symbols at 14
/// drop out. Each run indexes both, from an empty index.
fn index_sparse(bench: &mut Bench) {
    index(bench, SPARSE);
}

/// [`index_sparse`] for the dense tile ([`DENSE`]).
fn index_dense(bench: &mut Bench) {
    index(bench, DENSE);
}

fn index(bench: &mut Bench, tile: StreetTile) {
    let fixtures = Fixtures::open(bench);
    let style = loaded_liberty(&fixtures);
    let before = lay_out(&fixtures, &style, tile, ZOOM - 1);
    let after = lay_out(&fixtures, &style, tile, ZOOM);
    let zoomed_out = view(&style, &before, tile, ZOOM - 1);
    let zoomed_in = view(&style, &after, tile, ZOOM);

    let added = bench.measure(|| {
        (0..tile.repeats)
            .map(|_| {
                let mut index = SymbolIndex::default();
                zoomed_out.index(&mut index).unwrap_or_else(|error| panic!("{error}"));
                zoomed_in.index(&mut index).unwrap_or_else(|error| panic!("{error}"))
            })
            .sum::<usize>()
    });
    assert!(added > 0, "the tile has symbols");
    bench.per("tile", tile.repeats);
}

/// The sparse street tile's symbols, indexed, placed in the view, topmost
/// layer first, after a placement that showed none, as a renderer places
/// them when the view changes: each where its text and its icon collide
/// with none placed before it, and the placement records how each fades.
fn place_sparse(bench: &mut Bench) {
    place(bench, SPARSE);
}

/// [`place_sparse`] for the dense tile.
fn place_dense(bench: &mut Bench) {
    place(bench, DENSE);
}

/// The placements of a tile for each time the layout scenarios lay it out.
/// A placement's cost varies a little from one to the next, more than a
/// layout's, so a run averages over more of them.
const PLACEMENTS: u64 = 4;

fn place(bench: &mut Bench, tile: StreetTile) {
    let fixtures = Fixtures::open(bench);
    let style = loaded_liberty(&fixtures);
    let symbols = lay_out(&fixtures, &style, tile, ZOOM);
    let view = view(&style, &symbols, tile, ZOOM);
    view.index(&mut SymbolIndex::default()).unwrap_or_else(|error| panic!("{error}"));
    // The first placement marks the buckets as placed, once.
    let shown = view.place();
    assert!(shown > 0, "the tile shows symbols");

    let placements = tile.repeats * PLACEMENTS;
    let placed = bench.measure(|| (0..placements).map(|_| view.place()).collect::<Vec<_>>());
    assert!(placed.iter().all(|&placed| placed == shown), "each placement shows the same symbols");
    bench.per("tile", placements);
}

/// `tile` laid out by `style`'s symbol layers, shown at `zoom`.
fn lay_out(fixtures: &Fixtures, style: &Style, tile: StreetTile, zoom: u8) -> LaidOut {
    StreetLayout::shown_at(fixtures, style, |type_| type_ == "symbol", tile.id, zoom).run()
}

/// The view of `symbols`, at `zoom`, centered on `tile`.
fn view(style: &Style, symbols: &LaidOut, tile: StreetTile, zoom: u8) -> SymbolView {
    let (z, x, y) = tile.id;
    let mut state = TransformState::default();
    state.set_size(Size::new(VIEW.0, VIEW.1));
    state.set_lat_lng_zoom(&tile_center(z, x, y), f64::from(zoom));
    SymbolView::new("openmaptiles", &[symbols], update(style, &state)).unwrap_or_else(|error| panic!("{error}"))
}

/// The update that a continuous map at pixel ratio 1 with `style` loaded
/// gives its renderer for `state`'s view.
fn update(style: &Style, state: &TransformState) -> UpdateParameters {
    UpdateParameters {
        style_loaded: true,
        mode: MapMode::Continuous,
        pixel_ratio: 1.0,
        time_point: Some(Instant::now()),
        view: state.view().clone(),
        min_zoom: state.min_zoom(),
        max_zoom: state.limits().max_zoom,
        camera_moving: false,
        style: Arc::new(style.clone()),
        font_faces: None,
        sprite_loaded: true,
        feature_states: Arc::default(),
        file_source: None,
        prefetch_zoom_delta: 0,
        still_image_request: false,
        still_image_request_id: 0,
        update_id: 0,
        cross_source_collisions: true,
        fast_pfor_enabled: false,
        terrain: mln_map::TerrainOptions::default(),
        elevation: None,
        tile_lod_min_radius: 3.0,
        tile_lod_scale: 1.0,
        tile_lod_pitch_threshold: 60.0 * std::f64::consts::PI / 180.0,
        tile_lod_zoom_shift: 0.0,
        tile_lod_mode: TileLodMode::Default,
        subdivision_granularity: style.get_projection().get_subdivision_granularity(),
    }
}
