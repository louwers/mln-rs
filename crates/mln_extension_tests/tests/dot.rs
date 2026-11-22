//! The `dot` layer type through the facade: a runtime whose registry adds
//! it to the built-ins draws, queries and edits a style with a dot layer
//! over a background, offscreen.

use std::sync::Arc;
use std::time::{Duration, Instant};

use mln_extension_tests::dot::{Dot, DotLayer};
use mln_rs::mln_render::{Registry, RegistryBuilder};
use mln_rs::mln_util::geo::ScreenCoordinate;
use mln_rs::mln_util::size::Size;
use mln_rs::{
    AttachOptions, Completion, DemandMode, Error, FrameDemand, FrameDisposition, Map, MapOptions, RenderSession,
    RenderTarget, RenderedQueryGeometry, RenderedQueryOptions, Runtime, RuntimeOptions,
};

const SIZE: Size = Size { width: 64, height: 64 };
const WHITE: [u8; 4] = [255, 255, 255, 255];
const RED: [u8; 4] = [255, 0, 0, 255];

/// A red dot of radius 8 at the center of a white map, and another off to
/// the east, out of the frame.
const STYLE: &str = r##"{"version":8,"transition":{"duration":0},
    "sources":{"points":{"type":"geojson","data":{"type":"FeatureCollection","features":[
        {"type":"Feature","id":1,"properties":{"size":8},"geometry":{"type":"Point","coordinates":[0,0]}},
        {"type":"Feature","id":2,"properties":{"size":8},"geometry":{"type":"Point","coordinates":[90,0]}}]}}},
    "layers":[
        {"id":"background","type":"background","paint":{"background-color":"#ffffff"}},
        {"id":"dots","type":"dot","source":"points",
         "paint":{"dot-color":"#ff0000","dot-radius":["get","size"]}}]}"##;

fn registry() -> Arc<Registry> {
    let mut builder = RegistryBuilder::new();
    mln_rs::register_builtins(&mut builder).unwrap();
    builder.add_layer_type::<DotLayer>().unwrap();
    builder.build()
}

/// A runtime of `registry()`, a map of `SIZE` with `STYLE`, and an offscreen
/// session of it that shows the style loaded.
struct Fixture {
    session: RenderSession,
    map: Map,
    runtime: Runtime,
}

impl Fixture {
    fn new() -> Fixture {
        let runtime = Runtime::new(&RuntimeOptions::default(), registry()).unwrap();
        let options =
            MapOptions { map: mln_rs::map::MapCoreOptions { size: SIZE, ..Default::default() }, ..Default::default() };
        let (map, created) = runtime.create_map(options).unwrap();
        created.wait().unwrap();
        wait(map.set_style_json(STYLE));
        let (session, attached) =
            map.attach(RenderTarget::Offscreen { size: SIZE, device: None }, AttachOptions::default()).unwrap();
        attached.wait().unwrap();
        let fixture = Fixture { session, map, runtime };
        fixture.render_loaded();
        fixture
    }

    /// Renders one frame, whether or not the map changed.
    fn render(&self) -> mln_rs::FrameResult {
        self.session.demand_frame(FrameDemand::new(DemandMode::Force, 0)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(result) = self.session.drain_frame_results().unwrap().into_iter().next() {
                assert_eq!(FrameDisposition::Rendered, result.disposition);
                return result;
            }
            assert!(Instant::now() < deadline, "no frame rendered");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Renders until a frame shows every source loaded.
    fn render_loaded(&self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while {
            let frame = self.render();
            frame.mode != mln_rs::mln_render::RenderMode::Full || frame.needs_repaint
        } {
            assert!(Instant::now() < deadline, "the map never loaded");
        }
    }

    /// The premultiplied RGBA of the pixel `x` pixels right of the center of
    /// the last frame.
    fn pixel(&self, x: u32) -> [u8; 4] {
        let image = wait(self.session.read_texture());
        let offset = ((SIZE.height / 2 * SIZE.width + SIZE.width / 2 + x) * 4) as usize;
        image.data[offset..offset + 4].try_into().unwrap()
    }

    /// How many pixels east of the center the longitude `longitude` is, on
    /// the equator at zoom 0.
    fn east_pixels(&self, longitude: f64) -> u32 {
        (longitude / 360.0 * 512.0).round() as u32
    }

    /// The ids of the features drawn at the pixel `x` pixels right of the
    /// center, by layer.
    fn hits(&self, x: f64) -> Vec<(String, mln_rs::feature::Identifier)> {
        let center = f64::from(SIZE.width) / 2.0;
        let point = RenderedQueryGeometry::Point(ScreenCoordinate::new(center + x, center));
        let features = wait(self.session.query_rendered_features(point, RenderedQueryOptions::default()));
        features.into_iter().map(|feature| (feature.layer, feature.id)).collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        wait(self.session.release());
        wait(self.map.release());
        if let Ok(released) = self.runtime.release() {
            let _ = released.wait();
        }
    }
}

fn wait<T: Send + 'static>(completion: Result<Completion<T>, Error>) -> T {
    completion.unwrap().wait().unwrap()
}

#[test]
fn a_dot_draws_over_the_background() {
    let fixture = Fixture::new();
    assert_eq!(RED, fixture.pixel(0));
    assert_eq!(RED, fixture.pixel(6));
    assert_eq!(WHITE, fixture.pixel(12));
}

#[test]
fn a_query_hits_a_dot_as_drawn() {
    let fixture = Fixture::new();
    let dot = vec![("dots".to_owned(), mln_rs::feature::Identifier::Uint(1))];
    assert_eq!(dot, fixture.hits(0.0));
    assert_eq!(dot, fixture.hits(6.0));
    assert_eq!(Vec::<(String, mln_rs::feature::Identifier)>::new(), fixture.hits(12.0));
}

#[test]
fn an_edit_of_the_layers_data_scales_its_dots() {
    let fixture = Fixture::new();
    wait(fixture.map.edit_style_layer_data::<Dot>("dots", |data| {
        data.scale = 2.0;
        Ok(())
    }));
    fixture.render();
    assert_eq!(RED, fixture.pixel(12));
    assert_eq!(1, fixture.hits(12.0).len());

    // The renderer's layer has the data.
    let scale =
        wait(fixture.session.query_rendered_layer::<DotLayer, _>("dots", |_, values| Ok(values.style.data.scale)));
    assert_eq!(2.0, scale);

    // A style load keeps it.
    wait(fixture.map.set_style_json(STYLE));
    fixture.render_loaded();
    assert_eq!(RED, fixture.pixel(12));

    // The data of another type's layer is no dot's.
    let error = fixture.map.edit_style_layer_data::<Dot>("background", |_| Ok(())).unwrap().wait().unwrap_err();
    assert_eq!(mln_rs::mln_util::ErrorKind::NotFound, error.kind);
}

#[test]
fn a_removed_layer_goes_and_a_new_source_draws_its_own_data() {
    let fixture = Fixture::new();
    let layer =
        r##"{"id":"dots","type":"dot","source":"points","paint":{"dot-color":"#ff0000","dot-radius":["get","size"]}}"##;
    wait(fixture.map.remove_style_layer("dots"));
    fixture.render();
    assert_eq!(WHITE, fixture.pixel(0));
    let error = fixture.session.query_rendered_layer::<DotLayer, _>("dots", |_, _| Ok(())).unwrap().wait().unwrap_err();
    assert_eq!(mln_rs::mln_util::ErrorKind::NotFound, error.kind);

    wait(fixture.map.add_style_layer_json(layer, None));
    fixture.render_loaded();
    assert_eq!(RED, fixture.pixel(0));

    // The layer's source changes to one whose only point is east of the
    // center.
    let east = r#"{"type":"geojson","data":{"type":"Feature","properties":{"size":4},
        "geometry":{"type":"Point","coordinates":[5,0]}}}"#;
    wait(fixture.map.add_style_source_json("east", east));
    wait(fixture.map.set_style_layer_property("dots", "source", "\"east\""));
    fixture.render_loaded();
    assert_eq!(WHITE, fixture.pixel(0));
    let east_px = fixture.east_pixels(5.0);
    assert_eq!(RED, fixture.pixel(east_px));
}
