// Ports the style-side tests of test/style/source.test.cpp (the ones that
// need no RenderSource; those are in mln_renderer/tests/source_test.rs).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use mln_actor::RunLoop;
use mln_storage::resource_options::ResourceOptions;
use mln_storage::response::{Error as ResponseError, Reason};
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_storage::{FileSource, Kind, Response};
use mln_style_types::SourceType;
use mln_util::exception::ExceptionPtr;
use mln_util::geo::LatLng;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use mln_util::tile_server_options::TileServerOptions;
use mln_util::tileset::{RasterEncoding, Tileset, VectorEncoding};

use crate::conversion::tileset::SourceOptions;
use crate::observer::SourceObserver;
use crate::source::Source;
use crate::sources::geojson_source::GeoJSONSource;
use crate::sources::image_source::ImageSource;
use crate::sources::tile_source::{RasterDEMSource, RasterSource, UrlOrTileset, VectorSource};

/// `StubStyleObserver` (test/src/mln/test/stub_style_observer.hpp), the
/// SourceObserver half.
#[derive(Default)]
struct StubStyleObserver {
    source_loaded: RefCell<Option<Box<dyn Fn(&dyn Source)>>>,
    source_changed: RefCell<Option<Box<dyn Fn(&dyn Source)>>>,
    source_error: RefCell<Option<Box<dyn Fn(&dyn Source, ExceptionPtr)>>>,
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

    fn on_source_error(&self, source: &dyn Source, error: ExceptionPtr) {
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
        response.error = Some(Box::new(ResponseError::new(Reason::Other, "Failed by the test case")));
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

fn as_vector(source: &dyn Source) -> &VectorSource {
    source.as_any().downcast_ref::<VectorSource>().expect("a VectorSource")
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
            let impl_ = impl_.as_any().downcast_ref::<crate::sources::tile_source::Impl>().expect("a TileSource");
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

/// Loads `source`'s description, with `tile_server_options`, from a TileJSON
/// whose only tile URL is `tile_url`; returns the URL the TileJSON was
/// requested from and the tile URLs the source has once it's loaded.
fn load_tilejson_tiles(
    source: &dyn Source,
    tile_server_options: TileServerOptions,
    tile_url: &str,
) -> (String, Vec<String>) {
    let run_loop = RunLoop::new();
    let file_source = StubFileSource::new(ResponseType::Asynchronous);
    file_source.set_resource_options(ResourceOptions::new().with_tile_server_options(tile_server_options));
    let requested = Rc::new(RefCell::new(String::new()));
    {
        let requested = requested.clone();
        let tile_json = format!(r#"{{ "tilejson": "2.1.0", "tiles": [ "{tile_url}" ] }}"#);
        file_source.set_response(Kind::Source, move |resource| {
            *requested.borrow_mut() = resource.url.clone();
            let mut response = Response::new();
            response.data = Some(Arc::new(tile_json.clone().into_bytes()));
            Some(response)
        });
    }

    let tiles = Rc::new(RefCell::new(None));
    let observer = Rc::new(StubStyleObserver::default());
    {
        let tiles = tiles.clone();
        let handle = run_loop.handle();
        *observer.source_loaded.borrow_mut() = Some(Box::new(move |source| {
            let impl_ = source.base_impl();
            let impl_ = impl_.as_any().downcast_ref::<crate::sources::tile_source::Impl>().expect("a TileSource");
            *tiles.borrow_mut() = impl_.tileset.as_ref().map(|tileset| tileset.tiles.clone());
            handle.stop();
        }));
    }

    source.set_observer(Some(observer.clone()));
    source.load_description(&file_source);
    run_loop.run();
    (requested.take(), tiles.take().expect("a tileset"))
}

#[test]
fn vector_source_canonicalizes_mapbox_tile_urls() {
    let source = VectorSource::new(
        "source",
        UrlOrTileset::Url("mapbox://mapbox.streets".to_owned()),
        None,
        None,
        VectorEncoding::Mapbox,
    );
    let (requested, tiles) = load_tilejson_tiles(
        &source,
        TileServerOptions::mapbox_configuration(),
        "https://a.tiles.mapbox.com/v4/mapbox.streets/{z}/{x}/{y}.vector.pbf?access_token=key",
    );
    assert_eq!("mapbox://mapbox.streets", requested);
    assert_eq!(vec!["mapbox://tiles/mapbox.streets/{z}/{x}/{y}.vector.pbf".to_owned()], tiles);
}

#[test]
fn raster_sources_canonicalize_mapbox_tiles_by_tile_size() {
    let tile_url = "https://a.tiles.mapbox.com/v4/mapbox.satellite/{z}/{x}/{y}.png?access_token=key";
    for (source_type, tile_size, expected) in [
        (SourceType::Raster, 256, "mapbox://tiles/mapbox.satellite/{z}/{x}/{y}{ratio}.png"),
        (SourceType::Raster, 512, "mapbox://tiles/mapbox.satellite/{z}/{x}/{y}@2x.png"),
    ] {
        let source = RasterSource::new(
            "source",
            UrlOrTileset::Url("mapbox://mapbox.satellite".to_owned()),
            tile_size,
            source_type,
            None,
            None,
        );
        let (requested, tiles) = load_tilejson_tiles(&source, TileServerOptions::mapbox_configuration(), tile_url);
        assert_eq!("mapbox://mapbox.satellite", requested);
        assert_eq!(vec![expected.to_owned()], tiles);
    }

    let source = RasterDEMSource::new("source", UrlOrTileset::Url("mapbox://mapbox.terrain-rgb".to_owned()), 256, None);
    let (_, tiles) = load_tilejson_tiles(
        &source,
        TileServerOptions::mapbox_configuration(),
        "https://a.tiles.mapbox.com/v4/mapbox.terrain-rgb/{z}/{x}/{y}.pngraw?access_token=key",
    );
    assert_eq!(vec!["mapbox://tiles/mapbox.terrain-rgb/{z}/{x}/{y}{ratio}.pngraw".to_owned()], tiles);
}

/// The source URL is canonicalized with any tile server, the TileJSON's tile
/// URLs only with the "mapbox" scheme alias.
#[test]
fn vector_source_keeps_maptiler_tile_urls() {
    let tile_url = "https://api.maptiler.com/tiles/v3/{z}/{x}/{y}.pbf?key=abc";
    let source = VectorSource::new(
        "source",
        UrlOrTileset::Url("https://api.maptiler.com/tiles/streets/tiles.json?key=abc".to_owned()),
        None,
        None,
        VectorEncoding::Mapbox,
    );
    let (requested, tiles) = load_tilejson_tiles(&source, TileServerOptions::map_tiler_configuration(), tile_url);
    assert_eq!("maptiler://sources/streets", requested);
    assert_eq!(vec![tile_url.to_owned()], tiles);
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
