//! GeoJSON as a GeoJSON source takes it: its text parsed, and the data that
//! the source cuts its tiles from built and cut.

use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use mln_style::sources::geojson::{GeoJSONOptions, create_geojson_data};
use mln_style::tile_id::CanonicalTileID;

use crate::scenario::{Bench, Kind, Scenario};
use crate::support::fixtures::Fixtures;

pub(crate) const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "micro/geojson/parse/small",
        kind: Kind::Micro,
        question: "What does parsing GeoJSON text of 1.5 MB cost?",
        run: parse_small,
    },
    Scenario {
        name: "micro/geojson/parse/large",
        kind: Kind::Micro,
        question: "What does parsing GeoJSON text of 13 MB cost?",
        run: parse_large,
    },
    Scenario {
        name: "micro/geojson/cut/small",
        kind: Kind::Micro,
        question: "What does cutting the world's tiles of zooms 0 to 4 from 33 countries' borders cost?",
        run: cut_small,
    },
    Scenario {
        name: "micro/geojson/cut/large",
        kind: Kind::Micro,
        question: "What does cutting the world's tiles of zooms 0 to 4 from 258 countries' borders cost?",
        run: cut_large,
    },
];

const COUNTRIES: &str = "geojson/ne_10m_admin_0_countries.geojson";

/// The small input keeps every eighth of the countries.
const SMALL: usize = 8;

/// Natural Earth's countries at 1:10m, or every `every`th of them, as
/// GeoJSON text.
fn countries(fixtures: &Fixtures, every: usize) -> Vec<u8> {
    let text = fixtures.read(COUNTRIES);
    if every == 1 {
        return text;
    }
    let mut collection: serde_json::Value = serde_json::from_slice(&text).expect("the countries are JSON");
    let features = collection["features"].as_array().expect("a feature collection").iter().step_by(every).cloned();
    collection["features"] = features.collect::<Vec<_>>().into();
    serde_json::to_vec(&collection).expect("JSON")
}

/// Every eighth of Natural Earth's countries, their borders at 1:10m, as
/// the text an application gives a GeoJSON source: parsed into GeoJSON
/// values, as the runtime does before the source builds its data.
fn parse_small(bench: &mut Bench) {
    parse(bench, SMALL);
}

/// [`parse_small`] with every country.
fn parse_large(bench: &mut Bench) {
    parse(bench, 1);
}

fn parse(bench: &mut Bench, every: usize) {
    let fixtures = Fixtures::open(bench);
    let text = String::from_utf8(countries(&fixtures, every)).expect("UTF-8");
    let geojson = bench.measure(|| mln_runtime::bench::parse_geojson(black_box(&text)).expect("GeoJSON"));
    drop(black_box(geojson));
    bench.per("kb", (text.len() / 1024) as u64);
}

/// The tiles cut: every tile of zooms 0 to 4, those that a view of the
/// whole world asks a source for.
const MAX_ZOOM: u8 = 4;

/// Every eighth of Natural Earth's countries, as GeoJSON values: the data a
/// GeoJSON source with the default options builds from them (the features
/// converted, projected, simplified and indexed), and every tile of zooms 0
/// to 4 cut from it, as the source's tiles ask for them.
fn cut_small(bench: &mut Bench) {
    cut(bench, SMALL);
}

/// [`cut_small`] with every country.
fn cut_large(bench: &mut Bench) {
    cut(bench, 1);
}

fn cut(bench: &mut Bench, every: usize) {
    let fixtures = Fixtures::open(bench);
    let text = countries(&fixtures, every);
    let text = String::from_utf8(text).expect("UTF-8");
    let geojson = mln_runtime::bench::parse_geojson(&text).expect("GeoJSON");
    let options = GeoJSONOptions::default_options();
    let tiles: Vec<CanonicalTileID> = (0..=MAX_ZOOM)
        .flat_map(|z| (0..1u32 << z).flat_map(move |x| (0..1u32 << z).map(move |y| CanonicalTileID::new(z, x, y))))
        .collect();
    let features = Arc::new(AtomicUsize::new(0));
    let data = bench.measure(|| {
        let data = create_geojson_data(geojson, &options).expect("GeoJSON data");
        for tile in &tiles {
            let tile = data.get_tile(tile);
            features.fetch_add(tile.len(), Ordering::Relaxed);
            drop(black_box(tile));
        }
        data
    });
    assert!(features.load(Ordering::Relaxed) > tiles.len(), "the tiles hold the countries");
    drop(data);
    bench.per("kb", (text.len() / 1024) as u64);
}
