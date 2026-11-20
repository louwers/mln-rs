// Whether terrain draws, and the tiles that cover the view while it does.

use std::sync::Arc;

use mln_map::transform::Transform;
use mln_map_types::{CameraOptions, ConstrainMode, MapDebugOptions, MapMode, TileLodMode, ViewportMode};
use mln_style::sources::raster_dem_source;
use mln_style::terrain::Terrain;
use mln_style::{Edit, Source, Style};
use mln_style_types::ProjectionDefinition;
use mln_util::geo::LatLng;
use mln_util::size::Size;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_util::tileset::PartialTileset;

use crate::Registry;
use crate::terrain::RenderTerrain;
use crate::update_parameters::UpdateParameters;

fn tiles() -> PartialTileset {
    PartialTileset { tiles: Some(vec!["tiles".to_owned()]), ..Default::default() }
}

/// A style with `source` and a terrain on the source "dem".
fn style_with(source: Source) -> Style {
    let mut style = Style::new(Arc::clone(Registry::with_builtins().style()));
    style.apply(Edit::AddSource(source)).unwrap();
    style.apply(Edit::SetTerrain(Some(Terrain::new("dem")))).unwrap();
    style
}

/// A camera at zoom 10.5 over a 512 px view.
fn transform() -> Transform {
    let mut transform = Transform::new(ConstrainMode::HeightOnly, ViewportMode::Default);
    transform.resize(Size::new(512, 512)).unwrap();
    transform.jump_to(&CameraOptions {
        center: Some(LatLng::new(45.0, 10.0).unwrap()),
        zoom: Some(10.5),
        ..Default::default()
    });
    transform
}

fn update_parameters(style: Style, transform: &Transform, mode: MapMode) -> UpdateParameters {
    UpdateParameters {
        style_loaded: true,
        mode,
        pixel_ratio: 1.0,
        debug_options: MapDebugOptions::NO_DEBUG,
        time_point: None,
        view: transform.view().clone(),
        min_zoom: 0.0,
        max_zoom: 22.0,
        camera_moving: false,
        style: Arc::new(style),
        sprite_loaded: true,
        feature_states: Arc::default(),
        file_source: None,
        prefetch_zoom_delta: 0,
        still_image_request: false,
        still_image_request_id: 0,
        cross_source_collisions: true,
        fast_pfor_enabled: false,
        tile_lod_min_radius: 3.0,
        tile_lod_scale: 1.0,
        tile_lod_pitch_threshold: mln_math::deg2rad(60.0),
        tile_lod_zoom_shift: 0.0,
        tile_lod_mode: TileLodMode::Default,
        subdivision_granularity: SubdivisionGranularitySetting::none(),
    }
}

/// The zoom levels of the terrain's tiles for `style`, or `None` where
/// terrain doesn't draw.
fn terrain_zooms(style: Style, transform: &Transform, mode: MapMode) -> Option<Vec<u8>> {
    let mut terrain = RenderTerrain::default();
    let cover = terrain.update(&update_parameters(style, transform, mode))?;
    let mut zooms: Vec<u8> = cover.tiles.iter().map(|tile| tile.id.canonical.z).collect();
    zooms.dedup();
    Some(zooms)
}

#[test]
fn terrain_tiles_are_twice_as_wide_as_the_dem_tiles() {
    let transform = transform();
    // Terrain tiles of 512 px cover zoom 10.5 at zoom 10, of 1024 px at 9.
    let dem = |tile_size| crate::tests::ready(raster_dem_source("dem", None, tiles(), tile_size));
    assert_eq!(Some(vec![10]), terrain_zooms(style_with(dem(256)), &transform, MapMode::Static));
    assert_eq!(Some(vec![9]), terrain_zooms(style_with(dem(512)), &transform, MapMode::Continuous));
}

#[test]
fn terrain_draws_from_a_raster_dem_source_whose_tileset_has_loaded() {
    let transform = transform();
    let ready = crate::tests::ready(raster_dem_source("dem", None, tiles(), 256));
    assert!(terrain_zooms(style_with(ready.clone()), &transform, MapMode::Static).is_some());

    // Not before its tileset loads, nor without the source.
    let loading =
        raster_dem_source("dem", Some("https://example.com/tiles.json".to_owned()), PartialTileset::default(), 256);
    assert_eq!(None, terrain_zooms(style_with(loading), &transform, MapMode::Static));
    let mut missing = style_with(ready.clone());
    missing.apply(Edit::SetTerrain(Some(Terrain::new("elsewhere")))).unwrap();
    assert_eq!(None, terrain_zooms(missing, &transform, MapMode::Static));

    // Tile mode and the globe draw it flat.
    assert_eq!(None, terrain_zooms(style_with(ready.clone()), &transform, MapMode::Tile));
    let mut globe = self::transform();
    globe.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
    assert!(globe.view().is_globe());
    assert_eq!(None, terrain_zooms(style_with(ready), &globe, MapMode::Static));
}
