// A tile's layout in two stages (layout/tile_layout.rs), and the worker that
// runs them.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use feature::{Feature, Identifier, PropertyMap};
use geo_types::{Geometry, LineString, Polygon};
use mln_map_types::MapMode;
use mln_style::conversion::{Convertible, Error as ConversionError, convert_layer};
use mln_style::layer_properties::LayerProperties;
use mln_style::renderer::property_evaluation_parameters::PropertyEvaluationParameters;
use mln_style_types::image::{ImageMap, ImagePositions, ImageType};
use mln_text::glyph::{GlyphID, GlyphMap, GlyphPositions};
use mln_tile::TileData;
use mln_tile_id::OverscaledTileID;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_vec_map::VecSet;
use parking_lot::Mutex;
use serde_json::json;

use crate::gfx::dynamic_texture_atlas::ImageVersionMap;
use crate::layermanager;
use crate::layout::tile_layout::{LayoutInput, LayoutParams, ResolvedDependencies, build, discover};
use crate::text::glyph_manager::HBShapeResults;
use crate::tile::geometry_tile_worker::{GeometryTileWorker, TileMessage, WorkerMessage};
use crate::tile::tile_worker_transport::TileEndpoint;

/// A layer from its style JSON on source `s`, evaluated at zoom 0 as the
/// `RenderOrchestrator` evaluates it.
fn layer(mut json: serde_json::Value) -> Arc<dyn LayerProperties> {
    json["source"] = json!("s");
    let mut error = ConversionError::default();
    let layer = convert_layer(&Convertible::from(&json), &mut error).unwrap_or_else(|| panic!("{}", error.message));
    let render_layer = layermanager::create_render_layer(&layer.base_impl()).unwrap();
    render_layer.borrow_mut().evaluate(&PropertyEvaluationParameters::new(0.0));
    Arc::clone(&render_layer.borrow().base().evaluated_properties)
}

/// Two squares.
fn data() -> TileData {
    let square = |x: i16| Feature {
        geometry: Some(Geometry::Polygon(Polygon::new(
            LineString::from(vec![(x, 100), (x + 100, 100), (x + 100, 200), (x, 200), (x, 100)]),
            vec![],
        ))),
        properties: Arc::new(PropertyMap::new()),
        id: Identifier::Null,
    };
    TileData::Features(Arc::new(vec![square(100), square(1000)]))
}

fn input<'a>(
    data: &'a TileData,
    layers: &'a [Arc<dyn LayerProperties>],
    available: &'a VecSet<String>,
) -> LayoutInput<'a> {
    LayoutInput {
        id: OverscaledTileID::from_zxy(0, 0, 0),
        data: Some(data),
        layers,
        global_state: None,
        font_faces: None,
        available_images: available,
        params: LayoutParams {
            mode: MapMode::Static,
            pixel_ratio: 1.0,
            show_collision_boxes: false,
            subdivision_granularity: SubdivisionGranularitySetting::none(),
            first_load: true,
            capture_rendered_features: false,
        },
    }
}

#[test]
fn discover_records_what_the_layouts_need() {
    let layers = [
        layer(json!({"id": "outline", "type": "line"})),
        layer(json!({"id": "dots", "type": "circle"})),
        layer(json!({"id": "patterned", "type": "fill", "paint": {"fill-pattern": "stripes"}})),
        layer(json!({"id": "labels", "type": "symbol", "layout": {
            "text-field": "ab",
            "text-font": ["Test"],
            "icon-image": ["coalesce", ["image", "missing"], ["image", "marker"]],
        }})),
    ];
    let data = data();
    let available = VecSet::from_iter(["marker".to_owned()]);
    let discovered = discover(&input(&data, &layers, &available), &AtomicBool::new(false)).unwrap().unwrap();

    let glyphs = &discovered.dependencies.glyphs.glyphs;
    assert_eq!(glyphs.keys().collect::<Vec<_>>(), [&vec!["Test".to_owned()]]);
    assert_eq!(
        glyphs[&vec!["Test".to_owned()]].iter().copied().collect::<Vec<_>>(),
        [GlyphID::from(u16::from(b'a')), GlyphID::from(u16::from(b'b'))]
    );
    let mut images: Vec<(&str, ImageType)> =
        discovered.dependencies.images.iter().map(|(id, type_)| (id.as_str(), *type_)).collect();
    images.sort_by_key(|(id, _)| *id);
    assert_eq!(images, [("marker", ImageType::Icon), ("missing", ImageType::Icon), ("stripes", ImageType::Pattern)]);
    assert!(discovered.has_waiting_layouts());
}

#[test]
fn build_indexes_the_layouts_that_waited_last() {
    let layers = [
        layer(json!({"id": "patterned", "type": "fill", "paint": {"fill-pattern": "stripes"}})),
        layer(json!({"id": "outline", "type": "line"})),
    ];
    let data = data();
    let available = VecSet::new();
    let input = input(&data, &layers, &available);
    let obsolete = AtomicBool::new(false);
    let discovered = discover(&input, &obsolete).unwrap().unwrap();
    let resolved = ResolvedDependencies {
        glyphs: &GlyphMap::new(),
        glyph_positions: &GlyphPositions::new(),
        icons: &ImageMap::new(),
        icon_positions: &ImagePositions::default(),
        pattern_positions: &ImagePositions::default(),
        shaping: &HBShapeResults::new(),
    };
    let built = build(discovered, &input, &resolved, &obsolete).unwrap().unwrap();

    assert_eq!(built.feature_index.leaders(), ["outline", "outline", "patterned", "patterned"]);
    assert!(built.render_data.contains_key("outline"));
}

#[test]
fn discover_stops_when_obsolete() {
    let layers = [layer(json!({"id": "outline", "type": "line"}))];
    let data = data();
    let available = VecSet::new();
    assert!(discover(&input(&data, &layers, &available), &AtomicBool::new(true)).unwrap().is_none());
}

/// Where a test worker's messages go.
#[derive(Clone, Default)]
struct Replies(Arc<Mutex<Vec<TileMessage>>>);

impl TileEndpoint for Replies {
    fn send(&self, message: TileMessage) {
        self.0.lock().push(message);
    }
}

/// A symbol layer whose only feature has no icon, but whose `icon-image`
/// asked for an unavailable image on the way: its layout waits for that
/// image and nothing else.
#[test]
fn a_layout_waiting_only_for_unavailable_images_finishes() {
    let replies = Replies::default();
    let mut worker = GeometryTileWorker::new(
        None,
        Box::new(replies.clone()),
        OverscaledTileID::from_zxy(0, 0, 0),
        Arc::new(AtomicBool::new(false)),
        MapMode::Static,
        1.0,
        false,
        None,
        None,
        false,
    );
    let icon_image = json!([
        "case",
        ["==", ["to-string", ["coalesce", ["image", "missing"], ["image", "marker"]]], "marker"],
        "",
        "marker"
    ]);
    let available = Arc::new(VecSet::from_iter(["marker".to_owned()]));
    worker.handle(WorkerMessage::SetLayers {
        layers: vec![layer(json!({"id": "icons", "type": "symbol", "layout": {"icon-image": icon_image}}))],
        global_state: None,
        available_images: Arc::clone(&available),
        subdivision_granularity: SubdivisionGranularitySetting::none(),
        pixel_ratio: 1.0,
        correlation_id: 1,
    });
    worker.handle(WorkerMessage::SetData { data: Some(data()), available_images: available, correlation_id: 1 });

    let requests = std::mem::take(&mut *replies.0.lock());
    let [TileMessage::GetImages((images, image_correlation_id))] = requests.as_slice() else {
        panic!("expected one image request");
    };
    assert_eq!(images.keys().collect::<Vec<_>>(), ["missing"]);
    worker.handle(WorkerMessage::ImagesAvailable {
        icon_map: ImageMap::new(),
        pattern_map: ImageMap::new(),
        version_map: ImageVersionMap::default(),
        image_correlation_id: *image_correlation_id,
    });
    assert!(matches!(replies.0.lock().as_slice(), [TileMessage::Layout { correlation_id: 1, .. }]));
}
