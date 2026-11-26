// Whether terrain draws, and the tiles that cover the view while it does.

use std::collections::BTreeSet;
use std::ops::Not;
use std::sync::Arc;

use glam::DVec2;
use mln_storage::response::{Reason, ResourceError};
use mln_storage::{FileSource, Resource, Responder, Response};
use mln_style::bitmap::PremultipliedImage;
use mln_style::geo::{LatLng, WrapMode};
use mln_style::size::Size;
use mln_style::sources::raster_dem_source;
use mln_style::subdivision_granularity::SubdivisionGranularitySetting;
use mln_style::terrain::Terrain;
use mln_style::tile_id::CanonicalTileID;
use mln_style::tileset::{PartialTileset, RasterEncoding};
use mln_style::types::ProjectionDefinition;
use mln_style::{Edit, Source, Style};
use mln_tile_id::OverscaledTileID;
use mln_util::dem::{DemCoords, DemUnpack};
use mln_util::pool::TaskGroup;
use parking_lot::Mutex;

use crate::Registry;
use crate::camera::transform::Transform;
use crate::camera::{CameraOptions, ConstrainMode, Elevation, ViewportMode};
use crate::map::{MapMode, TerrainOptions, TerrainSkirtLength};
use crate::render::events::RendererEvent;
use crate::render::terrain::{RenderTerrain, TerrainCover, TerrainElevation};
use crate::render::update_parameters::UpdateParameters;
use crate::source::render_source::RenderSource;
use crate::tile_store::TileLodMode;
use crate::tile_store::contents::Contents;
use crate::tile_store::store::{StoreEvent, TileStore};
use crate::tile_store::tile::Tile;
use crate::tile_store::tile_parameters::TileParameters;
use crate::worker::tile_workers::InlineWorkers;

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
        time_point: None,
        view: transform.view().clone(),
        min_zoom: 0.0,
        max_zoom: 22.0,
        camera_moving: false,
        style: Arc::new(style),
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
        terrain: TerrainOptions::default(),
        elevation: None,
        tile_lod_min_radius: 3.0,
        tile_lod_scale: 1.0,
        tile_lod_pitch_threshold: mln_style::math::deg2rad(60.0),
        tile_lod_zoom_shift: 0.0,
        tile_lod_mode: TileLodMode::Default,
        subdivision_granularity: SubdivisionGranularitySetting::none(),
    }
}

/// The zoom levels of the terrain's tiles for `style`, or `None` where
/// terrain doesn't draw.
fn terrain_zooms(style: Style, transform: &Transform, mode: MapMode) -> Option<Vec<u8>> {
    let mut terrain = RenderTerrain::default();
    let cover = terrain.update(&update_parameters(style, transform, mode), |_| None)?;
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

    // Tile mode draws it flat; the globe draws it.
    assert_eq!(None, terrain_zooms(style_with(ready.clone()), &transform, MapMode::Tile));
    let mut globe = self::transform();
    globe.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
    assert!(globe.view().is_globe());
    assert!(terrain_zooms(style_with(ready), &globe, MapMode::Static).is_some());
}

#[test]
fn the_surface_decodes_the_dem_as_its_tileset_says_and_hangs_the_skirts_the_map_asks_for() {
    let transform = transform();
    let tileset = PartialTileset { raster_encoding: Some(RasterEncoding::Terrarium), ..tiles() };
    let mut style = style_with(crate::tests::ready(raster_dem_source("dem", None, tileset, 256)));
    style.apply(Edit::SetTerrain(Some(Terrain { source: "dem".to_owned(), exaggeration: 1.5 }))).unwrap();
    let surface = |skirts| {
        let parameters = UpdateParameters {
            terrain: TerrainOptions { skirt_length: skirts, ..TerrainOptions::default() },
            ..update_parameters(style.clone(), &transform, MapMode::Static)
        };
        let mut terrain = RenderTerrain::default();
        terrain.update(&parameters, |_| None).expect("terrain draws");
        terrain.surface()
    };

    let auto = surface(TerrainSkirtLength::Auto);
    assert!(auto.skirts);
    assert_eq!([65536.0, 256.0, 1.0, -8_388_608.0], auto.props.unpack);
    assert_eq!(1.0 / 256.0, auto.props.unpack_scale);
    assert_eq!(1.5, auto.props.exaggeration);
    // A fifth of a tile's width at the map's zoom, 10.5, in meters at the
    // equator.
    let tile_width = 2.0 * std::f64::consts::PI * 6_378_137.0 / 2f64.powf(10.5);
    assert_eq!((tile_width / 5.0) as f32, auto.props.skirt_length);

    // Without skirts, the tiles' grids have none to lower.
    let none = surface(TerrainSkirtLength::None);
    assert!(!none.skirts);
    assert_eq!(0.0, none.props.skirt_length);
    assert_eq!(auto.props.unpack, none.props.unpack);
}

/// A file source whose DEM tile requests wait until the test answers them,
/// and that can answer a request that stays open again, as the online file
/// source does to retry an error or revalidate expired data.
#[derive(Default)]
struct DemTiles {
    requested: Mutex<Vec<CanonicalTileID>>,
    pending: Mutex<Vec<(CanonicalTileID, Responder)>>,
    answered: Mutex<Vec<(CanonicalTileID, Responder)>>,
}

impl FileSource for DemTiles {
    fn load(&self, resource: &Resource, responder: Responder) {
        let data = resource.tile_data.as_ref().expect("only tiles are requested");
        let tile = CanonicalTileID::new(data.z as u8, data.x as u32, data.y as u32);
        self.requested.lock().push(tile);
        self.pending.lock().push((tile, responder));
    }

    fn can_request(&self, _resource: &Resource) -> bool {
        true
    }
}

impl DemTiles {
    /// Answers the waiting requests for which `answer` has a response.
    fn answer(&self, answer: impl Fn(CanonicalTileID) -> Option<Response>) {
        let pending = std::mem::take(&mut *self.pending.lock());
        let mut waiting = Vec::new();
        for (tile, responder) in pending {
            match answer(tile) {
                Some(response) => {
                    responder.send(response);
                    self.answered.lock().push((tile, responder));
                }
                None => waiting.push((tile, responder)),
            }
        }
        self.pending.lock().extend(waiting);
    }

    /// Answers again the answered requests that are still open and for which
    /// `answer` has a response.
    fn answer_again(&self, answer: impl Fn(CanonicalTileID) -> Option<Response>) {
        let answered = std::mem::take(&mut *self.answered.lock());
        let open: Vec<_> = answered.into_iter().filter(|(_, responder)| !responder.is_cancelled()).collect();
        for (tile, responder) in &open {
            if let Some(response) = answer(*tile) {
                responder.send(response);
            }
        }
        self.answered.lock().extend(open);
    }

    fn requested(&self) -> BTreeSet<CanonicalTileID> {
        self.requested.lock().iter().copied().collect()
    }

    /// How many requests `tile` got.
    fn requests_of(&self, tile: CanonicalTileID) -> usize {
        self.requested.lock().iter().filter(|&&requested| requested == tile).count()
    }
}

/// A DEM tile: 16 pixels a side, each `height` m in Terrarium.
fn terrarium_png(height: u16) -> Response {
    let mut image = PremultipliedImage::new(Size::new(16, 16));
    let [hi, lo] = (height + 32768).to_be_bytes();
    for pixel in image.data.as_chunks_mut::<4>().0 {
        *pixel = [hi, lo, 0, 255];
    }
    let mut response = Response::new();
    response.data = Some(Arc::new(mln_image::encode_png(&image)));
    response
}

/// A DEM tile: 16 pixels a side, rising 7 m a pixel eastward and 3 m
/// southward from 100 m, in Terrarium.
fn terrarium_slope() -> Response {
    let mut image = PremultipliedImage::new(Size::new(16, 16));
    for (i, pixel) in image.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = ((i % 16) as u16, (i / 16) as u16);
        let [hi, lo] = (100 + 7 * x + 3 * y + 32768).to_be_bytes();
        *pixel = [hi, lo, 0, 255];
    }
    let mut response = Response::new();
    response.data = Some(Arc::new(mln_image::encode_png(&image)));
    response
}

fn not_found() -> Response {
    let mut response = Response::new();
    response.error = Some(Box::new(ResourceError::new(Reason::NotFound, "no such tile")));
    response
}

fn connection_error() -> Response {
    let mut response = Response::new();
    response.error = Some(Box::new(ResourceError::new(Reason::Connection, "offline")));
    response
}

/// The terrain of a style whose only source is the raster-dem source
/// "dem", which no layer draws, driven as the renderer drives it.
struct TerrainLoad {
    transform: Transform,
    style: Style,
    source: Source,
    tiles: Arc<DemTiles>,
    pool: TaskGroup,
    store: TileStore,
    render_source: Box<dyn RenderSource>,
    terrain: RenderTerrain,
    relayout: bool,
    errors: usize,
    /// What the terrain told the map.
    events: Vec<RendererEvent>,
    /// The ground that the updates' camera stands on.
    seen: Option<Arc<dyn Elevation>>,
    /// Whether the last update's camera stood on the terrain's current
    /// ground.
    grounded: bool,
}

impl TerrainLoad {
    /// A terrain exaggerated by `exaggeration`, over a Terrarium DEM of 256
    /// px tiles of the zoom levels `zooms`.
    fn new(zooms: (u8, u8), exaggeration: f32) -> Self {
        let tileset = PartialTileset {
            min_zoom: Some(zooms.0),
            max_zoom: Some(zooms.1),
            raster_encoding: Some(RasterEncoding::Terrarium),
            ..tiles()
        };
        let source = crate::tests::ready(raster_dem_source("dem", None, tileset, 256));
        let mut style = style_with(source.clone());
        style.apply(Edit::SetTerrain(Some(Terrain { exaggeration, ..Terrain::new("dem") }))).unwrap();
        let pool = TaskGroup::new(&crate::tests::pool());
        let mut store = crate::tests::tile_store(&pool, Box::new(InlineWorkers));
        let render_source = crate::tests::create_render_source(&source, &mut store);
        let mut transform = transform();
        transform.resize(Size::new(1024, 1024)).unwrap();
        TerrainLoad {
            transform,
            style,
            source,
            tiles: Arc::default(),
            pool,
            store,
            render_source,
            terrain: RenderTerrain::default(),
            relayout: true,
            errors: 0,
            events: Vec::new(),
            seen: None,
            grounded: false,
        }
    }

    /// One update, as a frame's, which draws terrain: the terrain's cover,
    /// the source's tiles and the terrain's bindings.
    fn frame(&mut self) -> &TerrainCover {
        self.update();
        self.terrain.cover().expect("terrain draws")
    }

    /// One update, as a frame's.
    fn update(&mut self) {
        crate::tests::update(&mut self.store);
        let parameters = UpdateParameters {
            elevation: self.seen.clone(),
            ..update_parameters(self.style.clone(), &self.transform, MapMode::Static)
        };
        let (store, render_source) = (&self.store, &self.render_source);
        self.terrain.update(&parameters, |_| store.source(render_source.tiles()?));
        let file_source: Arc<dyn FileSource> = Arc::<DemTiles>::clone(&self.tiles);
        let tile_parameters = TileParameters {
            pixel_ratio: 1.0,
            view: &parameters.view,
            min_zoom: 0.0,
            max_zoom: 22.0,
            file_source: Some(file_source),
            mode: MapMode::Static,
            prefetch_zoom_delta: 0,
            tile_lod_min_radius: 3.0,
            tile_lod_scale: 1.0,
            tile_lod_pitch_threshold: mln_style::math::deg2rad(60.0),
            tile_lod_zoom_shift: 0.0,
            tile_lod_mode: TileLodMode::Default,
            subdivision_granularity: SubdivisionGranularitySetting::none(),
            global_state: None,
            is_update_synchronous: false,
            font_faces: None,
            glyph_pbfs: false,
            placement_commits: 0,
            terrain: self.terrain.cover(),
        };
        let relayout = std::mem::take(&mut self.relayout);
        self.render_source.update(&mut self.store, self.source.clone(), &[], false, relayout, &tile_parameters);
        let (store, render_source) = (&self.store, &self.render_source);
        self.grounded = self.terrain.bind(|_| store.source(render_source.tiles()?), &parameters, &mut self.events);
        self.store.drive();
        self.errors +=
            self.store.take_events().iter().filter(|event| matches!(event, StoreEvent::Error { .. })).count();
    }

    /// What the terrain told the map of its ground since the last call.
    fn ground_events(&mut self) -> Vec<Option<Arc<dyn Elevation>>> {
        self.events
            .drain(..)
            .filter_map(|event| match event {
                RendererEvent::TerrainElevation(elevation) => Some(elevation),
                _ => None,
            })
            .collect()
    }

    /// Answers the DEM requests `answer` has a response for, then runs
    /// frames until the tiles that arrived are in: one takes in the data,
    /// which decodes as the store drives it, and the next takes in the
    /// DEM.
    fn answer(&mut self, answer: impl Fn(CanonicalTileID) -> Option<Response>) {
        self.tiles.answer(answer);
        for _ in 0..3 {
            self.pool.wait();
            self.frame();
        }
    }

    fn is_loaded(&self) -> bool {
        self.render_source.is_loaded(&self.store)
    }

    /// The DEM zoom and coords each terrain tile draws with.
    fn bindings(&self) -> Vec<Option<(u8, DemCoords)>> {
        let cover = self.terrain.cover().expect("terrain draws");
        cover.tiles.iter().map(|tile| tile.dem.map(|(id, coords)| (id.canonical.z, coords))).collect()
    }
}

/// Where tile (z, x, y) is in its ancestor `dz` zooms up.
fn sub_rect(tile: &OverscaledTileID, dz: u8) -> DemCoords {
    let tiles = f32::from(1u16 << dz);
    let mask = (1u32 << dz) - 1;
    let CanonicalTileID { x, y, .. } = tile.canonical;
    DemCoords { scale: 1.0 / tiles, offset: [(x & mask) as f32 / tiles, (y & mask) as f32 / tiles] }
}

/// A raster-dem source that only the terrain reads requests the DEM tiles
/// the terrain's tiles read, at zoom − 1, and a coarse one at zoom 5 for
/// culling, and no others; it is loaded once they are.
#[test]
fn a_terrain_only_dem_source_loads_the_dem_tiles_the_terrain_reads() {
    let mut load = TerrainLoad::new((0, 14), 1.0);
    let cover = load.frame();
    let terrain_zooms: BTreeSet<u8> = cover.tiles.iter().map(|tile| tile.id.canonical.z).collect();
    assert_eq!(BTreeSet::from([10]), terrain_zooms);
    let dem_tiles: BTreeSet<CanonicalTileID> = cover.dem_tiles.iter().map(|id| id.canonical).collect();
    let expected: BTreeSet<CanonicalTileID> =
        cover.tiles.iter().flat_map(|tile| [tile.id.scaled_to(9).canonical, tile.id.scaled_to(5).canonical]).collect();
    assert_eq!(expected, dem_tiles);
    assert_eq!(dem_tiles, load.tiles.requested());
    assert!(!load.is_loaded(), "the DEM tiles are loading");
    assert!(load.bindings().iter().all(Option::is_none));
    assert!(load.render_source.base().enabled.not(), "nothing renders the source");

    load.answer(|_| Some(terrarium_png(100)));
    assert!(load.is_loaded());
    assert_eq!(dem_tiles, load.tiles.requested(), "nothing else is requested");
    assert_eq!(0, load.errors);
}

/// A DEM tile the tileset doesn't have settles: the terrain loads, and the
/// tile draws with an ancestor's DEM, so a still image is drawn without
/// failing.
#[test]
fn a_missing_dem_tile_settles() {
    let mut load = TerrainLoad::new((0, 14), 1.0);
    load.frame();
    let missing = load.terrain.cover().unwrap().tiles[0].id.scaled_to(9).canonical;
    for _ in 0..10 {
        load.answer(|tile| Some(if tile == missing { not_found() } else { terrarium_png(100) }));
        if load.is_loaded() {
            break;
        }
    }
    assert!(load.is_loaded());
    assert_eq!(0, load.errors);
    let first = load.terrain.cover().unwrap().tiles[0];
    let (dem, coords) = first.dem.expect("an ancestor's DEM");
    assert!(dem.canonical.z < 9, "{dem}");
    assert_eq!(coords, sub_rect(&first.id, 10 - dem.canonical.z));
}

/// The terrain tile under the view's center, which the camera stands on.
fn center_tile(load: &TerrainLoad) -> OverscaledTileID {
    let center = load.transform.view().center(WrapMode::Wrapped);
    let world = mln_util::projection::Projection::project(&center, 1.0 / mln_style::constants::TILE_SIZE_D);
    let cover = load.terrain.cover().expect("terrain draws");
    let holds = |id: &OverscaledTileID| {
        let tiles = f64::from(1u32 << id.canonical.z);
        let (u, v) = (world.x * tiles - f64::from(id.canonical.x), world.y * tiles - f64::from(id.canonical.y));
        (0.0..1.0).contains(&u) && (0.0..1.0).contains(&v)
    };
    cover.tiles.iter().map(|tile| tile.id).find(holds).expect("a tile at the center")
}

/// A DEM tile whose request fails has done what it can: the terrain loads,
/// so a still finishes, with the error, and the terrain tile that reads it
/// draws, and the camera stands, on an ancestor's DEM rather than wait for
/// its own. Its request stays open, and the file source's retry brings its
/// DEM without a new request: the tile draws it, the map stands on it, and
/// the cover culls by its range.
#[test]
fn a_failed_dem_tile_draws_an_ancestor_until_the_file_source_retries() {
    let mut load = TerrainLoad::new((0, 14), 1.0);
    load.frame();
    let tile = center_tile(&load);
    let failing = tile.scaled_to(9).canonical;
    for _ in 0..10 {
        load.answer(|dem| Some(if dem == failing { connection_error() } else { terrarium_png(100) }));
        if load.is_loaded() {
            break;
        }
    }
    assert!(load.is_loaded(), "a failed DEM tile has done what it can");
    assert_eq!(1, load.errors);
    let binding = |load: &TerrainLoad| {
        let tile = load.terrain.cover().unwrap().tiles.iter().find(|bound| bound.id == tile).copied().unwrap();
        tile.dem.map(|(id, coords)| (id.canonical.z, coords))
    };
    let Some((z, coords)) = binding(&load) else { panic!("{:?}", binding(&load)) };
    assert!(z < 9, "an ancestor's DEM: {z}");
    assert_eq!(sub_rect(&tile, 10 - z), coords);
    let center = load.transform.view().center(WrapMode::Wrapped);
    let ground = load.ground_events().pop().flatten().expect("the ground");
    assert_eq!(Some(100.0), ground.at(center), "the ancestor's ground");
    let requests = load.tiles.requests_of(failing);

    // The file source retries, after its backoff.
    load.tiles.answer_again(|dem| (dem == failing).then(|| terrarium_png(300)));
    for _ in 0..3 {
        load.pool.wait();
        load.frame();
    }
    assert!(load.is_loaded());
    assert_eq!(1, load.errors);
    assert_eq!(Some((9, sub_rect(&tile, 1))), binding(&load));
    let ground = load.ground_events().pop().flatten().expect("the new ground");
    assert_eq!(Some(300.0), ground.at(center));
    assert_eq!(Some((300.0, 300.0)), load.terrain.cover().unwrap().ranges.range(tile.canonical));
    assert_eq!(requests, load.tiles.requests_of(failing), "the retry made no new request");
}

/// A DEM tile whose data expires reloads in place when the file source
/// revalidates it: the terrain tile that reads it draws its DEM throughout,
/// never waiting on it nor standing on an ancestor's, and then the new DEM,
/// which the map stands on and the cover culls by.
#[test]
fn a_dem_tile_that_reloads_draws_its_new_dem() {
    let mut load = TerrainLoad::new((0, 14), 1.0);
    load.frame();
    load.answer(|_| Some(terrarium_png(100)));
    assert!(load.is_loaded());
    let tile = center_tile(&load);
    let reloaded = tile.scaled_to(9).canonical;
    let center = load.transform.view().center(WrapMode::Wrapped);
    assert_eq!(Some(100.0), load.ground_events().pop().flatten().expect("the ground").at(center));

    load.tiles.answer_again(|dem| (dem == reloaded).then(|| terrarium_png(300)));
    for _ in 0..3 {
        load.pool.wait();
        load.frame();
        let bound = load.terrain.cover().unwrap().tiles.iter().find(|bound| bound.id == tile).copied().unwrap();
        assert_eq!(Some((9, sub_rect(&tile, 1))), bound.dem.map(|(id, coords)| (id.canonical.z, coords)));
    }
    assert!(load.is_loaded());
    let ground = load.ground_events().pop().flatten().expect("the new ground");
    assert_eq!(Some(300.0), ground.at(center));
    assert_eq!(Some((300.0, 300.0)), load.terrain.cover().unwrap().ranges.range(tile.canonical));
    assert_eq!(0, load.errors);
}

/// A terrain tile draws with its own DEM tile, a quadrant of it; while that
/// loads, with the sub-rect of the deepest ancestor that has loaded; before
/// any has, flat.
#[test]
fn a_terrain_tile_falls_back_to_the_deepest_loaded_ancestor() {
    let mut load = TerrainLoad::new((0, 14), 1.0);
    load.frame();
    assert!(load.bindings().iter().all(Option::is_none));

    load.answer(|tile| (tile.z == 5).then(|| terrarium_png(100)));
    let tiles: Vec<OverscaledTileID> = load.terrain.cover().unwrap().tiles.iter().map(|tile| tile.id).collect();
    let ancestors: Vec<_> = tiles.iter().map(|id| Some((5, sub_rect(id, 5)))).collect();
    assert_eq!(ancestors, load.bindings());
    assert!(!load.is_loaded());

    load.answer(|_| Some(terrarium_png(100)));
    let own: Vec<_> = tiles.iter().map(|id| Some((9, sub_rect(id, 1)))).collect();
    assert_eq!(own, load.bindings());
    assert!(own.iter().any(|dem| dem.unwrap().1.offset != [0.0, 0.0]), "quadrants other than the first");
}

/// Past the DEM's highest zoom, a terrain tile reads that zoom's tile; at
/// its lowest, that zoom's own tile, not the zoom below, which the DEM
/// doesn't have; below it, no DEM (GL JS's `_getSourceZoom`).
#[test]
fn the_dem_zoom_stays_within_the_dem_zoom_levels() {
    for (zooms, dz) in [((0, 7), Some(3)), ((10, 14), Some(0)), ((11, 14), None)] {
        let mut load = TerrainLoad::new(zooms, 1.0);
        load.frame();
        let requested: BTreeSet<u8> = load.tiles.requested().iter().map(|tile| tile.z).collect();
        load.answer(|_| Some(terrarium_png(100)));
        assert!(load.is_loaded(), "{zooms:?}");
        let tiles: Vec<OverscaledTileID> = load.terrain.cover().unwrap().tiles.iter().map(|tile| tile.id).collect();
        let expected: Vec<_> = tiles.iter().map(|id| dz.map(|dz| (10 - dz, sub_rect(id, dz)))).collect();
        assert_eq!(expected, load.bindings(), "{zooms:?}");
        let reads = match dz {
            Some(3) => BTreeSet::from([5, 7]),
            Some(_) => BTreeSet::from([10]),
            None => BTreeSet::new(),
        };
        assert_eq!(reads, requested, "{zooms:?}");
    }
}

/// The ranges come from the DEM tiles that have loaded, with the
/// exaggeration: a tile under a known DEM takes its range, and the cover
/// keeps raised tiles that flat ground would cull; elsewhere, the range is
/// unknown, and the cover culls by GL JS's flat allowance.
#[test]
fn ranges_come_from_the_loaded_dem_tiles() {
    let mut load = TerrainLoad::new((0, 14), 2.0);
    load.transform.resize(Size::new(512, 512)).unwrap();
    load.transform.jump_to(&CameraOptions { zoom: Some(14.0), pitch: Some(60.0), ..Default::default() });
    let flat: Vec<OverscaledTileID> = load.frame().tiles.iter().map(|tile| tile.id).collect();
    let read = flat[0].scaled_to(flat[0].canonical.z - 1).canonical;
    load.answer(|tile| (tile.z == 5).then(|| terrarium_png(450)));
    load.answer(|tile| (tile == read).then(|| terrarium_png(425)));

    let cover = load.frame();
    assert_eq!(Some((850.0, 850.0)), cover.ranges.range(flat[0].canonical));
    // A tile whose own DEM tile hasn't loaded takes the zoom-5 tile's.
    let other = flat.iter().find(|id| id.scaled_to(read.z).canonical != read).expect("another DEM tile");
    assert_eq!(Some((900.0, 900.0)), cover.ranges.range(other.canonical));
    // Ground under no DEM tile that has loaded.
    assert_eq!(None, cover.ranges.range(CanonicalTileID::new(10, 0, 0)));

    // Raised ground in front of the camera, below the bottom edge of the
    // view, rises into it.
    let center = LatLng::new(45.0, 10.0).unwrap();
    let center_y = mln_util::projection::Projection::project(&center, 1.0 / mln_style::constants::TILE_SIZE_D).y;
    let added: Vec<OverscaledTileID> = cover.tiles.iter().map(|tile| tile.id).filter(|id| !flat.contains(id)).collect();
    assert_ne!(Vec::<OverscaledTileID>::new(), added);
    for tile in added {
        let top = f64::from(tile.canonical.y) / f64::from(1u32 << tile.canonical.z);
        assert!(top > center_y, "{tile} is south of the center");
    }
}

/// A DEM tile's range outlives the tile: once its source lets go of it,
/// without the cache to keep it, the cover still culls by it, with the
/// terrain's exaggeration of the moment. A DEM of another tileset has none
/// of the ranges of the one before.
#[test]
fn the_ranges_outlive_the_dem_tiles() {
    let mut load = TerrainLoad::new((0, 14), 2.0);
    load.store.set_cache_enabled(false);
    load.frame();
    load.answer(|_| Some(terrarium_png(425)));
    let tile = load.terrain.cover().unwrap().tiles[0].id;
    assert_eq!(Some((850.0, 850.0)), load.frame().ranges.range(tile.canonical));

    // Far away, the source lets go of the DEM tiles of the tile.
    let elsewhere = LatLng::new(-30.0, -100.0).unwrap();
    load.transform.jump_to(&CameraOptions { center: Some(elsewhere), ..Default::default() });
    load.frame();
    let source = load.store.source(load.render_source.tiles().unwrap()).unwrap();
    assert!(source.tile(&tile.scaled_to(9)).is_none(), "the source holds the DEM tile");
    assert!(source.tile(&tile.scaled_to(5)).is_none(), "the source holds the coarse DEM tile");
    assert_eq!(Some((850.0, 850.0)), load.frame().ranges.range(tile.canonical));

    let exaggerated = |exaggeration| Terrain { exaggeration, ..Terrain::new("dem") };
    load.style.apply(Edit::SetTerrain(Some(exaggerated(3.0)))).unwrap();
    assert_eq!(Some((1275.0, 1275.0)), load.frame().ranges.range(tile.canonical));

    // The source, replaced by one of other tiles, has no ranges until they
    // load: not those of the tiles before, which it still holds as the
    // terrain covers the view, before the source updates.
    load.answer(|_| Some(terrarium_png(425)));
    let tileset = PartialTileset {
        tiles: Some(vec!["other tiles".to_owned()]),
        min_zoom: Some(0),
        max_zoom: Some(14),
        raster_encoding: Some(RasterEncoding::Terrarium),
        ..Default::default()
    };
    load.source = crate::tests::ready(raster_dem_source("dem", None, tileset, 256));
    load.style = style_with(load.source.clone());
    load.style.apply(Edit::SetTerrain(Some(exaggerated(3.0)))).unwrap();
    let here = load.frame().tiles[0].id.canonical;
    assert_eq!(None, load.frame().ranges.range(here));
    load.answer(|_| Some(terrarium_png(100)));
    assert_eq!(Some((300.0, 300.0)), load.frame().ranges.range(here));
    assert_eq!(None, load.frame().ranges.range(tile.canonical));
}

/// Past their bound, the ranges forget those of the DEM tiles that their
/// source no longer holds, and keep those of the tiles it does.
#[test]
fn the_ranges_forget_the_dem_tiles_let_go_past_their_bound() {
    use mln_style::source::SourceData;
    use mln_style::sources::RasterDemSourceType;

    use crate::render::terrain::{ElevationRanges, REMEMBERED_RANGES};

    let mut load = TerrainLoad::new((0, 14), 1.0);
    load.frame();
    load.answer(|_| Some(terrarium_png(425)));
    let tile = load.frame().tiles[0].id.canonical;
    let source = load.store.source(load.render_source.tiles().unwrap());
    let data = SourceData::<RasterDemSourceType>::recover(&load.source).unwrap();
    let tileset = data.description.as_ref().unwrap();

    let mut ranges = ElevationRanges::default();
    ranges.update(tileset, source, 1.0);
    let held = ranges.len();
    let let_go = |i: usize| CanonicalTileID::new(14, i as u32, 0);
    for i in held..REMEMBERED_RANGES {
        ranges.insert(let_go(i), 0.0, 1.0);
    }
    ranges.update(tileset, source, 1.0);
    assert_eq!(REMEMBERED_RANGES, ranges.len(), "the ranges within their bound");
    ranges.insert(let_go(REMEMBERED_RANGES), 0.0, 1.0);
    ranges.update(tileset, source, 1.0);
    assert_eq!(held, ranges.len());
    assert_eq!(Some((425.0, 425.0)), ranges.range(tile));
    assert_eq!(None, ranges.range(CanonicalTileID::new(15, 0, 0)));
}

/// The ground the terrain tells the map is the height rule on the DEM that
/// each tile drew: at the center, the rule's height on the DEM of the tile
/// drawn there, exaggerated. The terrain tells the map once per change of
/// its ground, and once when terrain stops.
#[test]
fn the_ground_the_map_stands_on_is_the_height_rule_on_the_drawn_dem() {
    let mut load = TerrainLoad::new((0, 14), 1.5);
    load.frame();
    load.answer(|_| Some(terrarium_slope()));
    assert!(load.is_loaded());
    let ground = load.ground_events().pop().flatten().expect("the ground");

    let center = LatLng::new(45.0, 10.0).unwrap();
    let world = mln_util::projection::Projection::project(&center, 1.0 / mln_style::constants::TILE_SIZE_D);
    let cover = load.terrain.cover().unwrap();
    let (tile, uv) = cover
        .tiles
        .iter()
        .find_map(|tile| {
            let tiles = f64::from(1u32 << tile.id.canonical.z);
            let u = world.x * tiles - f64::from(tile.id.canonical.x);
            let v = world.y * tiles - f64::from(tile.id.canonical.y);
            ((0.0..1.0).contains(&u) && (0.0..1.0).contains(&v)).then_some((tile, [u as f32, v as f32]))
        })
        .expect("a tile at the center");
    let (dem, coords) = tile.dem.expect("its DEM");
    let source = load.store.source(load.render_source.tiles().unwrap()).unwrap();
    let Some(Contents::Dem(contents)) = source.tile(&dem).and_then(Tile::contents) else {
        panic!("the DEM tile {dem}")
    };
    let data = contents.dem();
    let (dim, stride) = (data.dim as u32, data.stride as usize);
    let unpack = DemUnpack::of(RasterEncoding::Terrarium);
    let texel = |x: u32, y: u32| {
        let start = (y as usize * stride + x as usize) * 4;
        unpack.decode(data.image().data[start..start + 4].try_into().unwrap())
    };
    let height = mln_util::dem::sample(dim, coords, uv, texel) * 1.5;
    assert_eq!(Some(f64::from(height)), ground.at(center));
    assert!(height.fract() != 0.0, "{height} reads between pixels");
    assert_eq!(Some(f64::from(data.range().0 * 1.5)), ground.min_at(center));
    // The surface drawn there, and none in a world copy the frame didn't draw.
    assert_eq!(Some(f64::from(height)), ground.surface_at(DVec2::new(world.x, world.y)));
    assert_eq!(None, ground.surface_at(DVec2::new(world.x - 2.0, world.y)));
    // Ground far from the view, where no tile drew.
    let far = LatLng::new(-45.0, -170.0).unwrap();
    let far_world = mln_util::projection::Projection::project(&far, 1.0 / mln_style::constants::TILE_SIZE_D);
    assert_eq!((None, None), (ground.at(far), ground.surface_at(DVec2::new(far_world.x, far_world.y))));

    load.seen = Some(ground);
    load.frame();
    assert!(load.ground_events().is_empty(), "no change, nothing to tell");

    load.style.apply(Edit::SetTerrain(None)).unwrap();
    load.update();
    load.seen = None;
    load.update();
    assert!(matches!(load.ground_events()[..], [None]));
}

/// An update is grounded only when it carries the terrain's current ground
/// itself, which a still image waits for: not one that came before the DEM
/// loaded, not other ground of the same height everywhere, and not none
/// while terrain draws. For an update that carries other ground, the
/// terrain tells the map of its current ground again, though it didn't
/// change. Without terrain, an update that carries none is grounded.
#[test]
fn an_update_is_grounded_when_it_carries_the_current_ground() {
    let mut load = TerrainLoad::new((0, 14), 1.0);
    load.frame();
    assert!(!load.grounded, "the update carries no ground");
    let loading = load.ground_events().pop().flatten().expect("the ground while it loads");
    // The tiles draw flat while their DEM loads, and the center stands on
    // what they draw.
    let center = load.transform.view().center(WrapMode::Wrapped);
    let world = mln_util::projection::Projection::project(&center, 1.0 / mln_style::constants::TILE_SIZE_D);
    assert_eq!((Some(0.0), Some(0.0)), (loading.at(center), loading.surface_at(DVec2::new(world.x, world.y))));
    load.seen = Some(Arc::clone(&loading));
    load.frame();
    assert!(load.grounded);
    assert!(load.ground_events().is_empty());

    load.answer(|_| Some(terrarium_png(100)));
    assert!(load.is_loaded());
    assert!(!load.grounded, "the ground changed as the DEM loaded");
    let loaded = load.ground_events().pop().flatten().expect("the loaded ground");
    load.frame();
    assert!(!load.grounded, "the update carries the ground from before");
    let again = load.ground_events();
    assert!(matches!(&again[..], [Some(ground)] if Arc::ptr_eq(ground, &loaded)), "{again:?}");
    load.seen = Some(Arc::clone(&loaded));
    load.frame();
    assert!(load.grounded);
    assert!(load.ground_events().is_empty());

    load.seen = Some(Arc::new(Uniform { at: Some(100.0), min_at: Some(100.0) }));
    load.frame();
    assert!(!load.grounded, "other ground of the same height");
    let again = load.ground_events();
    assert!(matches!(&again[..], [Some(ground)] if Arc::ptr_eq(ground, &loaded)), "{again:?}");

    load.style.apply(Edit::SetTerrain(None)).unwrap();
    load.update();
    assert!(!load.grounded, "the update carries a ground that is gone");
    load.seen = None;
    load.update();
    assert!(load.grounded);
}

/// A camera far below raised ground looks over it, which culls every tile;
/// the tile under the center still loads its DEM, which the camera stands
/// on, though it draws no surface.
#[test]
fn a_camera_below_the_ground_stands_on_the_dem_under_its_center() {
    let mut load = TerrainLoad::new((0, 14), 100.0);
    load.transform.jump_to(&CameraOptions { pitch: Some(60.0), ..Default::default() });
    load.frame();
    load.answer(|_| Some(terrarium_png(30_000)));
    let cover = load.terrain.cover().unwrap();
    assert!(cover.tiles.is_empty(), "{:?}", cover.tiles);
    let center = cover.center.expect("the tile under the center");
    assert!(cover.dem_tiles.contains(&center.id.scaled_to(center.id.canonical.z - 1)));
    let ground = load.ground_events().pop().flatten().expect("the ground");
    let center = LatLng::new(45.0, 10.0).unwrap();
    assert_eq!(Some(3_000_000.0), ground.at(center));
    // No surface drew there.
    let world = mln_util::projection::Projection::project(&center, 1.0 / mln_style::constants::TILE_SIZE_D);
    assert_eq!(None, ground.surface_at(DVec2::new(world.x, world.y)));
}

/// The snapshot of the ground that `load`'s last frame drew, of a Terrarium
/// DEM exaggerated by `exaggeration`.
fn snapshot(load: &TerrainLoad, exaggeration: f32) -> TerrainElevation {
    let cover = load.terrain.cover().expect("terrain draws");
    let dem = load.store.source(load.render_source.tiles().unwrap());
    TerrainElevation::new(cover, dem, DemUnpack::of(RasterEncoding::Terrarium), exaggeration)
}

/// The location of `world`, a Web Mercator point at zoom 0.
fn location_of(world: DVec2) -> LatLng {
    let point = mln_style::geometry::Point::new(world.x, world.y);
    mln_util::projection::Projection::unproject(&point, 1.0 / mln_style::constants::TILE_SIZE_D, WrapMode::Unwrapped)
        .unwrap()
}

/// Points over the tiles of `load`'s cover and the tile under its center,
/// and some way around them, off the tiles' edges, in the world copy each
/// tile is in.
fn points_over(load: &TerrainLoad) -> Vec<DVec2> {
    let cover = load.terrain.cover().unwrap();
    let bounds = |tile: &crate::render::terrain::TerrainTile| {
        let tiles = f64::from(1u32 << tile.id.canonical.z);
        let (x, y) = (f64::from(tile.id.canonical.x), f64::from(tile.id.canonical.y));
        let wrap = f64::from(tile.id.wrap);
        (DVec2::new(x / tiles + wrap, y / tiles), DVec2::new((x + 1.0) / tiles + wrap, (y + 1.0) / tiles))
    };
    let (min, max) = cover
        .tiles
        .iter()
        .chain(&cover.center)
        .map(bounds)
        .reduce(|(min, max), (low, high)| (min.min(low), max.max(high)))
        .expect("tiles");
    let (min, max) = (min - (max - min) * 0.1, max + (max - min) * 0.1);
    // Steps of irrational fractions keep the points off the edges.
    (0..120)
        .flat_map(|row| (0..120).map(move |column| (column, row)))
        .map(|(column, row)| {
            let f = DVec2::new((f64::from(column) + 0.5) / 120.0, (f64::from(row) + 0.5) / 120.0);
            let f = (f + DVec2::splat(std::f64::consts::SQRT_2 * 1e-4)).min(DVec2::ONE);
            min + (max - min) * f
        })
        .filter(|world| (0.0..=1.0).contains(&world.y))
        .collect()
}

/// The corners and the middles of the edges of the tiles of `load`'s cover
/// and of the tile under its center, in the world copy each tile is in:
/// points exactly on the edges between tiles.
fn edges_over(load: &TerrainLoad) -> Vec<DVec2> {
    let cover = load.terrain.cover().unwrap();
    let edges = |tile: &crate::render::terrain::TerrainTile| {
        let tiles = f64::from(1u32 << tile.id.canonical.z);
        let (x, y) = (f64::from(tile.id.canonical.x), f64::from(tile.id.canonical.y));
        let wrap = f64::from(tile.id.wrap);
        [(0.0, 0.0), (0.5, 0.0), (1.0, 0.0), (0.0, 0.5), (1.0, 0.5), (0.0, 1.0), (0.5, 1.0), (1.0, 1.0)]
            .map(|(u, v)| DVec2::new((x + u) / tiles + wrap, (y + v) / tiles))
    };
    cover.tiles.iter().chain(&cover.center).flat_map(edges).collect()
}

/// The per-zoom index finds, at every point, the tile the scan of the tiles
/// in order found, so the height and floor under the camera are as they
/// were: while the DEM loads, with ancestors in place, once it has loaded,
/// over tiles of several zooms, across world copies, on the tile the cover
/// keeps under the center, and on the edges between tiles.
#[test]
fn the_index_finds_the_tiles_the_scan_found() {
    let check = |load: &TerrainLoad, exaggeration: f32, what: &str| {
        let ground = snapshot(load, exaggeration);
        let points = points_over(load);
        let mut found = 0;
        for world in &points {
            let location = location_of(*world);
            assert_eq!(ground.scanned(location), (ground.at(location), ground.min_at(location)), "{what}: {world}");
            found += usize::from(ground.min_at(location).is_some());
        }
        assert!(found > points.len() / 2, "{what}: {found} of {} points on tiles", points.len());
        for world in edges_over(load) {
            let location = location_of(world);
            let edge = format!("{what}: on an edge at {world}");
            assert_eq!(ground.scanned(location), (ground.at(location), ground.min_at(location)), "{edge}");
        }
    };

    let mut load = TerrainLoad::new((0, 14), 1.5);
    load.frame();
    check(&load, 1.5, "loading");
    load.answer(|tile| (tile.z == 5).then(terrarium_slope));
    check(&load, 1.5, "ancestors");
    load.answer(|_| Some(terrarium_slope()));
    check(&load, 1.5, "loaded");

    load.transform.jump_to(&CameraOptions {
        zoom: Some(14.0),
        pitch: Some(70.0),
        bearing: Some(30.0),
        ..Default::default()
    });
    load.frame();
    load.answer(|_| Some(terrarium_slope()));
    let zooms: BTreeSet<u8> = load.terrain.cover().unwrap().tiles.iter().map(|tile| tile.id.canonical.z).collect();
    assert!(zooms.len() > 2, "{zooms:?}");
    check(&load, 1.5, "pitched");

    load.transform.jump_to(&CameraOptions {
        center: Some(LatLng::new(45.0, 179.99).unwrap()),
        zoom: Some(10.5),
        pitch: Some(0.0),
        ..Default::default()
    });
    load.frame();
    load.answer(|_| Some(terrarium_slope()));
    let wraps: BTreeSet<i16> = load.terrain.cover().unwrap().tiles.iter().map(|tile| tile.id.wrap).collect();
    assert!(wraps.len() > 1, "{wraps:?}");
    check(&load, 1.5, "across the antimeridian");

    let mut below = TerrainLoad::new((0, 14), 100.0);
    below.transform.jump_to(&CameraOptions { pitch: Some(60.0), ..Default::default() });
    below.frame();
    below.answer(|_| Some(terrarium_png(30_000)));
    let cover = below.terrain.cover().unwrap();
    assert!(cover.tiles.is_empty(), "{:?}", cover.tiles);
    let ground = snapshot(&below, 100.0);
    let center = LatLng::new(45.0, 10.0).unwrap();
    assert_eq!((Some(3_000_000.0), Some(3_000_000.0)), ground.scanned(center));
    assert_eq!(ground.scanned(center), (ground.at(center), ground.min_at(center)));
}

/// The surface is that of the tiles drawn in the world copy of the point:
/// none in a copy the frame didn't draw, and where copies draw the same
/// place at different zooms, each copy's own. Seen from a low zoom, the
/// view shows more than one copy.
#[test]
fn the_surface_is_the_one_drawn_in_the_points_world_copy() {
    let mut load = TerrainLoad::new((0, 14), 1.0);
    load.transform.jump_to(&CameraOptions {
        center: Some(LatLng::new(30.0, 150.0).unwrap()),
        zoom: Some(0.7),
        pitch: Some(60.0),
        bearing: Some(80.0),
        ..Default::default()
    });
    load.frame();
    load.answer(|_| Some(terrarium_slope()));
    let cover = load.terrain.cover().unwrap();
    let wraps: BTreeSet<i16> = cover.tiles.iter().map(|tile| tile.id.wrap).collect();
    assert!(wraps.len() > 1, "{wraps:?}");
    let ground = snapshot(&load, 1.0);
    let (mut drawn, mut undrawn) = (0, 0);
    for row in 0..60 {
        for column in 0..300 {
            let f = |at: u32, of: u32| (f64::from(at) + 0.5) / f64::from(of) + std::f64::consts::SQRT_2 * 1e-4;
            let world = DVec2::new(-2.0 + 5.0 * f(column, 300), f(row, 60).min(1.0));
            let surface = ground.surface_at(world);
            assert_eq!(ground.scanned_surface(world), surface, "{world}");
            match surface {
                Some(_) => drawn += 1,
                // A copy of a tile the frame drew elsewhere.
                None if ground.at(location_of(world)).is_some() => undrawn += 1,
                None => {}
            }
        }
    }
    assert!(drawn > 0 && undrawn > 0, "{drawn} points on drawn tiles, {undrawn} on tiles drawn in another copy");
}

/// Every tile drawn flat while its DEM loads draws at sea level, which the
/// ground gives, under the camera too; one drawn with an ancestor's DEM in
/// its place draws that DEM's height.
#[test]
fn the_ground_is_at_sea_level_on_loading_tiles() {
    let mut load = TerrainLoad::new((0, 14), 1.0);
    load.frame();
    let middles = |load: &TerrainLoad| -> Vec<DVec2> {
        let cover = load.terrain.cover().unwrap();
        assert!(cover.tiles.len() > 4);
        cover
            .tiles
            .iter()
            .map(|tile| {
                let tiles = f64::from(1u32 << tile.id.canonical.z);
                let middle = |at: u32| (f64::from(at) + 0.5) / tiles;
                DVec2::new(middle(tile.id.canonical.x) + f64::from(tile.id.wrap), middle(tile.id.canonical.y))
            })
            .collect()
    };
    let loading = snapshot(&load, 1.0);
    for world in middles(&load) {
        assert_eq!((Some(0.0), Some(0.0)), (loading.surface_at(world), loading.at(location_of(world))), "{world}");
    }

    load.answer(|tile| (tile.z == 5).then(|| terrarium_png(100)));
    let ancestors = snapshot(&load, 1.0);
    for world in middles(&load) {
        assert_eq!(Some(100.0), ancestors.surface_at(world), "{world}");
        assert_eq!(Some(100.0), ancestors.at(location_of(world)), "{world}");
    }
}

/// The range spans the drawn tiles' DEM data with the exaggeration, each
/// tile's own pixels, and sea level for tiles drawn flat; no tiles drawn,
/// no range. The surface stays within it.
#[test]
fn range_spans_the_drawn_dems_exaggerated() {
    let mut load = TerrainLoad::new((0, 14), 1.5);
    load.frame();
    assert_eq!(Some((0.0, 0.0)), snapshot(&load, 1.5).range(), "drawn flat while the DEM loads");

    // One DEM tile in, under some of the tiles: the others draw flat.
    let first = load.terrain.cover().unwrap().tiles[0].id;
    let read = first.scaled_to(first.canonical.z - 1).canonical;
    load.answer(|tile| (tile == read).then(|| terrarium_png(400)));
    assert_eq!(Some((0.0, 600.0)), snapshot(&load, 1.5).range());

    // The slope rises from 100 m to 100 + 7·15 + 3·15 m.
    let mut load = TerrainLoad::new((0, 14), 1.5);
    load.frame();
    load.answer(|_| Some(terrarium_slope()));
    let ground = snapshot(&load, 1.5);
    assert_eq!(Some((150.0, 375.0)), ground.range());
    for world in points_over(&load) {
        if let Some(height) = ground.surface_at(world) {
            assert!((150.0..=375.0).contains(&height), "{height} at {world}");
        }
    }

    let mut below = TerrainLoad::new((0, 14), 100.0);
    below.transform.jump_to(&CameraOptions { pitch: Some(60.0), ..Default::default() });
    below.frame();
    below.answer(|_| Some(terrarium_png(30_000)));
    assert_eq!(None, snapshot(&below, 100.0).range(), "no tile drew");
}

/// The coarse DEM tile a terrain tile reads for culling loads while its
/// own, finer DEM tile is still loading: the finer tile's search for a
/// loaded ancestor doesn't make the coarse tile a fallback that waits.
#[test]
fn the_coarse_dem_tile_loads_while_the_fine_one_is_loading() {
    let mut load = TerrainLoad::new((0, 14), 1.0);
    for _ in 0..3 {
        load.frame();
    }
    load.answer(|tile| (tile.z == 5).then(|| terrarium_png(100)));
    let tiles: Vec<OverscaledTileID> = load.terrain.cover().unwrap().tiles.iter().map(|tile| tile.id).collect();
    let ancestors: Vec<_> = tiles.iter().map(|id| Some((5, sub_rect(id, 5)))).collect();
    assert_eq!(ancestors, load.bindings());
}

/// A DEM whose lowest zoom is above its highest, as a TileJSON can say,
/// has no zoom a terrain tile can read: the terrain draws flat and loads
/// nothing, below its lowest zoom and at or above it.
#[test]
fn a_dem_whose_zoom_levels_are_empty_draws_flat() {
    for zoom in [10.5, 13.5] {
        let mut load = TerrainLoad::new((12, 10), 1.0);
        load.transform.jump_to(&CameraOptions { zoom: Some(zoom), ..Default::default() });
        let cover = load.frame();
        assert!(cover.tiles.iter().any(|tile| tile.id.canonical.z >= 12) == (zoom > 12.0), "{zoom}");
        load.frame();
        assert!(load.bindings().iter().all(Option::is_none), "{zoom}");
        assert_eq!(BTreeSet::new(), load.tiles.requested(), "{zoom}");
    }
}

/// Nothing of the terrain outlives the data it was made from: once the
/// renderer lets go of the map's data, the terrain has no cover, and it
/// tells the map once that the ground is gone.
#[test]
fn the_terrain_lets_go_of_the_map_data() {
    let pool = crate::tests::pool();
    let mut orchestrator = crate::render::orchestrator::RenderOrchestrator::new(
        true,
        mln_text::local_fonts::LocalGlyphs::default(),
        Arc::default(),
        crate::text::font_face_engine::default_engine(),
        Registry::with_builtins(),
        Arc::new(|| {}),
        &pool,
    );
    let ground = |events: Vec<RendererEvent>| -> Vec<bool> {
        events
            .into_iter()
            .filter_map(|event| match event {
                RendererEvent::TerrainElevation(elevation) => Some(elevation.is_some()),
                _ => None,
            })
            .collect()
    };
    let style = style_with(crate::tests::ready(raster_dem_source("dem", None, tiles(), 256)));
    orchestrator.update(&Arc::new(update_parameters(style, &transform(), MapMode::Static)));
    assert!(orchestrator.terrain().cover().is_some());
    assert_eq!(vec![true], ground(orchestrator.take_events()));

    orchestrator.clear_data();
    assert!(orchestrator.terrain().cover().is_none());
    assert_eq!(vec![false], ground(orchestrator.take_events()));
}

/// A renderer that doesn't draw terrain tells the map that there is no
/// ground for each update that carries one, though it never drew any:
/// another renderer may have.
#[test]
fn a_renderer_without_terrain_tells_a_grounded_map_there_is_no_ground() {
    let pool = crate::tests::pool();
    let mut orchestrator = crate::render::orchestrator::RenderOrchestrator::new(
        true,
        mln_text::local_fonts::LocalGlyphs::default(),
        Arc::default(),
        crate::text::font_face_engine::default_engine(),
        Registry::with_builtins(),
        Arc::new(|| {}),
        &pool,
    );
    let mut ground = |elevation: Option<Arc<dyn Elevation>>| -> Vec<bool> {
        let style = style_with(crate::tests::ready(raster_dem_source("dem", None, tiles(), 256)));
        let parameters = UpdateParameters { elevation, ..update_parameters(style, &transform(), MapMode::Tile) };
        orchestrator.update(&Arc::new(parameters));
        orchestrator
            .take_events()
            .into_iter()
            .filter_map(|event| match event {
                RendererEvent::TerrainElevation(elevation) => Some(elevation.is_some()),
                _ => None,
            })
            .collect()
    };
    assert_eq!(Vec::<bool>::new(), ground(None));
    let elsewhere: Arc<dyn Elevation> = Arc::new(Uniform { at: None, min_at: None });
    assert_eq!(vec![false], ground(Some(Arc::clone(&elsewhere))));
    assert_eq!(vec![false], ground(Some(elsewhere)));
}

/// Ground of one height and floor everywhere, as another renderer may
/// have drawn.
#[derive(Debug)]
struct Uniform {
    at: Option<f64>,
    min_at: Option<f64>,
}

impl Elevation for Uniform {
    fn at(&self, _: LatLng) -> Option<f64> {
        self.at
    }

    fn min_at(&self, _: LatLng) -> Option<f64> {
        self.min_at
    }

    fn surface_at(&self, _: DVec2) -> Option<f64> {
        self.at
    }

    fn range(&self) -> Option<(f64, f64)> {
        self.at.map(|at| (at, at))
    }
}

/// A ray that crosses the antimeridian can sample a hair west of a world
/// copy, where `x - x.floor()` rounds to the copy's east edge: the surface
/// there is the first column's of the next copy, as on the edge itself.
#[test]
fn the_surface_a_hair_west_of_a_copy_is_its_neighbors() {
    let mut load = TerrainLoad::new((0, 14), 1.0);
    load.transform.jump_to(&CameraOptions { center: Some(LatLng::new(45.0, -179.99).unwrap()), ..Default::default() });
    load.frame();
    load.answer(|_| Some(terrarium_slope()));
    let ground = snapshot(&load, 1.0);
    let y = mln_util::projection::Projection::project(
        &LatLng::new(45.0, -179.99).unwrap(),
        1.0 / mln_style::constants::TILE_SIZE_D,
    )
    .y;
    let edge = ground.surface_at(DVec2::new(0.0, y));
    assert!(edge.is_some(), "the first column drew");
    assert_eq!(edge, ground.surface_at(DVec2::new(-1e-17, y)));
    assert_eq!(ground.surface_at(DVec2::new(-1.0, y)), ground.surface_at(DVec2::new(-1.0 - 1e-17, y)));
}

/// Ridges across a DEM tile: 16 px a side, at 100 m but for three columns
/// of every eight, 60 m higher.
fn terrarium_ridges() -> Response {
    let mut image = PremultipliedImage::new(Size::new(16, 16));
    for (i, pixel) in image.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let ridge = if (3..6).contains(&(i % 16 % 8)) { 60 } else { 0 };
        let [hi, lo] = (100 + ridge + 32768u16).to_be_bytes();
        *pixel = [hi, lo, 0, 255];
    }
    let mut response = Response::new();
    response.data = Some(Arc::new(mln_image::encode_png(&image)));
    response
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
fn assert_glyphs_draw_in_circles(dem: fn() -> Response, pitch_with_map: bool) {
    use glam::Vec2;
    use mln_style::constants::{EXTENT, TILE_SIZE_D};
    use mln_style::layers::symbol_layer_properties::SymbolLayoutProperties;

    use crate::buckets::symbol_bucket::{PlacedSymbol, PlacedSymbolPools, SymbolSizeBinder};
    use crate::camera::TileProjector;
    use crate::layout::symbol_instance::WritingMode;
    use crate::layout::symbol_projection::{
        LabelPlaneProjector, LineLabelAlignment, SymbolHeights, reproject_line_labels,
    };
    use crate::placement::collision_feature::{CollisionBox, CollisionFeature};
    use crate::placement::collision_grid::Shape;
    use crate::placement::collision_index::{CollisionIndex, LabelRun, OverlapMode, PartCollision};
    use crate::query::feature_index::IndexedSubfeature;
    use crate::util::pool::PoolRange;

    let mut load = TerrainLoad::new((0, 14), 100.0);
    load.transform.jump_to(&CameraOptions { pitch: Some(60.0), ..Default::default() });
    load.frame();
    load.answer(|_| Some(dem()));
    let ground = snapshot(&load, 100.0);
    let view = load.transform.view().clone();
    let tile = load.terrain.cover().unwrap().tiles[0].id;
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
        overlap_mode: OverlapMode::Always,
        pitch_with_map,
        rotate_with_map: true,
        avoid_edges: None,
        collision_group: None,
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
            overlap_mode: OverlapMode::Always,
        },
        along_line: true,
    };
    let run = LabelRun { symbol: &symbol, pools: &pools, font_size: 24.0 };
    let mut shapes = Vec::new();
    index.place_feature(&part, &feature, &circles, &run, None, &mut shapes);
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
