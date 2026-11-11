// Ports test/text/line_label_flip.test.cpp.

use std::any::Any;
use std::f32::consts::PI;
use std::sync::Arc;

use mln_gfx::shaders::segment::SegmentBase;
use mln_style::layer_properties::LayerProperties;
use mln_style::property_value::PropertyValue;
use mln_style_types::image::ImageMap;
use mln_tile_id::geometry_tile_data::GeometryCoordinates;
use mln_util::Padding;
use mln_util::geo::LatLng;
use mln_util::immutable::Immutable;
use mln_util::size::Size;

use super::*;
use crate::geometry::anchor::Anchor;
use crate::layout::symbol_instance::{ShapedTextOrientations, SymbolContent, SymbolInstanceSharedData};
use crate::layout::symbol_layout::SymbolLayout;
use crate::layout::symbol_projection::line_label_needs_flip;
use crate::paint_parameters::TransformParameters;
use crate::tile::tile::{Kind, Tile, TileBase};
use crate::tile_render_data::TileRenderData;

const MARGIN: f32 = 20.0;

/// Whether a label whose unflipped reading direction points `degrees`
/// counterclockwise from the screen's x axis (gl coordinates, y up: 90 =
/// straight up) has to be drawn flipped.
fn needs_flip_with(degrees: f32, was_flipped: Option<bool>, aspect_ratio: f32, keep_upright_margin: f32) -> bool {
    let radians = mln_math::deg2radf(degrees);
    let first = Point::new(0.0, 0.0);
    let last = Point::new(100.0 * radians.cos() / aspect_ratio, 100.0 * radians.sin());
    line_label_needs_flip(first, last, aspect_ratio, was_flipped, keep_upright_margin)
}

fn needs_flip(degrees: f32, was_flipped: Option<bool>) -> bool {
    needs_flip_with(degrees, was_flipped, 1.0, MARGIN)
}

// gtest: LineLabelFlip.FirstDrawingFlipsWhatRunsRightToLeft
#[test]
fn first_drawing_flips_what_runs_right_to_left() {
    // As without the margin, also right next to the vertical.
    assert!(!needs_flip(0.0, None));
    assert!(!needs_flip(45.0, None));
    assert!(!needs_flip(89.0, None));
    assert!(needs_flip(91.0, None));
    assert!(needs_flip(180.0, None));
    assert!(needs_flip(225.0, None));
    assert!(needs_flip(269.0, None));
    assert!(!needs_flip(271.0, None));
}

// gtest: LineLabelFlip.KeepsItsDirectionWhileTheLineStaysCloseToTheVertical
#[test]
fn keeps_its_direction_while_the_line_stays_close_to_the_vertical() {
    // Drawn unflipped, the map rotating a few degrees either way around the vertical of the line.
    for degrees in [85.0, 92.0, 95.0, 105.0, 265.0, 275.0, 255.0] {
        assert!(!needs_flip(degrees, Some(false)), "{degrees}");
    }
    // Drawn flipped: it stays flipped through the same rotations.
    for degrees in [95.0, 88.0, 85.0, 75.0, 265.0, 275.0, 285.0] {
        assert!(needs_flip(degrees, Some(true)), "{degrees}");
    }
}

// gtest: LineLabelFlip.TurnsOverOnceClearlyUpsideDown
#[test]
fn turns_over_once_clearly_upside_down() {
    assert!(needs_flip(90.0 + MARGIN + 1.0, Some(false)));
    assert!(needs_flip(270.0 - MARGIN - 1.0, Some(false)));
    assert!(!needs_flip(90.0 - MARGIN - 1.0, Some(true)));
    assert!(!needs_flip(270.0 + MARGIN + 1.0, Some(true)));
}

// gtest: LineLabelFlip.MeasuresTheMarginOnTheScreenNotInGlUnits
#[test]
fn measures_the_margin_on_the_screen_not_in_gl_units() {
    // On a tall screen one gl unit across is much shorter than one up: 15 degrees from the vertical
    // on the screen is inside the margin, even though in raw gl units it would look like 30.
    assert!(!needs_flip_with(105.0, Some(false), 0.46, MARGIN));
    assert!(needs_flip_with(105.0, Some(true), 0.46, MARGIN));
}

// gtest: LineLabelFlip.WithoutAMarginTheLastDirectionIsIgnored
#[test]
fn without_a_margin_the_last_direction_is_ignored() {
    // The default: decided anew on every drawing, whichever way the label was drawn before.
    for degrees in [0.0, 85.0, 89.0, 91.0, 95.0, 180.0, 265.0, 269.0, 271.0, 275.0] {
        let rule = needs_flip_with(degrees, None, 1.0, 0.0);
        assert_eq!(rule, degrees > 90.0 && degrees < 270.0, "{degrees}");
        assert_eq!(needs_flip_with(degrees, Some(false), 1.0, 0.0), rule, "{degrees}");
        assert_eq!(needs_flip_with(degrees, Some(true), 1.0, 0.0), rule, "{degrees}");
    }
    // Straight up: not flipped, as today, even if it was drawn flipped before.
    assert!(!line_label_needs_flip(Point::new(0.0, 0.0), Point::new(0.0, 100.0), 1.0, Some(true), 0.0));
}

struct FakeTile {
    base: TileBase,
}

impl Tile for FakeTile {
    fn base(&self) -> &TileBase {
        &self.base
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn create_render_data(&self) -> Box<dyn TileRenderData> {
        unreachable!()
    }
    fn cancel(&self) {}
    fn layer_properties_updated(&self, _layer_properties: &Immutable<dyn LayerProperties>) -> bool {
        true
    }
}

/// `TestPlacement`: a placement with a keep-upright margin, with access to
/// what it does on every frame (`update_bucket_dynamic_attribute_data`) and
/// once per placement (`sync_line_label_directions`).
fn test_placement(keep_upright_margin: f32) -> Placement {
    let mut placement = Placement::new_default();
    placement.keep_upright_margin = keep_upright_margin;
    placement
}

fn set_previous(placement: &Placement, previous: Rc<Placement>) {
    *placement.prev_placement.borrow_mut() = Some(previous);
}

fn make_line_label(cross_tile_id: u32) -> SymbolInstance {
    let shaping = ShapedTextOrientations::default();
    let layout = SymbolLayoutProperties::Evaluated::default();
    let offset = [0.0f32, 0.0];
    let image_map = ImageMap::new();
    let shared_data = Arc::new(SymbolInstanceSharedData::new(
        GeometryCoordinates::new(),
        &shaping,
        &None,
        &None,
        &layout,
        SymbolPlacementType::Line,
        &offset,
        &image_map,
        0.0,
        SymbolContent::IconRGBA,
        false,
        false,
    ));
    let anchor = Anchor::new(4096.0, 4000.0, 0.0, Some(0));
    let subfeature = IndexedSubfeature {
        index: 0,
        source_layer_name: String::new(),
        bucket_leader_id: String::new(),
        sort_index: 0,
        bucket_instance_id: 0,
        collision_group_id: 0,
    };
    let mut symbol = SymbolInstance::new(
        anchor,
        shared_data,
        &shaping,
        &None,
        &None,
        0.0,
        0.0,
        SymbolPlacementType::Line,
        offset,
        0.0,
        Padding::uniform(0.0),
        offset,
        &subfeature,
        0,
        0,
        "Main Street".encode_utf16().collect(),
        0.0,
        0.0,
        0.0,
        &None,
        false,
        SymbolContent::IconRGBA,
    );
    symbol.set_cross_tile_id(cross_tile_id);
    symbol
}

#[derive(Clone, Copy)]
enum Line {
    Up,
    Down,
    Across,
    /// Two vertices at the same place: no direction at the anchor.
    Point,
}

/// A text (three glyphs) or an icon (one) along a line through the middle of
/// its tile: straight up the tile, down, across to the right, or a point.
fn make_placed_symbol(glyph_offsets: Vec<f32>, direction: Line) -> PlacedSymbol {
    let coordinates: [(i16, i16); 2] = match direction {
        Line::Up => [(4096, 7000), (4096, 1000)],
        Line::Down => [(4096, 1000), (4096, 7000)],
        Line::Across => [(1000, 4000), (7000, 4000)],
        Line::Point => [(4096, 4000), (4096, 4000)],
    };
    let line: GeometryCoordinates = coordinates.iter().map(|&(x, y)| Point::new(x, y)).collect();
    let anchor = Anchor::new(4096.0, 4000.0, 0.0, Some(0));
    let tile_distances = SymbolLayout::calculate_tile_distances(&line, &anchor);
    let mut placed = PlacedSymbol::new(
        anchor.point,
        0,
        16.0,
        16.0,
        [0.0, 0.0],
        WritingModeType::Horizontal,
        line,
        tile_distances,
        None,
        None,
    );
    placed.glyph_offsets = glyph_offsets;
    placed
}

/// A bucket of a layer of line labels kept upright and rotated with the map:
/// one label per cross tile id, each with its text and an icon (an arrow
/// along the line, kept upright as well), and one SDF icon along the same
/// line.
fn make_bucket(cross_tile_ids: &[u32], line: Line) -> Arc<SymbolBucket> {
    let mut layout = SymbolLayoutProperties::PossiblyEvaluated::default();
    *layout.get_mut::<SymbolPlacement>() = SymbolPlacementType::Line;
    *layout.get_mut::<TextRotationAlignment>() = AlignmentType::Map;
    *layout.get_mut::<TextPitchAlignment>() = AlignmentType::Viewport;
    *layout.get_mut::<TextKeepUpright>() = true;
    *layout.get_mut::<IconRotationAlignment>() = AlignmentType::Map;
    *layout.get_mut::<IconPitchAlignment>() = AlignmentType::Viewport;
    *layout.get_mut::<IconKeepUpright>() = true;
    let symbols: Vec<SymbolInstance> = cross_tile_ids.iter().map(|&id| make_line_label(id)).collect();
    let mut bucket = SymbolBucket::new(
        Arc::new(layout),
        &VecMap::new(),
        &PropertyValue::Constant(16.0f32),
        &PropertyValue::Constant(1.0f32),
        0.0,
        false,
        false,
        "layer".to_owned(),
        symbols,
        Vec::new(),
        1.0,
        false,
        Vec::new(),
        false,
    );
    {
        let state = bucket.state_mut();
        state.text.segments.push(SegmentBase::new(0, 0));
        state.icon.segments.push(SegmentBase::new(0, 0));
        state.sdf_icon.segments.push(SegmentBase::new(0, 0));
        state.sdf_icon.placed_symbols.push(make_placed_symbol(vec![0.0], Line::Up));
        for i in 0..state.symbol_instances.len() {
            state.text.placed_symbols.push(make_placed_symbol(vec![-30.0, 0.0, 30.0], line));
            state.symbol_instances[i].placed_right_text_index = Some(i);
            state.icon.placed_symbols.push(make_placed_symbol(vec![0.0], Line::Up));
            state.symbol_instances[i].placed_icon_index = Some(i);
        }
    }
    Arc::new(bucket)
}

fn up(cross_tile_ids: &[u32]) -> Arc<SymbolBucket> {
    make_bucket(cross_tile_ids, Line::Up)
}

fn text_direction(bucket: &SymbolBucket, label: usize) -> Option<bool> {
    bucket.state().text.placed_symbols[label].keep_upright_flipped
}

fn set_text_direction(bucket: &SymbolBucket, label: usize, flipped: Option<bool>) {
    bucket.state().text.placed_symbols[label].keep_upright_flipped = flipped;
}

fn icon_direction(bucket: &SymbolBucket) -> Option<bool> {
    bucket.state().icon.placed_symbols[0].keep_upright_flipped
}

fn sdf_icon_direction(bucket: &SymbolBucket) -> Option<bool> {
    bucket.state().sdf_icon.placed_symbols[0].keep_upright_flipped
}

fn hide_text(bucket: &SymbolBucket) {
    bucket.state().text.placed_symbols[0].hidden = true;
}

fn mark_stale(bucket: &SymbolBucket) {
    bucket.state().line_label_directions_stale = true;
}

fn is_stale(bucket: &SymbolBucket) -> bool {
    bucket.state().line_label_directions_stale
}

/// The angles at which a frame drew the first glyph of the first text, the
/// first icon and the SDF icon of a bucket.
struct Angles {
    text: f32,
    icon: f32,
    sdf_icon: f32,
}

/// A map of 256 x 512 at zoom 0, showing the whole tile 0/0/0.
struct Frame {
    render_tiles: Rc<Vec<RenderTile>>,
    state: TransformState,
}

impl Frame {
    fn new() -> Self {
        let tile = FakeTile {
            base: TileBase::new(Kind::Geometry, OverscaledTileID::from_zxy(0, 0, 0), "source".to_owned(), None),
        };
        let render_tile = RenderTile::new(UnwrappedTileID::new(0, 0, 0), &tile);
        let mut state = TransformState::default();
        state.set_size(Size::new(256, 512));
        state.set_lat_lng_zoom(&LatLng::new(0.0, 0.0).unwrap(), 0.0);
        let mut frame = Frame { render_tiles: Rc::new(vec![render_tile]), state };
        frame.set_bearing(0.0);
        frame
    }

    /// The tile's matrix at this bearing, in degrees.
    fn set_bearing(&mut self, degrees: f64) {
        self.state.set_bearing(mln_math::deg2rad(-degrees));
        let render_tile = &mut Rc::get_mut(&mut self.render_tiles).unwrap()[0];
        self.state.matrix_for(&mut render_tile.matrix, &render_tile.id);
        let tile_matrix = render_tile.matrix;
        matrix::multiply(&mut render_tile.matrix, &TransformParameters::new(&self.state).proj_matrix, &tile_matrix);
    }

    /// Draws the bucket as on every frame.
    fn draw(&self, placement: &Placement, bucket: &SymbolBucket) -> Angles {
        placement.update_bucket_dynamic_attribute_data(bucket, &self.state, &self.render_tiles[0]);
        let state = bucket.state();
        Angles {
            text: state.text.dynamic_attribute_data[0].a1[2],
            icon: state.icon.dynamic_attribute_data[0].a1[2],
            sdf_icon: state.sdf_icon.dynamic_attribute_data[0].a1[2],
        }
    }

    /// What Placement does with a layer whenever it updates the opacities.
    fn sync_layer(&self, placement: &Placement, buckets: &[&Arc<SymbolBucket>], layer_id: &str) {
        let data: LayerPlacementData = buckets
            .iter()
            .map(|bucket| BucketPlacementData {
                bucket: Arc::clone(*bucket) as Arc<dyn Bucket>,
                tile_owner: self.render_tiles.clone(),
                tile_index: 0,
                feature_index: None,
                source_id: "source".to_owned(),
                sort_key_range: None,
            })
            .collect();
        placement.sync_line_label_directions(layer_id, &data);
    }

    fn sync(&self, placement: &Placement, buckets: &[&Arc<SymbolBucket>]) {
        self.sync_layer(placement, buckets, "layer");
    }
}

/// Whether a glyph drawn at `angle` and later at `later_angle` was turned
/// over in between.
fn turned_over(angle: f32, later_angle: f32) -> bool {
    let difference = (later_angle - angle).abs() % (2.0 * PI);
    difference.min(2.0 * PI - difference) > PI / 2.0
}

// gtest: LineLabelFlip.WithAMarginTheTextKeepsItsDirectionButIconsDoNot
#[test]
fn with_a_margin_the_text_keeps_its_direction_but_icons_do_not() {
    let placement = test_placement(MARGIN);
    let bucket = up(&[1]);
    let mut frame = Frame::new();
    frame.set_bearing(-6.0);
    let first = frame.draw(&placement, &bucket);
    let first_direction = text_direction(&bucket, 0).expect("drawn");

    // The map turns from 6 degrees one side of the line to 6 degrees the other side.
    frame.set_bearing(6.0);
    let second = frame.draw(&placement, &bucket);
    assert!(!turned_over(first.text, second.text));
    assert_eq!(text_direction(&bucket, 0), Some(first_direction));
    // Icons, kept upright too, turn over as without the margin: nothing is remembered for them.
    assert!(turned_over(first.icon, second.icon));
    assert!(turned_over(first.sdf_icon, second.sdf_icon));
    assert!(icon_direction(&bucket).is_none());
    assert!(sdf_icon_direction(&bucket).is_none());

    // Past the margin the text turns over as well.
    frame.set_bearing(30.0);
    let third = frame.draw(&placement, &bucket);
    assert!(turned_over(first.text, third.text));
    assert_eq!(text_direction(&bucket, 0), Some(!first_direction));
}

// gtest: LineLabelFlip.WithoutAMarginNothingIsRemembered
#[test]
fn without_a_margin_nothing_is_remembered() {
    let placement = test_placement(0.0);
    let bucket = up(&[1]);
    let mut frame = Frame::new();
    frame.set_bearing(-6.0);
    let first = frame.draw(&placement, &bucket);
    frame.set_bearing(6.0);
    let second = frame.draw(&placement, &bucket);
    // Today's rule: text and icons all turn over at the vertical.
    assert!(turned_over(first.text, second.text));
    assert!(turned_over(first.icon, second.icon));
    assert!(turned_over(first.sdf_icon, second.sdf_icon));
    assert!(text_direction(&bucket, 0).is_none());
    assert!(icon_direction(&bucket).is_none());
    assert!(sdf_icon_direction(&bucket).is_none());

    // Nor is a direction set on the label read.
    for was_flipped in [false, true] {
        set_text_direction(&bucket, 0, Some(was_flipped));
        assert_eq!(frame.draw(&placement, &bucket).text, second.text, "{was_flipped}");
    }
}

// gtest: LineLabelFlip.WithoutAMarginNothingIsSynced
#[test]
fn without_a_margin_nothing_is_synced() {
    let placement = test_placement(0.0);
    let drawn = up(&[1]);
    set_text_direction(&drawn, 0, Some(true));
    let from_cache = up(&[1]);
    mark_stale(&from_cache);
    let frame = Frame::new();
    frame.sync(&placement, &[&drawn, &from_cache]);
    assert!(text_direction(&from_cache, 0).is_none());
    assert!(is_stale(&from_cache));
}

// gtest: LineLabelFlip.TwoCopiesOnTwoZoomLevelsReadTheSameWay
#[test]
fn two_copies_on_two_zoom_levels_read_the_same_way() {
    let placement = test_placement(MARGIN);
    let mut frame = Frame::new();
    frame.set_bearing(6.0);
    // How a label drawn for the first time reads here, 6 degrees past the vertical of its line.
    let fresh = up(&[3]);
    frame.draw(&placement, &fresh);
    let first_drawing = text_direction(&fresh, 0).expect("drawn");

    // The zoom 16 copies were drawn before the map turned: label 1 the other way. The zoom 17 tile
    // has just come in, listed first.
    let z16 = up(&[1, 2]);
    set_text_direction(&z16, 0, Some(!first_drawing));
    set_text_direction(&z16, 1, Some(first_drawing));
    let z17 = up(&[1, 2]);
    mark_stale(&z17);
    frame.sync(&placement, &[&z17, &z16]);
    assert_eq!(text_direction(&z17, 0), Some(!first_drawing));
    assert_eq!(text_direction(&z17, 1), Some(first_drawing));
    assert_eq!(text_direction(&z16, 0), Some(!first_drawing));
    assert_eq!(text_direction(&z16, 1), Some(first_drawing));
    assert!(!is_stale(&z17));

    // The new copy is drawn the way the label reads, not as if for the first time.
    frame.draw(&placement, &z17);
    assert_eq!(text_direction(&z17, 0), Some(!first_drawing));
    assert_eq!(text_direction(&z17, 1), Some(first_drawing));
}

// gtest: LineLabelFlip.ATileBackFromTheCacheDoesNotBringBackAnOldDirection
#[test]
fn a_tile_back_from_the_cache_does_not_bring_back_an_old_direction() {
    let placement = test_placement(MARGIN);
    let frame = Frame::new();
    // Drawn unflipped when its tile went to the cache; since then the label has turned over on
    // another copy, which is drawn.
    let from_cache = up(&[1]);
    set_text_direction(&from_cache, 0, Some(false));
    mark_stale(&from_cache);
    let drawn = up(&[1]);
    set_text_direction(&drawn, 0, Some(true));
    frame.sync(&placement, &[&from_cache, &drawn]);
    assert_eq!(text_direction(&from_cache, 0), Some(true));
    assert_eq!(text_direction(&drawn, 0), Some(true));

    // With no other copy, it starts afresh rather than reading the old way.
    let later = test_placement(MARGIN);
    let alone = up(&[1]);
    set_text_direction(&alone, 0, Some(false));
    mark_stale(&alone);
    frame.sync(&later, &[&alone]);
    assert!(text_direction(&alone, 0).is_none());
    assert!(!is_stale(&alone));
}

// gtest: LineLabelFlip.AHiddenCopyDoesNotPassOnItsDirection
#[test]
fn a_hidden_copy_does_not_pass_on_its_direction() {
    let placement = test_placement(MARGIN);
    let frame = Frame::new();
    let duplicate = up(&[1]);
    set_text_direction(&duplicate, 0, Some(false));
    hide_text(&duplicate);
    let drawn = up(&[1]);
    set_text_direction(&drawn, 0, Some(true));
    frame.sync(&placement, &[&drawn, &duplicate]);
    assert_eq!(text_direction(&duplicate, 0), Some(true));
    assert_eq!(text_direction(&drawn, 0), Some(true));
}

// gtest: LineLabelFlip.IconsAreNotSynced
#[test]
fn icons_are_not_synced() {
    let placement = test_placement(MARGIN);
    let frame = Frame::new();
    let drawn = up(&[1]);
    set_text_direction(&drawn, 0, Some(true));
    drawn.state().icon.placed_symbols[0].keep_upright_flipped = Some(false);
    let other = up(&[1]);
    mark_stale(&other);
    frame.sync(&placement, &[&drawn, &other]);
    assert_eq!(text_direction(&other, 0), Some(true));
    assert!(icon_direction(&other).is_none());
    assert_eq!(icon_direction(&drawn), Some(false));
}

// gtest: LineLabelFlip.ThePreviousPlacementFillsInButTheCurrentOneComesFirst
#[test]
fn the_previous_placement_fills_in_but_the_current_one_comes_first() {
    let frame = Frame::new();
    // The previous placement saw the label drawn flipped.
    let previous_copy = up(&[1]);
    set_text_direction(&previous_copy, 0, Some(true));
    let previous = Rc::new(test_placement(MARGIN));
    frame.sync(&previous, &[&previous_copy]);

    // Now its only copy is hidden: it reads the way the previous placement saw it.
    let placement = test_placement(MARGIN);
    set_previous(&placement, previous.clone());
    let hidden = up(&[1]);
    hide_text(&hidden);
    frame.sync(&placement, &[&hidden]);
    assert_eq!(text_direction(&hidden, 0), Some(true));

    // A copy drawn the other way since then: the current placement's direction wins, otherwise the
    // label would turn back at every placement while the map is within the margin.
    let next = test_placement(MARGIN);
    set_previous(&next, previous);
    let drawn = up(&[1]);
    set_text_direction(&drawn, 0, Some(false));
    let duplicate = up(&[1]);
    hide_text(&duplicate);
    frame.sync(&next, &[&duplicate, &drawn]);
    assert_eq!(text_direction(&drawn, 0), Some(false));
    assert_eq!(text_direction(&duplicate, 0), Some(false));
}

// gtest: LineLabelFlip.SyncsOnlyWhenThePlacementOrTheBucketsChange
#[test]
fn syncs_only_when_the_placement_or_the_buckets_change() {
    // The opacities can be updated on every frame (a bucket of an overscaled tile with symbols out of
    // view is registered again on every frame), the sync only runs when what it depends on changes.
    let placement = test_placement(MARGIN);
    let frame = Frame::new();
    let drawn = up(&[1]);
    set_text_direction(&drawn, 0, Some(true));
    let duplicate = up(&[1]);
    hide_text(&duplicate);
    frame.sync(&placement, &[&drawn, &duplicate]);
    assert_eq!(text_direction(&duplicate, 0), Some(true));

    // The same buckets, in any order: nothing to do, so the hidden copy keeps what it holds.
    set_text_direction(&duplicate, 0, Some(false));
    frame.sync(&placement, &[&duplicate, &drawn]);
    assert_eq!(text_direction(&duplicate, 0), Some(false));

    // Another layer is synced on its own.
    let other = up(&[2]);
    mark_stale(&other);
    frame.sync_layer(&placement, &[&other], "other");
    assert!(!is_stale(&other));
    frame.sync(&placement, &[&duplicate, &drawn]);
    assert_eq!(text_direction(&duplicate, 0), Some(false));

    // The same buckets enter the cross tile index again (the layer was hidden and shown).
    mark_stale(&duplicate);
    frame.sync(&placement, &[&duplicate, &drawn]);
    assert_eq!(text_direction(&duplicate, 0), Some(true));

    // A bucket enters the cross tile index.
    let entered = up(&[1]);
    mark_stale(&entered);
    frame.sync(&placement, &[&drawn, &duplicate, &entered]);
    assert_eq!(text_direction(&duplicate, 0), Some(true));
    assert_eq!(text_direction(&entered, 0), Some(true));

    // A bucket leaves.
    set_text_direction(&duplicate, 0, Some(false));
    frame.sync(&placement, &[&drawn, &duplicate]);
    assert_eq!(text_direction(&duplicate, 0), Some(true));

    // A new placement.
    set_text_direction(&duplicate, 0, Some(false));
    let next = test_placement(MARGIN);
    frame.sync(&next, &[&drawn, &duplicate]);
    assert_eq!(text_direction(&duplicate, 0), Some(true));
}

// gtest: LineLabelFlip.CopiesWhoseLinesRunOppositeWaysReadTheSameWay
#[test]
fn copies_whose_lines_run_opposite_ways_read_the_same_way() {
    // Two tiles can hold the same road as lines running opposite ways: the copies share the way the
    // label reads, not the flag, which is relative to each copy's line.
    let mut frame = Frame::new();
    frame.set_bearing(6.0);
    for flipped in [false, true] {
        let placement = test_placement(MARGIN);
        let drawn = up(&[1]);
        set_text_direction(&drawn, 0, Some(flipped));
        let drawn_angle = frame.draw(&placement, &drawn).text;
        assert_eq!(text_direction(&drawn, 0), Some(flipped), "within the margin, either way is kept");

        let opposite = make_bucket(&[1], Line::Down);
        mark_stale(&opposite);
        frame.sync(&placement, &[&opposite, &drawn]);
        assert_eq!(text_direction(&opposite, 0), Some(!flipped), "{flipped}");
        assert!(!turned_over(drawn_angle, frame.draw(&placement, &opposite).text), "{flipped}");
    }
}

// gtest: LineLabelFlip.ACopyWhoseLineCrossesTheOthersStartsAfresh
#[test]
fn a_copy_whose_line_crosses_the_others_starts_afresh() {
    // Which way it would read cannot be told from a line at right angles: decided anew.
    let placement = test_placement(MARGIN);
    let frame = Frame::new();
    let drawn = up(&[1]);
    set_text_direction(&drawn, 0, Some(true));
    let across = make_bucket(&[1], Line::Across);
    mark_stale(&across);
    set_text_direction(&across, 0, Some(false));
    frame.sync(&placement, &[&across, &drawn]);
    assert!(text_direction(&across, 0).is_none());
}

// gtest: LineLabelFlip.ACopyDrawnWithoutALineDirectionPassesNothingOn
#[test]
fn a_copy_drawn_without_a_line_direction_passes_nothing_on() {
    // A drawn copy whose line has no direction at its anchor cannot tell which way the label reads: the
    // previous placement's direction is used instead.
    let frame = Frame::new();
    let previous_copy = up(&[1]);
    set_text_direction(&previous_copy, 0, Some(true));
    let previous = Rc::new(test_placement(MARGIN));
    frame.sync(&previous, &[&previous_copy]);

    let placement = test_placement(MARGIN);
    set_previous(&placement, previous);
    let drawn = make_bucket(&[1], Line::Point);
    set_text_direction(&drawn, 0, Some(false));
    let duplicate = up(&[1]);
    hide_text(&duplicate);
    frame.sync(&placement, &[&drawn, &duplicate]);
    assert_eq!(text_direction(&duplicate, 0), Some(true));
}

// gtest: LineLabelFlip.ASymbolBackInViewTakesTheDirectionOfItsLabel
#[test]
fn a_symbol_back_in_view_takes_the_direction_of_its_label() {
    // A symbol of an overscaled tile was out of view (no cross tile id) while the copy of its label in
    // another tile turned over. Back in view, it gets that copy's id in the same bucket: synced, even
    // though no bucket entered or left.
    let placement = test_placement(MARGIN);
    let frame = Frame::new();
    let overscaled = up(&[SymbolInstance::INVALID_CROSS_TILE_ID]);
    set_text_direction(&overscaled, 0, Some(false));
    hide_text(&overscaled);
    let other = up(&[1]);
    set_text_direction(&other, 0, Some(true));
    frame.sync(&placement, &[&overscaled, &other]);
    assert_eq!(text_direction(&overscaled, 0), Some(false));

    {
        let mut state = overscaled.state();
        state.symbol_instances[0].set_cross_tile_id(1);
        state.cross_tile_ids_reassigned = true;
    }
    frame.sync(&placement, &[&overscaled, &other]);
    assert_eq!(text_direction(&overscaled, 0), Some(true));
    assert!(!overscaled.state().cross_tile_ids_reassigned);
}
