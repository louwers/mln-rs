// Ports the style-side tests of test/style/source.test.cpp (the ones that
// need no RenderSource; those are in mln_renderer/tests/source_test.rs).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use mln_actor::RunLoop;
use mln_conversion::Convertible;
use mln_storage::response::{Reason, ResourceError};
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_storage::{Kind, Response};
use mln_style_types::SourceType;
use mln_tile_id::CanonicalTileID;
use mln_util::geo::LatLng;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use mln_util::tileset::{RasterEncoding, Tileset, VectorEncoding};

use super::util::parse_json;
use crate::conversion::Error;
use crate::conversion::source::convert_source;
use crate::conversion::tileset::SourceOptions;
use crate::observer::SourceObserver;
use crate::source::Source;
use crate::sources::geojson_source::{GeoJSONOptions, GeoJSONSource};
use crate::sources::image_source::ImageSource;
use crate::sources::tile_source::{RasterDEMSource, RasterSource, UrlOrTileset, VectorSource};

/// A test's handler for a source event.
type SourceHandler = Box<dyn Fn(&dyn Source)>;

/// A test's handler for a source error.
type SourceErrorHandler = Box<dyn Fn(&dyn Source, mln_util::Error)>;

/// `StubStyleObserver` (`test/src/mln/test/stub_style_observer.hpp`), the
/// `SourceObserver` half.
#[derive(Default)]
struct StubStyleObserver {
    source_loaded: RefCell<Option<SourceHandler>>,
    source_changed: RefCell<Option<SourceHandler>>,
    source_error: RefCell<Option<SourceErrorHandler>>,
}

impl SourceObserver for StubStyleObserver {
    fn on_source_changed(&self, source: &dyn Source) {
        if let Some(f) = &*self.source_changed.borrow() {
            f(source);
        }
    }

    fn on_source_loaded(&self, source: &dyn Source) {
        if let Some(f) = &*self.source_loaded.borrow() {
            f(source);
        }
    }

    fn on_source_error(&self, source: &dyn Source, error: mln_util::Error) {
        if let Some(f) = &*self.source_error.borrow() {
            f(source, error);
        }
    }
}

// gtest: Source.LoadingFail
#[test]
fn loading_fail() {
    let run_loop = RunLoop::new();
    let file_source = StubFileSource::new(ResponseType::Asynchronous);
    file_source.set_response(Kind::Source, |resource| {
        assert_eq!("url", resource.url);
        let mut response = Response::new();
        response.error = Some(Box::new(ResourceError::new(Reason::Other, "Failed by the test case")));
        Some(response)
    });

    let observer = Rc::new(StubStyleObserver::default());
    let failed = Rc::new(RefCell::new(false));
    {
        let failed = failed.clone();
        let handle = run_loop.handle();
        *observer.source_error.borrow_mut() = Some(Box::new(move |source, error| {
            assert_eq!("source", source.get_id());
            assert_eq!("Failed by the test case", error.to_string());
            *failed.borrow_mut() = true;
            handle.stop();
        }));
    }

    let source = VectorSource::new("source", UrlOrTileset::Url("url".to_owned()), None, None, VectorEncoding::Mapbox);
    source.set_observer(Some(observer.clone()));
    source.load_description(&file_source);

    run_loop.run();
    assert!(*failed.borrow());
}

// gtest: Source.LoadingCorrupt
#[test]
fn loading_corrupt() {
    let run_loop = RunLoop::new();
    let file_source = StubFileSource::new(ResponseType::Asynchronous);
    file_source.set_response(Kind::Source, |resource| {
        assert_eq!("url", resource.url);
        let mut response = Response::new();
        response.data = Some(Arc::new(b"CORRUPTED".to_vec()));
        Some(response)
    });

    let observer = Rc::new(StubStyleObserver::default());
    let failed = Rc::new(RefCell::new(None));
    {
        let failed = failed.clone();
        let handle = run_loop.handle();
        *observer.source_error.borrow_mut() = Some(Box::new(move |source, error| {
            assert_eq!("source", source.get_id());
            *failed.borrow_mut() = Some(error.to_string());
            handle.stop();
        }));
    }

    let source = VectorSource::new("source", UrlOrTileset::Url("url".to_owned()), None, None, VectorEncoding::Mapbox);
    source.set_observer(Some(observer.clone()));
    source.load_description(&file_source);

    run_loop.run();
    // C++ expects rapidjson's "Invalid value. at offset 0"; JSON syntax
    // errors carry serde_json's message in the port.
    let message = failed.borrow().clone().expect("source error");
    assert_eq!("expected value at line 1 column 1", message);
}

// gtest: Source.GeoJSonSourceUrlUpdate
#[test]
fn geojson_source_url_update() {
    let run_loop = RunLoop::new();
    let file_source = StubFileSource::new(ResponseType::Asynchronous);
    file_source.set_response(Kind::Source, |resource| {
        assert_eq!("http://source-url.ext", resource.url);
        let mut response = Response::new();
        response.data = Some(Arc::new(
            br#"{"geometry": {"type": "Point", "coordinates": [1.1, 1.1]}, "type": "Feature", "properties": {}}"#
                .to_vec(),
        ));
        Some(response)
    });

    let observer = Rc::new(StubStyleObserver::default());
    let loaded = Rc::new(RefCell::new(false));
    {
        let loaded = loaded.clone();
        let handle = run_loop.handle();
        *observer.source_loaded.borrow_mut() = Some(Box::new(move |_source| {
            // Should be called (test will hang if it doesn't)
            *loaded.borrow_mut() = true;
            handle.stop();
        }));
    }

    let source = GeoJSONSource::with_default_options("source");
    source.set_observer(Some(observer.clone()));

    // Load initial, so the source state will be loaded=true
    source.load_description(&file_source);

    // Schedule an update. C++ runs this from the loop (`loop.invoke`); the
    // source's callbacks only arrive while the loop runs either way.
    // Update the url
    source.set_url("http://source-url.ext");
    source.load_description(&file_source);

    run_loop.run();
    assert!(*loaded.borrow());
    assert!(source.loaded());
    assert!(source.impl_().get_data().and_then(|d| d.upgrade()).is_some());
}

// gtest: Source.GeoJSONSourceSynchronousUpdateOverrideReplacesTheOption
#[test]
fn geojson_source_synchronous_update_override_replaces_the_option() {
    let options = GeoJSONOptions { synchronous_update: true, ..Default::default() };
    let source = GeoJSONSource::new("source", Arc::new(options));
    assert!(source.is_update_synchronous());

    source.set_override_synchronous_update(false);
    assert!(!source.is_update_synchronous());

    source.set_override_synchronous_update(true);
    assert!(source.is_update_synchronous());

    let asynchronous = GeoJSONSource::with_default_options("asynchronous");
    assert!(!asynchronous.is_update_synchronous());
    asynchronous.set_override_synchronous_update(true);
    assert!(asynchronous.is_update_synchronous());
}

// gtest: Source.GeoJSONSourceKeepsTheStyleAttribution
#[test]
fn geojson_source_keeps_the_style_attribution() {
    let run_loop = RunLoop::new();
    let file_source = StubFileSource::new(ResponseType::Asynchronous);

    let mut error = Error::default();
    let value = parse_json(
        r#"{"type": "geojson", "attribution": "Fixture data", "data": {"type": "FeatureCollection", "features": []}}"#,
    );
    let converted = convert_source(&Convertible::from(&value), &mut error, "source");
    let converted = converted.unwrap_or_else(|| panic!("{}", error.message));
    let source = converted.downcast_ref::<GeoJSONSource>().expect("a GeoJSONSource");
    assert_eq!(Some("Fixture data".to_owned()), source.get_attribution());

    let observer = Rc::new(StubStyleObserver::default());
    source.set_observer(Some(observer.clone()));
    source.set_geojson(&geojson::parse(r#"{"type": "Point", "coordinates": [1.1, 1.1]}"#).unwrap()).unwrap();
    assert_eq!(Some("Fixture data".to_owned()), source.get_attribution());

    file_source.set_response(Kind::Source, |_| {
        let mut response = Response::new();
        response.data = Some(Arc::new(br#"{"type": "Point", "coordinates": [2.2, 2.2]}"#.to_vec()));
        Some(response)
    });
    let loaded = Rc::new(RefCell::new(false));
    {
        let loaded = loaded.clone();
        let handle = run_loop.handle();
        *observer.source_loaded.borrow_mut() = Some(Box::new(move |source| {
            assert_eq!(Some("Fixture data".to_owned()), source.get_attribution());
            *loaded.borrow_mut() = true;
            handle.stop();
        }));
    }
    source.set_url("url");
    source.load_description(&file_source);
    run_loop.run();
    assert!(*loaded.borrow());
}

/// A `FeatureCollection` of `count` points at 0, 0.
fn geojson_points(count: usize) -> Arc<Vec<u8>> {
    let feature = r#"{"type": "Feature", "properties": {}, "geometry": {"type": "Point", "coordinates": [0, 0]}}"#;
    let features = vec![feature; count].join(",");
    Arc::new(format!(r#"{{"type": "FeatureCollection", "features": [{features}]}}"#).into_bytes())
}

/// The number of features in tile 0/0/0 of `source`'s data.
fn geojson_source_feature_count(source: &GeoJSONSource) -> usize {
    let count = Rc::new(RefCell::new(0));
    if let Some(data) = source.impl_().get_data().and_then(|data| data.upgrade()) {
        let count = count.clone();
        data.get_tile(
            &CanonicalTileID::new(0, 0, 0),
            Box::new(move |features| *count.borrow_mut() = features.len()),
            true,
        );
    }
    *count.borrow()
}

/// Runs `run_loop` until the background build of a URL response has
/// replied. Nothing else posts to the loop in these tests.
fn finish_geojson_build(run_loop: &RunLoop) {
    while !run_loop.run_once() {
        run_loop.wait_for_work();
    }
}

/// A source observer that counts `on_source_loaded`.
fn counting_loads() -> (Rc<StubStyleObserver>, Rc<RefCell<u32>>) {
    let observer = Rc::new(StubStyleObserver::default());
    let loads = Rc::new(RefCell::new(0));
    {
        let loads = loads.clone();
        *observer.source_loaded.borrow_mut() = Some(Box::new(move |_| *loads.borrow_mut() += 1));
    }
    (observer, loads)
}

// gtest: Source.GeoJSONSourceDataReplacesBuildingURLData
#[test]
fn geojson_source_data_replaces_building_url_data() {
    let run_loop = RunLoop::new();
    let file_source = StubFileSource::new(ResponseType::Manual);
    file_source.set_response(Kind::Source, |_| {
        let mut response = Response::new();
        response.data = Some(geojson_points(1));
        Some(response)
    });
    let (observer, loads) = counting_loads();

    let source = GeoJSONSource::with_default_options("source");
    source.set_observer(Some(observer.clone()));
    source.set_url("url");
    source.load_description(&file_source);
    // The URL's data arrives and starts building in the background.
    file_source.respond_to_all();

    let data = geojson::parse(std::str::from_utf8(&geojson_points(2)).unwrap()).unwrap();
    source.set_geojson(&data).unwrap();
    finish_geojson_build(&run_loop);

    assert_eq!(0, *loads.borrow());
    assert_eq!(2, geojson_source_feature_count(&source));
}

// gtest: Source.GeoJSONSourceURLReplacesBuildingURLData
#[test]
fn geojson_source_url_replaces_building_url_data() {
    let run_loop = RunLoop::new();
    let file_source = StubFileSource::new(ResponseType::Manual);
    file_source.set_response(Kind::Source, |resource| {
        let mut response = Response::new();
        response.data = Some(geojson_points(if resource.url == "old" { 1 } else { 2 }));
        Some(response)
    });
    let (observer, loads) = counting_loads();

    let source = GeoJSONSource::with_default_options("source");
    source.set_observer(Some(observer.clone()));
    source.set_url("old");
    source.load_description(&file_source);
    // The old URL's data arrives and starts building in the background.
    file_source.respond_to_all();

    source.set_url("new");
    finish_geojson_build(&run_loop);

    assert_eq!(0, *loads.borrow());
    assert!(!source.loaded());

    // Request the new URL only now, so that its response can't arrive
    // before the old build's reply runs.
    source.load_description(&file_source);
    file_source.respond_to_all();
    finish_geojson_build(&run_loop);

    assert_eq!(1, *loads.borrow());
    assert_eq!(2, geojson_source_feature_count(&source));
}

fn as_vector(source: &dyn Source) -> &VectorSource {
    source.downcast_ref::<VectorSource>().expect("a VectorSource")
}

// gtest: Source.VectorSourceSetTiles
#[test]
fn vector_source_set_tiles() {
    let run_loop = RunLoop::new();
    let file_source = StubFileSource::new(ResponseType::Asynchronous);
    let observer = Rc::new(StubStyleObserver::default());
    let changed = Rc::new(RefCell::new(false));
    {
        let changed = changed.clone();
        *observer.source_changed.borrow_mut() = Some(Box::new(move |source| {
            assert_eq!(as_vector(source).get_tiles(), vec!["new".to_owned()]);
            *changed.borrow_mut() = true;
        }));
    }

    let tileset = Tileset { tiles: vec!["url".to_owned()], ..Default::default() };
    let source = VectorSource::new("source", UrlOrTileset::Tileset(tileset), None, None, VectorEncoding::Mapbox);
    source.set_observer(Some(observer.clone()));
    source.set_tiles(&["unused".to_owned()]); // make sure early setTiles does not crash
    source.load_description(&file_source);
    assert_eq!(source.get_tiles(), vec!["url".to_owned()]);
    source.set_tiles(&["new".to_owned()]);
    run_loop.run_once();
    assert!(*changed.borrow());
}

// gtest: Source.VectorSourceUrlSetTiles
#[test]
fn vector_source_url_set_tiles() {
    let run_loop = RunLoop::new();
    let file_source = StubFileSource::new(ResponseType::Asynchronous);
    file_source.set_response(Kind::Source, |resource| {
        assert_eq!("url", resource.url);
        let mut response = Response::new();
        response.data = Some(Arc::new(br#"{"tiles": ["tiles"]}"#.to_vec()));
        Some(response)
    });

    let observer = Rc::new(StubStyleObserver::default());
    *observer.source_loaded.borrow_mut() = Some(Box::new(|source| {
        assert_eq!(as_vector(source).get_tiles(), vec!["tiles".to_owned()]);
        as_vector(source).set_tiles(&["new".to_owned()]);
    }));
    let count = Rc::new(RefCell::new(0));
    {
        let count = count.clone();
        let handle = run_loop.handle();
        *observer.source_changed.borrow_mut() = Some(Box::new(move |source| {
            let n = *count.borrow();
            *count.borrow_mut() += 1;
            if n > 0 {
                assert_eq!(as_vector(source).get_tiles(), vec!["new".to_owned()]);
                handle.stop();
            }
        }));
    }

    let source = VectorSource::new("source", UrlOrTileset::Url("url".to_owned()), None, None, VectorEncoding::Mapbox);
    source.set_observer(Some(observer.clone()));
    source.set_tiles(&["unused".to_owned()]);
    source.load_description(&file_source);
    run_loop.run();
    assert_eq!(*count.borrow(), 2);
}

/// Loads `source`'s description from a TileJSON with `"minzoom": 4, "maxzoom": 12` and returns
/// the tileset it has when it's loaded (the C++ tests read it in `sourceLoaded`).
fn tileset_loaded_from_tilejson(source: &dyn Source) -> Tileset {
    let run_loop = RunLoop::new();
    let file_source = StubFileSource::new(ResponseType::Asynchronous);
    file_source.set_response(Kind::Source, |resource| {
        assert_eq!("url", resource.url);
        let mut response = Response::new();
        response.data =
            Some(Arc::new(br#"{ "tilejson": "2.1.0", "tiles": [ "tiles" ], "minzoom": 4, "maxzoom": 12 }"#.to_vec()));
        Some(response)
    });

    let tileset = Rc::new(RefCell::new(None));
    let observer = Rc::new(StubStyleObserver::default());
    {
        let tileset = tileset.clone();
        let handle = run_loop.handle();
        *observer.source_loaded.borrow_mut() = Some(Box::new(move |source| {
            let impl_ = source.base_impl();
            let impl_ = impl_.downcast_ref::<crate::sources::tile_source::Impl>().expect("a TileSource");
            *tileset.borrow_mut() = impl_.tileset.clone();
            handle.stop();
        }));
    }

    source.set_observer(Some(observer.clone()));
    source.load_description(&file_source);
    run_loop.run();
    tileset.take().expect("a tileset")
}

// gtest: Source.RasterDEMInlineZoomOverridesTileJSON
#[test]
fn raster_dem_inline_zoom_overrides_tile_json() {
    let source = RasterDEMSource::new(
        "source",
        UrlOrTileset::Url("url".to_owned()),
        512,
        Some(SourceOptions {
            raster_encoding: Some(RasterEncoding::Terrarium),
            maxzoom: Some(15),
            ..Default::default()
        }),
    );
    let tileset = tileset_loaded_from_tilejson(&source);
    // The inline maxzoom wins over the TileJSON's; minzoom was not given inline, so the TileJSON
    // value is kept.
    assert_eq!(4, tileset.zoom_range.min);
    assert_eq!(15, tileset.zoom_range.max);
    assert_eq!(Some(RasterEncoding::Terrarium), tileset.raster_encoding);
}

// gtest: Source.RasterInlineZoomOverridesTileJSON
#[test]
fn raster_inline_zoom_overrides_tile_json() {
    let source =
        RasterSource::new("source", UrlOrTileset::Url("url".to_owned()), 512, SourceType::Raster, None, Some(16));
    let tileset = tileset_loaded_from_tilejson(&source);
    // The inline maxzoom wins; minzoom was not given inline, so the TileJSON's is kept.
    assert_eq!(4, tileset.zoom_range.min);
    assert_eq!(16, tileset.zoom_range.max);
}

// gtest: Source.RasterDEMTileJSONZoomKeptWithoutInlineOverride
#[test]
fn raster_dem_tile_json_zoom_kept_without_inline_override() {
    let source = RasterDEMSource::new("source", UrlOrTileset::Url("url".to_owned()), 512, None);
    let tileset = tileset_loaded_from_tilejson(&source);
    assert_eq!(4, tileset.zoom_range.min);
    assert_eq!(12, tileset.zoom_range.max);
}

// gtest: Source.ImageSourceImageUpdate
#[test]
fn image_source_image_update() {
    let run_loop = RunLoop::new();
    let file_source = StubFileSource::new(ResponseType::Asynchronous);
    file_source.set_global_response(|resource| {
        assert_eq!("http://url", resource.url);
        let mut response = Response::new();
        response.data = Some(Arc::new(super::util::read_file_bytes("test/fixtures/image/no_profile.png")));
        Some(response)
    });

    let observer = Rc::new(StubStyleObserver::default());
    let changed = Rc::new(RefCell::new(false));
    {
        let changed = changed.clone();
        let handle = run_loop.handle();
        *observer.source_changed.borrow_mut() = Some(Box::new(move |_source| {
            // Should be called (test will hang if it doesn't)
            *changed.borrow_mut() = true;
            handle.stop();
        }));
    }
    let coords = [LatLng::default(); 4];

    let source = ImageSource::new("source", coords);
    source.set_url("http://url");
    source.set_observer(Some(observer.clone()));

    // Load initial, so the source state will be loaded=true
    source.load_description(&file_source);
    let mut rgba = PremultipliedImage::new(Size::new(1, 1));
    rgba.data[0] = 255;
    rgba.data[1] = 254;
    rgba.data[2] = 253;
    rgba.data[3] = 0;

    // Schedule an update (`test.loop.invoke`; the source is not Send, so an
    // AsyncTask on this loop stands in for it).
    let update = {
        let source = source.clone();
        let rgba = RefCell::new(Some(rgba));
        mln_actor::AsyncTask::new(move || {
            // Update the url
            if let Some(rgba) = rgba.borrow_mut().take() {
                source.set_image(rgba);
            }
        })
    };
    update.send();

    run_loop.run();
    assert!(*changed.borrow());
}

/// The source the style JSON `json` converts to.
fn converted_source(json: &str) -> Result<Box<dyn Source>, String> {
    let mut error = Error::default();
    convert_source(&Convertible::from(&parse_json(json)), &mut error, "source").ok_or(error.message)
}

// A vector source's tileset takes the style's sparse, over a TileJSON or inline.
#[test]
fn vector_sources_take_the_style_sparse() {
    let source =
        converted_source(r#"{"type": "vector", "url": "url", "sparse": true}"#).unwrap_or_else(|e| panic!("{e}"));
    assert!(tileset_loaded_from_tilejson(&*source).sparse);

    let source = converted_source(r#"{"type": "vector", "url": "url"}"#).unwrap();
    assert!(!tileset_loaded_from_tilejson(&*source).sparse);

    let source = converted_source(r#"{"type": "vector", "tiles": ["tiles"], "sparse": true}"#).unwrap();
    source.load_description(&StubFileSource::new(ResponseType::Asynchronous));
    let impl_ = source.base_impl();
    let impl_ = impl_.downcast_ref::<crate::sources::tile_source::Impl>().expect("a TileSource");
    assert!(impl_.tileset.as_ref().unwrap().sparse);

    let Err(error) = converted_source(r#"{"type": "vector", "url": "url", "sparse": "yes"}"#) else {
        panic!("an invalid sparse converts");
    };
    assert!(error.contains("sparse"), "{error}");
}
