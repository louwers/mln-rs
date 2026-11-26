//! Text, as the labels of a city written in Japanese script need it: the
//! glyph ranges of a style's font parsed, the glyphs that a font face draws
//! turned into signed distance fields, and labels shaped into lines of
//! positioned glyphs.
//!
//! The scenarios read the labels of a tile of the Tokyo tiles as Liberty
//! writes them: a place's or a road's name in Latin script, then in
//! Japanese, on a line of its own for a place.

use std::collections::BTreeSet;
use std::hint::black_box;
use std::num::NonZeroU16;

use feature::Value;
use mln_engine::text::atlas::ImagePositions;
use mln_engine::text::glyph::{GlyphID, GlyphIDType, GlyphPosition, GlyphPositions, GlyphRange, get_glyph_range};
use mln_engine::text::glyph_pbf::parse_glyph_pbf;
use mln_engine::text::tagged_string::font_stack_hash;
use mln_engine::text::{SectionOptions, ShapedText, Shaper, ShapingOptions, TaggedString, shape_text};
use mln_style::feature::GeometryTileFeature;
use mln_style::types::{SymbolAnchorType, TextJustifyType, TextWritingModeType};
use mln_util::constants::ONE_EM;
use mln_util::tiny_sdf::transform_raster_to_sdf;

use super::{tile_bytes, tile_data};
use crate::scenario::{Bench, Kind, Scenario};
use crate::support::fixtures::{Fixtures, Tiles};

pub(crate) const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "micro/text/glyph-ranges/small",
        kind: Kind::Micro,
        question: "What does parsing 7 glyph ranges, those of a tile's labels in Latin script, cost?",
        run: glyph_ranges_small,
    },
    Scenario {
        name: "micro/text/glyph-ranges/large",
        kind: Kind::Micro,
        question: "What does parsing 99 glyph ranges, those of a tile's labels in Japanese script, cost?",
        run: glyph_ranges_large,
    },
    Scenario {
        name: "micro/text/glyph-sdf/sparse",
        kind: Kind::Micro,
        question: "What does drawing a sparse tile's Japanese glyphs from a font, as distance fields, cost?",
        run: glyph_sdf_sparse,
    },
    Scenario {
        name: "micro/text/glyph-sdf/dense",
        kind: Kind::Micro,
        question: "What does drawing a dense tile's Japanese glyphs from a font, as distance fields, cost?",
        run: glyph_sdf_dense,
    },
    Scenario {
        name: "micro/text/shape/sparse",
        kind: Kind::Micro,
        question: "What does shaping a sparse tile's labels into lines of glyphs cost?",
        run: shape_sparse,
    },
    Scenario {
        name: "micro/text/shape/dense",
        kind: Kind::Micro,
        question: "What does shaping a dense tile's labels into lines of glyphs cost?",
        run: shape_dense,
    },
];

/// A tile whose labels a text scenario reads.
#[derive(Clone, Copy)]
struct LabelTile {
    tiles: Tiles,
    id: (u8, u32, u32),
}

/// A tile of a city's overview: 12/3638/1612, central and eastern Tokyo at
/// zoom 12, whose 518 labels name its districts, stations, rivers and main
/// roads, with 357 Japanese glyphs.
const SPARSE: LabelTile = LabelTile { tiles: Tiles::Cjk, id: (12, 3638, 1612) };

/// The Tokyo tile with the most labels: 14/14553/6450, Akihabara and Ueno,
/// whose 12,371 labels are mostly the names of shops, restaurants and
/// other points of interest, with 1,772 Japanese glyphs.
const DENSE: LabelTile = LabelTile { tiles: Tiles::Cjk, id: (14, 14553, 6450) };

/// The street tile with the most features ([`super::DENSE`]), whose 3,093
/// labels are in Latin script.
const LATIN: LabelTile = LabelTile { tiles: Tiles::Street, id: super::DENSE.id };

/// The source layers whose features Liberty labels, and what its text
/// puts between a name's Latin and Japanese forms: a line break for a point,
/// a space along a line.
const LABELLED: [(&str, &str); 4] =
    [("place", "\n"), ("poi", "\n"), ("water_name", "\n"), ("transportation_name", " ")];

/// The font stack of Liberty's labels.
const FONT: &str = "Noto Sans Regular";

/// A label, as characters, and its name in Japanese script alone, when it
/// has one.
struct Label {
    text: Vec<char>,
    japanese: Option<Vec<char>>,
}

/// The labels of `tile`'s features, as Liberty's text writes them: a name
/// in Latin and Japanese script has both, the Latin first; another has its
/// name.
fn labels(fixtures: &Fixtures, tile: LabelTile) -> Vec<Label> {
    let data = tile_data(tile.tiles, &tile_bytes(fixtures, tile.tiles, tile.id));
    let string = |value: Option<Value>| match value {
        Some(Value::String(text)) => Some(text),
        _ => None,
    };
    let mut labels = Vec::new();
    for (name, separator) in LABELLED {
        let Some(layer) = data.layer(name).expect("a tile that decodes") else { continue };
        for index in 0..layer.len() {
            let feature = layer.feature(index).expect("a feature that decodes");
            let japanese = string(feature.get_value("name:nonlatin"));
            let text = match (&japanese, string(feature.get_value("name:latin"))) {
                (Some(japanese), Some(latin)) => format!("{latin}{separator}{japanese}"),
                _ => match string(feature.get_value("name_en")).or_else(|| string(feature.get_value("name"))) {
                    Some(name) => name,
                    None => continue,
                },
            };
            labels.push(Label { text: text.chars().collect(), japanese: japanese.map(|text| text.chars().collect()) });
        }
    }
    labels
}

/// The glyph ranges of [`FONT`] that `labels` need.
fn glyph_ranges(labels: &[Label]) -> BTreeSet<GlyphRange> {
    labels.iter().flat_map(|label| &label.text).filter_map(|&c| GlyphID::from_char(c)).map(get_glyph_range).collect()
}

/// The snapshot's path of `range` of [`FONT`].
fn range_path(range: GlyphRange) -> String {
    format!("glyphs/{FONT}/{}-{}.pbf", range.first, range.second)
}

/// The glyph ranges of the style's font that the labels of the densest
/// street tile need ([`LATIN`]), 7 ranges of Latin script and punctuation,
/// 0.9 MB: their glyph PBFs parsed into glyphs, each with its metrics and
/// its distance field, as a renderer parses each range that a tile's labels
/// ask for.
fn glyph_ranges_small(bench: &mut Bench) {
    parse_glyph_ranges(bench, LATIN, 20);
}

/// [`glyph_ranges_small`] for the labels of the densest Tokyo tile
/// ([`DENSE`]): 99 ranges, most of them of Japanese script, 17 MB.
fn glyph_ranges_large(bench: &mut Bench) {
    parse_glyph_ranges(bench, DENSE, 1);
}

fn parse_glyph_ranges(bench: &mut Bench, tile: LabelTile, repeats: u64) {
    let fixtures = Fixtures::open(bench);
    let ranges: Vec<(GlyphRange, Vec<u8>)> = glyph_ranges(&labels(&fixtures, tile))
        .into_iter()
        .map(|range| (range, fixtures.read(&range_path(range))))
        .collect();
    let bytes: usize = ranges.iter().map(|(_, pbf)| pbf.len()).sum();
    for _ in 0..repeats {
        let glyphs = bench.measure(|| {
            ranges.iter().map(|(range, pbf)| parse_glyph_pbf(range, pbf).expect("a glyph PBF").len()).sum::<usize>()
        });
        assert!(glyphs > 0, "the ranges have glyphs");
    }
    bench.per("kb", repeats * (bytes / 1024) as u64);
}

/// The Japanese glyphs of the labels of a sparse tile of the Tokyo tiles
/// ([`SPARSE`]), as a style whose font faces cover Japanese script draws
/// them: each name in Japanese script shaped with Noto Sans JP to find its
/// glyphs, then each glyph drawn once from the font's outline, as a bitmap
/// of its coverage, and turned into a signed distance field. Only the
/// drawing is measured.
fn glyph_sdf_sparse(bench: &mut Bench) {
    glyph_sdf(bench, SPARSE, 4);
}

/// [`glyph_sdf_sparse`] for the dense tile ([`DENSE`]).
fn glyph_sdf_dense(bench: &mut Bench) {
    glyph_sdf(bench, DENSE, 1);
}

fn glyph_sdf(bench: &mut Bench, tile: LabelTile, repeats: u64) {
    let fixtures = Fixtures::open(bench);
    let face = GlyphIDType::face(NonZeroU16::MIN);
    let shaper = Shaper::new(face, fixtures.read("fonts/NotoSansJP-Regular.otf"));
    assert!(shaper.valid(), "Noto Sans JP opens");
    let mut glyphs = BTreeSet::new();
    for japanese in labels(&fixtures, tile).iter().filter_map(|label| label.japanese.as_ref()) {
        let mut shaped = ShapedText::default();
        shaper.shape(japanese, false, &mut shaped);
        glyphs.extend(shaped.glyphs);
    }
    for _ in 0..repeats {
        bench.measure(|| {
            for &glyph in &glyphs {
                let drawn = shaper.rasterize_glyph(glyph);
                black_box(transform_raster_to_sdf(&drawn.bitmap, 8.0, 0.25));
            }
        });
    }
    bench.per("glyph", repeats * glyphs.len() as u64);
}

/// The labels of a sparse tile of the Tokyo tiles ([`SPARSE`]), each
/// shaped as Liberty's labels of points of interest are: their text split
/// into lines no wider than 9 ems, ordered for display, each glyph placed
/// from its advance, the lines centered under the point. The glyphs'
/// metrics come from the style's glyph ranges, parsed beforehand.
fn shape_sparse(bench: &mut Bench) {
    shape(bench, SPARSE, 10);
}

/// [`shape_sparse`] for the dense tile ([`DENSE`]).
fn shape_dense(bench: &mut Bench) {
    shape(bench, DENSE, 1);
}

/// Liberty's text size of points of interest, in pixels.
const TEXT_SIZE: f32 = 12.0;

fn shape(bench: &mut Bench, tile: LabelTile, repeats: u64) {
    let fixtures = Fixtures::open(bench);
    let labels = labels(&fixtures, tile);
    let font_stack = vec![FONT.to_owned()];
    let mut positions = GlyphPositions::new();
    let font_positions = positions.entry(font_stack_hash(&font_stack)).or_default();
    for range in glyph_ranges(&labels) {
        for glyph in parse_glyph_pbf(&range, &fixtures.read(&range_path(range))).expect("a glyph PBF") {
            font_positions.insert(glyph.id, Some(GlyphPosition { metrics: glyph.metrics, ..GlyphPosition::default() }));
        }
    }
    let section = SectionOptions::new(1.0, font_stack, GlyphIDType::FontPBF, None);
    let texts: Vec<TaggedString> =
        labels.iter().map(|label| TaggedString::new(label.text.clone(), section.clone())).collect();
    let images = ImagePositions::default();
    let options = ShapingOptions {
        max_width: 9.0 * ONE_EM,
        line_height: 1.2 * ONE_EM,
        anchor: SymbolAnchorType::Top,
        justify: TextJustifyType::Center,
        spacing: 0.0,
        offset: [0.0, 0.6 * ONE_EM],
        writing_mode: TextWritingModeType::Horizontal,
        allow_vertical_placement: false,
        text_size: TEXT_SIZE,
        text_size_at_bucket_zoom: TEXT_SIZE,
    };

    for _ in 0..repeats {
        let lines = bench.measure(|| {
            texts.iter().map(|text| shape_text(text, &options, &positions, &images).lines.len()).sum::<usize>()
        });
        assert!(lines >= texts.len(), "each label has a line");
    }
    bench.per("label", repeats * texts.len() as u64);
}
