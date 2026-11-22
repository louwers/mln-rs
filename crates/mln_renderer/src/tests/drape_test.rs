// The terrain's drapes: the projection that draws a tile's square of the map
// into its drape, the scale of the drape's pixels, and the frame's tiles and
// image copies a drape draws.

use glam::{DVec4, Vec2, Vec4};
use mln_gfx::ScissorRect;
use mln_map::transform::Transform;
use mln_map_types::{CameraOptions, ConstrainMode, MapDebugOptions, View, ViewportMode};
use mln_style::layers::Fill;
use mln_tile_id::{CanonicalTileID, OverscaledTileID, UnwrappedTileID};
use mln_util::constants::EXTENT;
use mln_util::geo::LatLng;
use mln_util::size::Size;

use super::with_context;
use crate::clip_masks::ClipMasks;
use crate::frame::Frame;
use crate::layer_composite::LayerComposite;
use crate::layers::cast_matrix;
use crate::paint_parameters::{PaintParameters, TransformParameters};
use crate::render_layer::FrameSourceTiles;
use crate::render_static_data::RenderStaticData;
use crate::render_tile::{FoundTile, RenderTile};
use crate::sources::render_image_source::{ImageQuad, ImageSourceRenderData};

/// A camera pitched by `pitch` and turned to `bearing`, in degrees, over a
/// 512 px view at a fractional zoom.
fn view(pitch: f64, bearing: f64) -> View {
    let mut transform = Transform::new(ConstrainMode::HeightOnly, ViewportMode::Default);
    transform.resize(Size::new(512, 512)).unwrap();
    transform.jump_to(&CameraOptions {
        center: Some(LatLng::new(30.0, 20.0).unwrap()),
        zoom: Some(3.7),
        pitch: Some(pitch),
        bearing: Some(bearing),
        ..Default::default()
    });
    transform.view().clone()
}

fn tile(wrap: i16, z: u8, x: u32, y: u32) -> UnwrappedTileID {
    UnwrappedTileID::with_wrap(wrap, CanonicalTileID::new(z, x, y))
}

/// Where `transform` draws the point `x`, `y` of `tile`, in clip
/// coordinates.
fn clip(transform: &TransformParameters, tile: UnwrappedTileID, x: f64, y: f64) -> DVec4 {
    transform.proj_matrix * transform.view.tile_matrix(&tile) * DVec4::new(x, y, 0.0, 1.0)
}

#[track_caller]
fn assert_clip(actual: DVec4, x: f64, y: f64) {
    let expected = DVec4::new(x, y, 0.0, 1.0);
    assert!((actual - expected).abs().max_element() < 1e-9, "{actual} is not {expected}");
}

/// A drape draws its tile over the whole target, north up, whatever the
/// camera's pitch and bearing: its parent over four times the target, its
/// children over a quarter, each where it lies, and the tile in another
/// world copy beside it.
#[test]
fn a_drape_projects_its_tile_onto_the_whole_target() {
    let extent = f64::from(EXTENT);
    let view = view(60.0, 30.0);
    let drape = tile(1, 3, 5, 2);
    let transform = TransformParameters::for_drape(&view, &drape);
    assert_clip(clip(&transform, drape, 0.0, 0.0), -1.0, 1.0);
    assert_clip(clip(&transform, drape, extent, extent), 1.0, -1.0);
    assert_clip(clip(&transform, drape, extent / 2.0, extent / 4.0), 0.0, 0.5);

    // The drape's tile is the parent's north-east quarter.
    let parent = tile(1, 2, 2, 1);
    assert_clip(clip(&transform, parent, 0.0, 0.0), -3.0, 1.0);
    assert_clip(clip(&transform, parent, extent, extent), 1.0, -3.0);

    // This child is the drape's south-west quarter.
    let child = tile(1, 4, 10, 5);
    assert_clip(clip(&transform, child, 0.0, 0.0), -1.0, 0.0);
    assert_clip(clip(&transform, child, extent, extent), 0.0, -1.0);

    // The world copy to the west is eight tiles to the west.
    assert_clip(clip(&transform, tile(0, 3, 5, 2), 0.0, 0.0), -17.0, 1.0);
}

/// A drape's pixels are the map's at its zoom, so a line's extrusion in a
/// drape is as long in pixels as the line shader asks: its antialiasing's
/// gamma scale is 1, as GL JS's is in a drape, for the drape's tile and the
/// tiles of other zooms in it. The fill outlines' world is the drape's
/// texture.
#[test]
fn lines_in_a_drape_antialias_at_a_gamma_scale_of_1() {
    with_context(|_, context| {
        let view = view(60.0, 30.0);
        let frame = Frame::new(&view, false, MapDebugOptions::NO_DEBUG);
        let static_data = RenderStaticData::default();
        let (mut clip_masks, mut layer_composite) = (ClipMasks::default(), LayerComposite::default());
        let size = Size::new(512, 512);
        let scissor = ScissorRect { x: 0, y: 0, width: size.width, height: size.height };
        let mut parameters = PaintParameters::new(
            context,
            &mut clip_masks,
            &mut layer_composite,
            false,
            &frame,
            &static_data,
            1.0,
            size,
            scissor,
        );
        let drape = tile(0, 3, 5, 2);
        let transform = TransformParameters::for_drape(&view, &drape);
        let (mut drape_masks, mut drape_composite) = (ClipMasks::default(), LayerComposite::default());
        let drape_size = Size::new(1024, 1024);
        let drape_parameters = parameters.drape(&transform, &mut drape_masks, &mut drape_composite, drape_size, drape);
        let global = drape_parameters.global_paint_params(Size::new(1, 1));
        assert_eq!([1024.0, 1024.0], global.world_size);
        let units_to_pixels = Vec2::from(global.units_to_pixels);

        let zoom = view.zoom() as f32;
        let pixel_scale = view.projection().pixel_scale(&view) as f32;
        for tile in [drape, tile(0, 2, 2, 1), tile(0, 5, 21, 9)] {
            // The line shader's extrusion of `dist` pixels, at the middle of
            // the tile (`linesofar` and offsets aside).
            let matrix = glam::Mat4::from_cols_array(&cast_matrix(&(transform.proj_matrix * view.tile_matrix(&tile))));
            let ratio = pixel_scale / tile.pixels_to_tile_units(1.0, zoom);
            let dist = Vec2::new(3.0, -4.0);
            let projected_extrude = matrix * Vec4::new(dist.x / ratio, dist.y / ratio, 0.0, 0.0);
            let half = EXTENT as f32 / 2.0;
            let position = matrix * Vec4::new(half, half, 0.0, 1.0) + projected_extrude;
            let with_perspective = (projected_extrude.truncate().truncate() / position.w * units_to_pixels).length();
            let gamma_scale = dist.length() / with_perspective;
            assert!((gamma_scale - 1.0).abs() < 1e-4, "{tile:?}: gamma scale {gamma_scale}");
        }
    });
}

/// Where a pass clears the stencil only as it begins, a drape clears it by
/// drawing the mask of a tile that covers its whole target, also in another
/// world copy than the first: its own tile.
#[test]
fn a_drape_clears_its_stencil_over_its_whole_target() {
    with_context(|_, context| {
        let view = view(0.0, 0.0);
        let frame = Frame::new(&view, false, MapDebugOptions::NO_DEBUG);
        let static_data = RenderStaticData::default();
        let (mut clip_masks, mut layer_composite) = (ClipMasks::default(), LayerComposite::default());
        let size = Size::new(512, 512);
        let scissor = ScissorRect { x: 0, y: 0, width: size.width, height: size.height };
        let mut parameters = PaintParameters::new(
            context,
            &mut clip_masks,
            &mut layer_composite,
            false,
            &frame,
            &static_data,
            1.0,
            size,
            scissor,
        );
        if parameters.context.caps().clear_stencil_with_pass {
            return;
        }
        let drape = tile(1, 3, 5, 2);
        let transform = TransformParameters::for_drape(&view, &drape);
        let (mut drape_masks, mut drape_composite) = (ClipMasks::default(), LayerComposite::default());
        let drape_parameters =
            parameters.drape(&transform, &mut drape_masks, &mut drape_composite, Size::new(1024, 1024), drape);
        let [clear] = drape_parameters.stencil_clear_tiles(&[drape])[..] else {
            panic!("one tile that clears the stencil");
        };
        let extent = f64::from(EXTENT);
        let (north_west, south_east) = (clip(&transform, clear, 0.0, 0.0), clip(&transform, clear, extent, extent));
        let covers = |corner: DVec4, x: f64, y: f64| corner.x * x >= 1.0 - 1e-9 && corner.y * y >= 1.0 - 1e-9;
        assert!(covers(north_west, -1.0, 1.0), "{clear:?}: {north_west}");
        assert!(covers(south_east, 1.0, -1.0), "{clear:?}: {south_east}");
    });
}

/// A drape draws the frame's tiles that overlap its tile, those of its world
/// copy: its own, its ancestors and its descendants, and no others.
#[test]
fn a_drape_draws_the_tiles_that_overlap_its_tile() {
    let ids = [
        tile(0, 3, 5, 2),  // the drape's tile
        tile(0, 2, 2, 1),  // its parent
        tile(0, 0, 0, 0),  // the world's
        tile(0, 5, 21, 9), // a descendant
        tile(0, 3, 4, 2),  // a neighbor
        tile(0, 4, 8, 4),  // the neighbor's child
        tile(0, 2, 3, 1),  // a cousin
        tile(1, 3, 5, 2),  // the drape's tile in the next world copy
    ];
    let render_tiles: Vec<RenderTile> =
        ids.iter().map(|id| RenderTile::of_tile_id(*id, id.overscale_to(id.canonical.z), false)).collect();
    let contents = vec![None; render_tiles.len()];
    let found: Vec<FoundTile> = (0..render_tiles.len()).map(|tile| FoundTile { tile, view: None }).collect();
    let source = FrameSourceTiles::new(&render_tiles, &contents);
    let drawn: Vec<UnwrappedTileID> =
        source.overlapping::<Fill>(&found, "fill", ids[0], false).iter().map(|tile| tile.tile.id).collect();
    assert_eq!(&ids[..4], &drawn[..]);
}

/// On the globe, which has no world copies, a drape draws the tiles over
/// its tile whatever their copy, in its own: near the antimeridian, a
/// zoom 0 drape in copy 0 draws the western zoom 1 tile that the globe's
/// cover puts in copy 1, as its west half. A Mercator drape doesn't.
#[test]
fn a_globe_drape_draws_the_tiles_over_it_of_every_world_copy() {
    let ids = [tile(1, 1, 0, 0), tile(0, 1, 1, 0), tile(1, 2, 0, 1)];
    let render_tiles: Vec<RenderTile> =
        ids.iter().map(|id| RenderTile::of_tile_id(*id, id.overscale_to(id.canonical.z), false)).collect();
    let contents = vec![None; render_tiles.len()];
    let found: Vec<FoundTile> = (0..render_tiles.len()).map(|tile| FoundTile { tile, view: None }).collect();
    let source = FrameSourceTiles::new(&render_tiles, &contents);
    let drape = tile(0, 0, 0, 0);
    let drawn = |globe: bool| -> Vec<OverscaledTileID> {
        source.overlapping::<Fill>(&found, "fill", drape, globe).iter().map(|tile| tile.id()).collect()
    };
    let in_drape = ids.map(|id| tile(0, id.canonical.z, id.canonical.x, id.canonical.y));
    assert_eq!(in_drape.map(|id| id.overscale_to(id.canonical.z)).to_vec(), drawn(true));
    assert_eq!(vec![ids[1].overscale_to(1)], drawn(false));
    let stamped: Vec<OverscaledTileID> = source.stamps(&found, drape, true).map(|stamp| stamp.tile).collect();
    assert_eq!(drawn(true), stamped);
}

/// An image drawn in the tiles `tile_ids` over the box `bounds` of their
/// tile's units, which hasn't loaded.
fn image_in(tile_ids: &[UnwrappedTileID], bounds: [i32; 4]) -> ImageSourceRenderData<'_> {
    ImageSourceRenderData { bucket: None, tile_ids, bounds, quad: ImageQuad::TILE }
}

/// Each copy of an image overlaps the drapes of its own world copy.
#[test]
fn an_image_copy_overlaps_the_drapes_of_its_world_copy() {
    let copies = [tile(0, 3, 5, 2), tile(-1, 3, 5, 2)];
    let image = image_in(&copies, [1000, 1000, 7000, 7000]);
    assert!(image.overlaps(&copies[0], &tile(0, 3, 5, 2)));
    assert!(!image.overlaps(&copies[0], &tile(-1, 3, 5, 2)));
    assert!(image.overlaps(&copies[1], &tile(-1, 3, 5, 2)));
    assert!(!image.overlaps(&copies[1], &tile(0, 3, 5, 2)));
}

/// An image that reaches past its tile overlaps the drapes of the
/// neighbors it reaches into, and not those of the neighbors it only
/// touches.
#[test]
fn an_image_overlaps_the_drapes_past_its_tile_that_it_reaches() {
    let copies = [tile(0, 3, 5, 2)];
    // From the tile's west half to a quarter into its eastern neighbor, and
    // down to its southern neighbor's edge.
    let image = image_in(&copies, [EXTENT / 2, 0, EXTENT + EXTENT / 4, EXTENT]);
    assert!(image.overlaps(&copies[0], &tile(0, 3, 5, 2)));
    assert!(image.overlaps(&copies[0], &tile(0, 3, 6, 2)));
    assert!(!image.overlaps(&copies[0], &tile(0, 3, 4, 2)));
    assert!(!image.overlaps(&copies[0], &tile(0, 3, 5, 3)));
    assert!(!image.overlaps(&copies[0], &tile(0, 3, 5, 1)));
    // An image west of its tile, in the next world copy's westernmost tile,
    // reaches into this copy's easternmost.
    let copies = [tile(1, 3, 0, 2)];
    let image = image_in(&copies, [-EXTENT / 4, 0, EXTENT / 4, EXTENT]);
    assert!(image.overlaps(&copies[0], &tile(0, 3, 7, 2)));
    assert!(image.overlaps(&copies[0], &tile(1, 3, 0, 2)));
    assert!(!image.overlaps(&copies[0], &tile(1, 3, 7, 2)));
}

/// An image overlaps the drapes of coarser tiles that hold part of it, and
/// of finer tiles that it covers part of.
#[test]
fn an_image_overlaps_the_drapes_at_coarser_and_finer_zooms() {
    let copies = [tile(0, 3, 5, 2)];
    // The tile's north-east quarter, and a sliver past its east edge.
    let image = image_in(&copies, [EXTENT / 2, 0, EXTENT + 8, EXTENT / 2]);
    // Its parent and the world hold it; so does the parent's eastern
    // neighbor, which the sliver reaches.
    assert!(image.overlaps(&copies[0], &tile(0, 2, 2, 1)));
    assert!(image.overlaps(&copies[0], &tile(0, 0, 0, 0)));
    assert!(image.overlaps(&copies[0], &tile(0, 2, 3, 1)));
    assert!(!image.overlaps(&copies[0], &tile(0, 2, 2, 0)));
    // Of the tile's children, only the north-east one; of the eastern
    // neighbor's, only the north-west one, which the sliver reaches.
    assert!(image.overlaps(&copies[0], &tile(0, 4, 11, 4)));
    assert!(!image.overlaps(&copies[0], &tile(0, 4, 10, 4)));
    assert!(!image.overlaps(&copies[0], &tile(0, 4, 11, 5)));
    assert!(image.overlaps(&copies[0], &tile(0, 4, 12, 4)));
    assert!(!image.overlaps(&copies[0], &tile(0, 4, 12, 5)));
    // At zoom 10, the sliver is the first of eight columns of the eastern
    // neighbor's tiles.
    assert!(image.overlaps(&copies[0], &tile(0, 10, 6 * 128, 2 * 128)));
    assert!(!image.overlaps(&copies[0], &tile(0, 10, 6 * 128 + 1, 2 * 128)));
}
