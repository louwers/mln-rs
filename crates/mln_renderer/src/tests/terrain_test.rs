// Whether terrain draws, and the tiles that cover the view while it does.

use std::collections::BTreeSet;
use std::ops::Not;
use std::sync::Arc;

use mln_map::transform::Transform;
use mln_map_types::{
    CameraOptions, ConstrainMode, Elevation, MapDebugOptions, MapMode, TerrainSkirtLength, TileLodMode, ViewportMode,
};
use mln_storage::response::{Reason, ResourceError};
use mln_storage::{FileSource, Resource, Responder, Response};
use mln_style::sources::raster_dem_source;
use mln_style::terrain::Terrain;
use mln_style::{Edit, Source, Style};
use mln_style_types::ProjectionDefinition;
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::dem::{DemCoords, DemUnpack};
use mln_util::geo::LatLng;
use mln_util::image::PremultipliedImage;
use mln_util::pool::TaskGroup;
use mln_util::size::Size;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_util::tileset::{PartialTileset, RasterEncoding};
use parking_lot::Mutex;

use crate::Registry;
use crate::render_source::RenderSource;
use crate::renderer_event::RendererEvent;
use crate::terrain::{RenderTerrain, TerrainCover};
use crate::tile::contents::Contents;
use crate::tile::store::{StoreEvent, TileStore};
use crate::tile::tile::Tile;
use crate::tile::tile_workers::InlineWorkers;
use crate::tile_parameters::TileParameters;
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
        terrain_skirt_length: TerrainSkirtLength::Auto,
        elevation: None,
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

    // Tile mode and the globe draw it flat.
    assert_eq!(None, terrain_zooms(style_with(ready.clone()), &transform, MapMode::Tile));
    let mut globe = self::transform();
    globe.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
    assert!(globe.view().is_globe());
    assert_eq!(None, terrain_zooms(style_with(ready), &globe, MapMode::Static));
}

#[test]
fn the_surface_decodes_the_dem_as_its_tileset_says_and_hangs_skirts_as_the_map_says() {
    let transform = transform();
    let tileset = PartialTileset { raster_encoding: Some(RasterEncoding::Terrarium), ..tiles() };
    let mut style = style_with(crate::tests::ready(raster_dem_source("dem", None, tileset, 256)));
    style.apply(Edit::SetTerrain(Some(Terrain { source: "dem".to_owned(), exaggeration: 1.5 }))).unwrap();
    let surface = |skirts| {
        let mut parameters = update_parameters(style.clone(), &transform, MapMode::Static);
        parameters.terrain_skirt_length = skirts;
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
    // equator of GL JS's sphere.
    let tile_width = 2.0 * std::f64::consts::PI * 6_371_008.8 / 2f64.powf(10.5);
    assert_eq!((tile_width / 5.0) as f32, auto.props.skirt_length);

    let none = surface(TerrainSkirtLength::None);
    assert!(!none.skirts, "no skirts on the mesh");
    assert_eq!(0.0, none.props.skirt_length);
    assert_eq!(auto.props.unpack, none.props.unpack);
}

/// A file source whose DEM tile requests wait until the test answers them.
#[derive(Default)]
struct DemTiles {
    requested: Mutex<Vec<CanonicalTileID>>,
    pending: Mutex<Vec<(CanonicalTileID, Responder)>>,
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
                Some(response) => responder.send(response),
                None => waiting.push((tile, responder)),
            }
        }
        self.pending.lock().extend(waiting);
    }

    fn requested(&self) -> BTreeSet<CanonicalTileID> {
        self.requested.lock().iter().copied().collect()
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
            debug_options: MapDebugOptions::NO_DEBUG,
            view: &parameters.view,
            min_zoom: 0.0,
            max_zoom: 22.0,
            file_source: Some(file_source),
            mode: MapMode::Static,
            prefetch_zoom_delta: 0,
            tile_lod_min_radius: 3.0,
            tile_lod_scale: 1.0,
            tile_lod_pitch_threshold: mln_math::deg2rad(60.0),
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

    /// The DEM zoom and coords each terrain tile draws with, and whether
    /// its own DEM tile loads.
    fn bindings(&self) -> Vec<(Option<(u8, DemCoords)>, bool)> {
        let cover = self.terrain.cover().expect("terrain draws");
        cover
            .tiles
            .iter()
            .map(|tile| (tile.dem.map(|(id, coords)| (id.canonical.z, coords)), tile.dem_loading))
            .collect()
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
    assert!(load.bindings().iter().all(|&binding| binding == (None, true)));
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
    assert!(!first.dem_loading);
}

/// A terrain tile draws with its own DEM tile, a quadrant of it; while that
/// loads, with the sub-rect of the deepest ancestor that has loaded; before
/// any has, flat.
#[test]
fn a_terrain_tile_falls_back_to_the_deepest_loaded_ancestor() {
    let mut load = TerrainLoad::new((0, 14), 1.0);
    load.frame();
    assert!(load.bindings().iter().all(|&binding| binding == (None, true)));

    load.answer(|tile| (tile.z == 5).then(|| terrarium_png(100)));
    let tiles: Vec<OverscaledTileID> = load.terrain.cover().unwrap().tiles.iter().map(|tile| tile.id).collect();
    let ancestors: Vec<_> = tiles.iter().map(|id| (Some((5, sub_rect(id, 5))), true)).collect();
    assert_eq!(ancestors, load.bindings());
    assert!(!load.is_loaded());

    load.answer(|_| Some(terrarium_png(100)));
    let own: Vec<_> = tiles.iter().map(|id| (Some((9, sub_rect(id, 1))), false)).collect();
    assert_eq!(own, load.bindings());
    assert!(own.iter().any(|(dem, _)| dem.unwrap().1.offset != [0.0, 0.0]), "quadrants other than the first");
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
        let expected: Vec<_> = tiles.iter().map(|id| (dz.map(|dz| (10 - dz, sub_rect(id, dz))), false)).collect();
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
    let center_y = mln_util::projection::Projection::project(&center, 1.0 / mln_util::constants::TILE_SIZE_D).y;
    let added: Vec<OverscaledTileID> = cover.tiles.iter().map(|tile| tile.id).filter(|id| !flat.contains(id)).collect();
    assert_ne!(Vec::<OverscaledTileID>::new(), added);
    for tile in added {
        let top = f64::from(tile.canonical.y) / f64::from(1u32 << tile.canonical.z);
        assert!(top > center_y, "{tile} is south of the center");
    }
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
    let world = mln_util::projection::Projection::project(&center, 1.0 / mln_util::constants::TILE_SIZE_D);
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
    let Some(Contents::Dem(bucket)) = source.tile(&dem).and_then(Tile::contents) else { panic!("the DEM tile {dem}") };
    let data = bucket.dem_data();
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
    // Ground far from the view, where no tile drew.
    assert_eq!(None, ground.at(LatLng::new(-45.0, -170.0).unwrap()));

    load.seen = Some(ground);
    load.frame();
    assert!(load.ground_events().is_empty(), "no change, nothing to tell");

    load.style.apply(Edit::SetTerrain(None)).unwrap();
    load.update();
    load.seen = None;
    load.update();
    assert!(matches!(load.ground_events()[..], [None]));
}

/// An update's camera stands on the terrain's current ground only when the
/// update carries the snapshot of it that the terrain last told the map of,
/// which a still image waits for: not one that came before, and not none
/// while terrain draws. For an update that carries another, the terrain
/// tells the map of its current ground again, though it didn't change.
/// Without terrain, an update that carries none stands on it.
#[test]
fn an_update_stands_on_the_current_ground_when_it_carries_its_snapshot() {
    let mut load = TerrainLoad::new((0, 14), 1.0);
    load.frame();
    assert!(!load.grounded, "the update carries no ground");
    let loading = load.ground_events().pop().flatten().expect("the ground while it loads");
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

    load.style.apply(Edit::SetTerrain(None)).unwrap();
    load.update();
    assert!(!load.grounded, "the update carries a ground that is gone");
    load.seen = None;
    load.update();
    assert!(load.grounded);
}

/// A camera far below raised ground looks over it, which culls every tile;
/// the tile under the center still loads its DEM, which the camera stands
/// on.
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
    assert_eq!(Some(3_000_000.0), ground.at(LatLng::new(45.0, 10.0).unwrap()));
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
    let ancestors: Vec<_> = tiles.iter().map(|id| (Some((5, sub_rect(id, 5))), true)).collect();
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
        assert!(load.bindings().iter().all(|&binding| binding == (None, false)), "{zoom}");
        assert_eq!(BTreeSet::new(), load.tiles.requested(), "{zoom}");
    }
}

/// Nothing of the terrain outlives the data it was made from: once the
/// renderer lets go of the map's data, the terrain has no cover, and it
/// tells the map once that the ground is gone.
#[test]
fn the_terrain_lets_go_of_the_map_data() {
    let pool = crate::tests::pool();
    let mut orchestrator = crate::render_orchestrator::RenderOrchestrator::new(
        true,
        mln_text::local_glyph_rasterizer::LocalGlyphs::default(),
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
    let mut orchestrator = crate::render_orchestrator::RenderOrchestrator::new(
        true,
        mln_text::local_glyph_rasterizer::LocalGlyphs::default(),
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
}
