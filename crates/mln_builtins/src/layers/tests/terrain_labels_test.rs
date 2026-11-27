//! Labels along lines over terrain: their glyphs draw where placement
//! stands their collision circles.

use std::sync::Arc;

use glam::Vec2;
use mln_engine::camera::{TileProjector, TransformState};
use mln_engine::map::MapMode;
use mln_engine::placement::{CollisionGroup, CollisionIndex, Overlap, Shape};
use mln_engine::query::IndexedSubfeature;
use mln_engine::tile::OverscaledTileID;
use mln_style::bitmap::PremultipliedImage;
use mln_style::constants::{EXTENT, TILE_SIZE_D};
use mln_style::geo::LatLng;
use mln_style::layers::symbol_layer_properties::SymbolLayoutProperties;
use mln_style::size::Size;
use mln_style::tile_id::CanonicalTileID;
use mln_style::tileset::RasterEncoding;

use crate::layers::symbol::bucket::{PlacedSymbol, PlacedSymbolPools, SymbolSizeBinder};
use crate::layers::symbol::instance::WritingMode;
use crate::layers::symbol::placement::collision::{self, LabelRun, PartCollision};
use crate::layers::symbol::placement::collision_feature::{CollisionBox, CollisionFeature};
use crate::layers::symbol::pool::PoolRange;
use crate::layers::symbol::projection::{
    LabelPlaneProjector, LineLabelAlignment, SymbolHeights, reproject_line_labels,
};

/// A DEM tile: 16 pixels a side, rising 7 m a pixel eastward and 3 m
/// southward from 100 m, in Terrarium.
fn terrarium_slope() -> PremultipliedImage {
    let mut image = PremultipliedImage::new(Size::new(16, 16));
    for (i, pixel) in image.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = ((i % 16) as u16, (i / 16) as u16);
        let [hi, lo] = (100 + 7 * x + 3 * y + 32768).to_be_bytes();
        *pixel = [hi, lo, 0, 255];
    }
    image
}

/// Ridges across a DEM tile: 16 px a side, at 100 m but for three columns
/// of every eight, 60 m higher.
fn terrarium_ridges() -> PremultipliedImage {
    let mut image = PremultipliedImage::new(Size::new(16, 16));
    for (i, pixel) in image.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let ridge = if (3..6).contains(&(i % 16 % 8)) { 60 } else { 0 };
        let [hi, lo] = (100 + ridge + 32768u16).to_be_bytes();
        *pixel = [hi, lo, 0, 255];
    }
    image
}

/// A label along a line pitched with the map, over steep ground, draws each
/// glyph on its path, and placement stands each of its collision circles on
/// the same path: every glyph draws inside a circle, where the label
/// collides and a query finds it.
#[test]
fn a_pitched_line_label_draws_its_glyphs_where_its_circles_collide() {
    assert_glyphs_draw_in_circles(terrarium_slope, true);
}

/// A label along a line, aligned with the viewport or pitched with the map,
/// over ridges between its line's vertices, lays its glyphs out on its path,
/// straight from vertex to vertex, and its collision circles stand on the
/// same path rather than on the ridges: every glyph draws inside a circle.
#[test]
fn a_line_label_over_ridges_draws_its_glyphs_where_its_circles_collide() {
    assert_glyphs_draw_in_circles(terrarium_ridges, false);
    assert_glyphs_draw_in_circles(terrarium_ridges, true);
}

/// Places a 480 px label along a line across a tile, over the ground of
/// `dem` raised 100 times, aligned with the viewport or pitched with the map
/// (`pitch_with_map`), and checks that each glyph draws inside one of its
/// placed collision circles.
fn assert_glyphs_draw_in_circles(dem: fn() -> PremultipliedImage, pitch_with_map: bool) {
    // A 1024 px view at zoom 10.5 over 45° N, 10° E, pitched 60°, and the
    // terrain tile under its center, which reads a quarter of its parent's
    // DEM tile.
    let mut state = TransformState::default();
    state.set_size(Size::new(1024, 1024));
    state.set_lat_lng_zoom(&LatLng::new(45.0, 10.0).unwrap(), 10.5);
    state.set_pitch(60f64.to_radians());
    let view = state.view().clone();
    let tile = OverscaledTileID::new(10, 0, CanonicalTileID::new(10, 540, 368));
    let dem_tile = OverscaledTileID::new(9, 0, CanonicalTileID::new(9, 270, 184));
    let ground = mln_engine::bench::ground(tile, dem_tile, &dem(), RasterEncoding::Terrarium, 100.0).unwrap();
    let unwrapped = tile.to_unwrapped();
    let zoom = view.zoom() as f32;
    let layout = SymbolLayoutProperties::PossiblyEvaluated::default();
    let heights = SymbolHeights::new(Some(&ground), unwrapped, &layout);
    let projector = TileProjector::new(&view, &unwrapped);

    // An east-west line across the tile's middle, up the DEM's slope, and a
    // 24 px label of 41 glyphs, 480 px long, at its middle.
    let (y, anchor) = (f32::from(EXTENT as i16) / 2.0, Vec2::splat(EXTENT as f32 / 2.0));
    let line = [geometry::Point::new(200i16, y as i16), geometry::Point::new(EXTENT as i16 - 200, y as i16)];
    let glyph_offsets: Vec<f32> = (-20..=20).map(|i| i as f32 * 12.0).collect();
    let mut pools = PlacedSymbolPools::default();
    let symbol = PlacedSymbol {
        anchor_point: anchor,
        segment: 0,
        lower_size: 24.0,
        upper_size: 24.0,
        line_offset: [0.0; 2],
        writing_modes: WritingMode::Horizontal,
        line: PoolRange::extend(&mut pools.lines, line.iter().copied()),
        tile_distances: PoolRange::extend(
            &mut pools.tile_distances,
            line.iter().map(|p| Vec2::new(f32::from(p.x), f32::from(p.y)).distance(anchor)),
        ),
        glyph_offsets: PoolRange::extend(&mut pools.glyph_offsets, glyph_offsets.iter().copied()),
        start_index: 0,
        angle: 0.0,
        height_offset: 0.0,
        hidden: false,
    };

    // The glyphs as they draw, in the viewport.
    let alignment = LineLabelAlignment { pitch_with_map, rotate_with_map: true, keep_upright: false };
    let size = SymbolSizeBinder::Constant { layout_size: 24.0, covering_ranges: None, expression: None };
    let mut vertices = Vec::new();
    reproject_line_labels(
        &mut vertices,
        std::slice::from_ref(&symbol),
        &pools,
        &projector,
        heights,
        &size,
        Vec2::ZERO,
        alignment,
    );
    let pixels_to_tile_units = unwrapped.pixels_to_tile_units(1.0, zoom);
    let label_plane =
        LabelPlaneProjector::new(&projector, heights, pitch_with_map, true, pixels_to_tile_units, Vec2::ZERO);
    let index = CollisionIndex::new(&view, MapMode::Static);
    let padding = index.viewport_padding();
    let viewport = Vec2::new(view.size().width as f32, view.size().height as f32);
    let glyphs: Vec<Vec2> = vertices
        .iter()
        .step_by(4)
        .map(|vertex| {
            let [x, y, _, height] = vertex.a1;
            let clip = label_plane.to_clip_space(Vec2::new(x, y), height);
            (clip * Vec2::new(0.5, -0.5) + 0.5) * viewport + padding
        })
        .collect();
    assert_eq!(glyph_offsets.len(), glyphs.len(), "every glyph draws");
    let span = glyphs.first().unwrap().distance(*glyphs.last().unwrap());
    assert!(span > 300.0, "the label spans {span} px");

    // Its collision circles, a radius apart along the line, as placement
    // places them.
    let radius = 12.0 * 16.0;
    let count = ((EXTENT as f32 - 400.0) / radius) as usize;
    let circles: Vec<CollisionBox> = (0..count)
        .map(|i| 200.0 + i as f32 * radius)
        .map(|x| CollisionBox {
            anchor: Vec2::new(x, y),
            x1: -radius,
            y1: -radius,
            x2: radius,
            y2: radius,
            signed_distance_from_anchor: x - anchor.x,
        })
        .collect();
    let pixel_ratio = (TILE_SIZE_D * f64::from(tile.overscale_factor()) / f64::from(EXTENT)) as f32;
    let part = PartCollision {
        tile_projector: &projector,
        label_plane: &label_plane,
        heights,
        translation: Vec2::ZERO,
        pixel_ratio,
        scale: (f64::from(zoom) - f64::from(tile.overscaled_z)).exp2() as f32,
        pitch_factor: (view.pitch().cos() * view.camera_to_center_distance()) as f32,
        overlap_mode: Overlap::Always,
        pitch_with_map,
        rotate_with_map: true,
        avoid_edges: None,
        collision_group: CollisionGroup::ALL,
    };
    let feature = CollisionFeature {
        boxes: PoolRange::default(),
        indexed_feature: IndexedSubfeature {
            index: 0,
            source_layer_name: Arc::from(""),
            bucket_leader_id: Arc::from(""),
            sort_index: 0,
            bucket_instance_id: 0,
            collision_group_id: 0,
            overlap_mode: Overlap::Always,
        },
        along_line: true,
    };
    let run = LabelRun { symbol: &symbol, pools: &pools, font_size: 24.0 };
    let mut shapes = Vec::new();
    collision::place_feature(&index, &part, &feature, &circles, &run, None, &mut shapes);
    let placed: Vec<(Vec2, f32)> = shapes
        .iter()
        .filter_map(|shape| match shape {
            Some(Shape::Circle { center, radius }) => Some((*center, *radius)),
            _ => None,
        })
        .collect();
    assert!(placed.len() > 4, "the label's circles: {placed:?}");
    let pitch = if pitch_with_map { "pitched with the map" } else { "aligned with the viewport" };
    for (i, glyph) in glyphs.iter().enumerate() {
        let nearest =
            placed.iter().map(|(center, radius)| center.distance(*glyph) - radius).fold(f32::INFINITY, f32::min);
        assert!(nearest <= 0.0, "{pitch}: glyph {i} at {glyph} is {nearest} px outside every circle: {placed:?}");
    }
}
