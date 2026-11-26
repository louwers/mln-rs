// The terrain's drapes: the projection that draws a tile's square of the map
// into its drape, its stencil, the frame's tiles and image copies a drape
// draws, and the drapes that hold a texture.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use glam::DVec4;
use mln_gfx::frame::FrameTarget;
use mln_gfx::offscreen_target::OffscreenTarget;
use mln_gfx::{RendererBackend as _, ScissorRect};
use mln_style::bitmap::PremultipliedImage;
use mln_style::constants::EXTENT;
use mln_style::geo::LatLng;
use mln_style::layers::Raster;
use mln_style::size::Size;
use mln_style::subdivision_granularity::SubdivisionGranularitySetting;
use mln_style::tile_id::CanonicalTileID;
use mln_style::{Edit, Style};

use super::with_context;
use crate::camera::transform::Transform;
use crate::camera::{CameraOptions, ConstrainMode, Elevation, View, ViewportMode};
use crate::layer::render_layer::FrameSourceTiles;
use crate::map::{MapMode, TerrainOptions};
use crate::render::clip_masks::ClipMasks;
use crate::render::frame::Frame;
use crate::render::layer_composite::LayerComposite;
use crate::render::paint_parameters::{PaintParameters, TransformParameters};
use crate::render::static_data::RenderStaticData;
use crate::render::tile::{FoundTile, RenderTile};
use crate::render::update_parameters::UpdateParameters;
use crate::source::render_image_source::{ImageQuad, ImageSourceRenderData};
use crate::storage::{FileSource, Resource, Responder, Response};
use crate::tile::{OverscaledTileID, UnwrappedTileID};
use crate::tile_store::TileLodMode;
use crate::{Registry, RenderMode, Renderer, RendererEvent, RendererEvents};

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

/// Where a pass clears the stencil only as it begins, a drape clears it by
/// drawing the mask of a tile that covers its whole target, also in another
/// world copy than the first: its own tile.
#[test]
fn a_drape_clears_its_stencil_over_its_whole_target() {
    with_context(|_, context| {
        let view = view(0.0, 0.0);
        let frame = Frame::new(&view, false);
        let static_data = RenderStaticData::default();
        let (mut clip_masks, mut layer_composite) = (ClipMasks::default(), LayerComposite::default());
        let size = mln_gfx::Size::new(512, 512);
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
            parameters.drape(&transform, &mut drape_masks, &mut drape_composite, mln_gfx::Size::new(1024, 1024), drape);
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
        source.overlapping::<Raster>(&found, "raster", ids[0], false).iter().map(|tile| tile.tile.id).collect();
    assert_eq!(&ids[..4], &drawn[..]);
}

/// On the globe, which has no world copies, a drape draws the tiles over
/// its tile whatever their copy: near the antimeridian, a zoom 0 drape in
/// copy 0 draws the western zoom 1 tile that the globe's cover puts in
/// copy 1, as its west half. A Mercator drape doesn't.
#[test]
fn a_globe_drape_draws_the_tiles_over_it_of_every_world_copy() {
    let ids = [tile(1, 1, 0, 0), tile(0, 1, 1, 0), tile(1, 2, 0, 1)];
    let render_tiles: Vec<RenderTile> =
        ids.iter().map(|id| RenderTile::of_tile_id(*id, id.overscale_to(id.canonical.z), false)).collect();
    let contents = vec![None; render_tiles.len()];
    let found: Vec<FoundTile> = (0..render_tiles.len()).map(|tile| FoundTile { tile, view: None }).collect();
    let source = FrameSourceTiles::new(&render_tiles, &contents);
    let drape = tile(0, 0, 0, 0);
    let drawn = |globe: bool| -> Vec<UnwrappedTileID> {
        source.overlapping::<Raster>(&found, "raster", drape, globe).iter().map(|tile| tile.tile.id).collect()
    };
    assert_eq!(ids.to_vec(), drawn(true));
    assert_eq!(ids[1..2].to_vec(), drawn(false));
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

/// Answers every request at once with a PNG of sea level in the Mapbox
/// encoding, (1 · 65536 + 134 · 256 + 160) · 0.1 − 10000 = 0 m, which a
/// raster source draws as an opaque color.
struct SeaLevel;

impl FileSource for SeaLevel {
    fn load(&self, _resource: &Resource, responder: Responder) {
        let mut image = PremultipliedImage::new(Size::new(256, 256));
        for texel in image.data.as_chunks_mut::<4>().0 {
            *texel = [1, 134, 160, 255];
        }
        let mut response = Response::new();
        response.data = Some(Arc::new(crate::image::encode_png(&image)));
        responder.send(response);
    }

    fn can_request(&self, _resource: &Resource) -> bool {
        true
    }
}

/// A style over a terrain of flat ground with two raster layers that a
/// circle splits into two stacks: the first of a source whose bounds are in
/// the north-west terrain tile of a view of zoom 2 over 10° E, 10° S, the
/// second in the south-east one.
const TWO_STACKS: &str = r#"{
    "version": 8,
    "terrain": {"source": "dem"},
    "sources": {
        "dem": {"type": "raster-dem", "tiles": ["dem/{z}/{x}/{y}.png"], "tileSize": 256, "maxzoom": 10},
        "north-west": {"type": "raster", "tiles": ["a/{z}/{x}/{y}.png"], "bounds": [-20, 10, -10, 20]},
        "south-east": {"type": "raster", "tiles": ["b/{z}/{x}/{y}.png"], "bounds": [20, -30, 30, -20]},
        "point": {"type": "geojson", "data": {"type": "Point", "coordinates": [10, -10]}}
    },
    "layers": [
        {"id": "north-west", "type": "raster", "source": "north-west", "paint": {"raster-fade-duration": 0}},
        {"id": "circle", "type": "circle", "source": "point"},
        {"id": "south-east", "type": "raster", "source": "south-east", "paint": {"raster-fade-duration": 0}}
    ]
}"#;

/// A continuous map of `style` at zoom 2 over 10° E, 10° S, seen from above
/// over a 256 px WebGPU target, drawn until a frame of the camera on the
/// terrain's ground is complete. Returns the tile and stack of each drape
/// that has a texture then.
fn textured_drapes(style: &str) -> Option<BTreeSet<(OverscaledTileID, usize)>> {
    let registry = Registry::with_builtins();
    let parsed = Style::from_json(registry.style(), style).expect("the style");
    let mut style = Style::new(Arc::clone(registry.style()));
    for source in parsed.get_sources().iter() {
        style.apply(Edit::AddSource(crate::tests::ready(source.clone()))).unwrap();
    }
    for layer in parsed.get_layer_list().iter() {
        style.apply(Edit::AddLayer { layer: layer.clone(), before: None }).unwrap();
    }
    style.apply(Edit::SetTerrain(parsed.get_terrain().map(|terrain| (**terrain).clone()))).unwrap();
    let style = Arc::new(style);

    let size = Size::new(256, 256);
    let device = super::webgpu_device()?;
    let mut backend = mln_webgpu::RendererBackend::new(&device);
    let mut target = OffscreenTarget::<mln_webgpu::WebGpu>::new(mln_gfx::Size::new(size.width, size.height));
    let pool = crate::tests::pool();
    let mut renderer =
        Renderer::new(backend.gpu(), 1.0, crate::text::local_fonts::LocalGlyphs::default(), Arc::new(|| {}), &pool);
    let events = RendererEvents::new();
    renderer.set_event_queue(Some(events.clone()));
    let mut transform = Transform::new(ConstrainMode::HeightOnly, ViewportMode::Default);
    transform.resize(size).unwrap();
    transform.jump_to(&CameraOptions {
        center: Some(LatLng::new(-10.0, 10.0).unwrap()),
        zoom: Some(2.0),
        ..Default::default()
    });
    let file_source: Arc<dyn FileSource> = Arc::new(SeaLevel);
    let mut elevation: Option<Arc<dyn Elevation>> = None;
    let mut complete = false;
    for _ in 0..1000 {
        let parameters = Arc::new(UpdateParameters {
            style_loaded: true,
            mode: MapMode::Continuous,
            pixel_ratio: 1.0,
            time_point: None,
            view: transform.view().clone(),
            min_zoom: 0.0,
            max_zoom: 22.0,
            camera_moving: false,
            style: Arc::clone(&style),
            font_faces: None,
            sprite_loaded: true,
            feature_states: Arc::default(),
            file_source: Some(Arc::clone(&file_source)),
            prefetch_zoom_delta: 0,
            still_image_request: false,
            still_image_request_id: 0,
            update_id: 0,
            cross_source_collisions: true,
            fast_pfor_enabled: false,
            terrain: TerrainOptions::default(),
            elevation: elevation.clone(),
            tile_lod_min_radius: 3.0,
            tile_lod_scale: 1.0,
            tile_lod_pitch_threshold: mln_style::math::deg2rad(60.0),
            tile_lod_zoom_shift: 0.0,
            tile_lod_mode: TileLodMode::Default,
            subdivision_granularity: SubdivisionGranularitySetting::none(),
        });
        let gpu = backend.gpu();
        let frame = renderer.update(gpu, &parameters).expect("a continuous map's frame");
        let image = target.begin_frame(gpu).expect("the frame's image");
        let drawn = renderer.render(gpu, &image, frame);
        let handoff = target.end_frame(gpu);
        renderer.finish_frame(gpu, drawn, handoff);
        for event in events.take() {
            match event {
                RendererEvent::RenderFrameFinished { mode, .. } => complete = mode == RenderMode::Full,
                RendererEvent::TerrainElevation(ground) => elevation = ground,
                _ => {}
            }
        }
        if complete {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(complete, "no frame was complete");
    let textured = renderer.terrain().drapes().textured().into_iter().collect();
    target.tear_down(backend.gpu());
    Some(textured)
}

/// A stack has a drape with a texture only in the terrain tiles where its
/// layers draw something: each raster's stack, only in the tile of its
/// source's bounds.
#[test]
fn a_stack_has_a_drape_texture_only_where_its_layers_draw() {
    let tile = |x, y| OverscaledTileID::new(2, 0, CanonicalTileID::new(2, x, y));
    let expected = BTreeSet::from([(tile(1, 1), 0), (tile(2, 2), 1)]);
    let Some(actual) = textured_drapes(TWO_STACKS) else {
        return;
    };
    assert_eq!(expected, actual);
}
