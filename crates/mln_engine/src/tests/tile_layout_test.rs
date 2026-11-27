// A tile's layout in two stages (layout/tile_layout.rs), and the job that
// runs them.

use std::sync::Arc;

use geo_types::{Geometry, LineString, Polygon};
use mln_style::collections::VecSet;
use mln_style::conversion::{Convertible, Error as ConversionError};
use mln_style::feature::{Feature, Identifier, PropertyMap};
use mln_style::layer::convert_layer;
use mln_style::property::evaluation::PropertyEvaluationParameters;
use mln_style::subdivision_granularity::SubdivisionGranularitySetting;
use mln_style::tile_id::CanonicalTileID;
use serde_json::json;

use crate::image::image_store::ImageType;
use crate::layer::layer_type::ErasedEvaluatedLayer;
use crate::layer::tile_layout::{LayoutInput, LayoutParams, Resolved, build, discover};
use crate::map::MapMode;
use crate::text::glyph::GlyphID;
use crate::tile::{OverscaledTileID, TileData};
use crate::worker::job::{DataInput, DataKey, Job, JobId, JobResult, LayoutJob, Run, SourceKey, TileKey, run_job};
use crate::worker::tile_workers::InlineWorkers;

/// A layer from its style JSON on source `s`, evaluated at zoom 0 as the
/// `RenderOrchestrator` evaluates it.
fn layer(mut json: serde_json::Value) -> Arc<dyn ErasedEvaluatedLayer> {
    json["source"] = json!("s");
    let mut error = ConversionError::default();
    let layer = convert_layer(crate::tests::registry().style(), &Convertible::from(&json), &mut error)
        .unwrap_or_else(|| panic!("{}", error.message));
    let mut render_layer = crate::tests::render_layer(layer.style());
    render_layer.evaluate(&PropertyEvaluationParameters::new(0.0));
    Arc::clone(&render_layer.base().evaluated_properties)
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

fn params() -> LayoutParams {
    LayoutParams {
        mode: MapMode::Static,
        pixel_ratio: 1.0,
        subdivision_granularity: SubdivisionGranularitySetting::none(),
        first_load: true,
    }
}

fn input<'a>(
    data: &'a TileData,
    layers: &'a [Arc<dyn ErasedEvaluatedLayer>],
    available: &'a VecSet<String>,
) -> LayoutInput<'a> {
    LayoutInput {
        id: OverscaledTileID::from_zxy(0, 0, 0),
        data: Some(data),
        layers,
        global_state: None,
        font_faces: None,
        glyph_pbfs: true,
        available_images: available,
        params: params(),
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
    let discovered = discover(&input(&data, &layers, &available)).unwrap();

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
    let discovered = discover(&input).unwrap();
    let built = build(discovered, &input, &Resolved::default()).unwrap();

    assert_eq!(built.feature_index.finish().leaders(), ["outline", "outline", "patterned", "patterned"]);
    assert!(built.buckets.contains_key("outline"));
}

/// The result of a job that lays tile 0/0/0 out with `layers`, `data` and
/// `resolved`.
fn run(layers: &[Arc<dyn ErasedEvaluatedLayer>], available: &[&str], resolved: Resolved) -> JobResult {
    let tile = TileKey { source: SourceKey(1), id: OverscaledTileID::from_zxy(0, 0, 0) };
    let key = DataKey { source: SourceKey(1), tile: CanonicalTileID::new(0, 0, 0), load: 1 };
    let job = LayoutJob {
        style: crate::tests::style(layers),
        available_images: Arc::new(available.iter().map(|id| (*id).to_owned()).collect()),
        data: DataInput::Full { key, data: Some(data()) },
        params: params(),
        resolved: Arc::new(resolved),
    };
    let run = Run { id: JobId { tile, generation: 1 }, job: Job::Layout(job), superseded: Arc::default() };
    InlineWorkers::run(run).result
}

/// A symbol layer whose only feature has no icon, but whose `icon-image`
/// asked for an unavailable image on the way: its first job needs that
/// image and nothing else, and a job that knows it missing lays out.
#[test]
fn a_layout_waiting_only_for_unavailable_images_finishes() {
    let icon_image = json!([
        "case",
        ["==", ["to-string", ["coalesce", ["image", "missing"], ["image", "marker"]]], "marker"],
        "",
        "marker"
    ]);
    let layers = [layer(json!({"id": "icons", "type": "symbol", "layout": {"icon-image": icon_image}}))];
    let JobResult::NeedsDeps(needs) = run(&layers, &["marker"], Resolved::default()) else {
        panic!("expected the dependencies");
    };
    assert!(needs.glyphs.glyphs.is_empty());
    assert_eq!(needs.images.keys().collect::<Vec<_>>(), ["missing"]);
    let resolved = Resolved { missing_images: needs.images.keys().cloned().collect(), ..Resolved::default() };
    assert!(matches!(run(&layers, &["marker"], resolved), JobResult::Laid(_)));
}

/// A layout that uses no glyphs or images lays out in its first job, and
/// reports that it needs none.
#[test]
fn a_layout_without_dependencies_takes_one_job() {
    let layers = [layer(json!({"id": "outline", "type": "line"}))];
    let JobResult::Laid(output) = run(&layers, &[], Resolved::default()) else {
        panic!("expected the layout");
    };
    assert!(output.deps.glyphs.glyphs.is_empty() && output.deps.images.is_empty());
    assert!(output.result.layer_render_data.contains_key("outline"));
}

/// A job lays out only its tile's zoom's layers.
#[test]
fn a_job_lays_out_the_layers_of_its_zoom() {
    let layers =
        [layer(json!({"id": "outline", "type": "line"})), layer(json!({"id": "near", "type": "line", "minzoom": 4}))];
    let JobResult::Laid(output) = run(&layers, &[], Resolved::default()) else {
        panic!("expected the layout");
    };
    assert!(output.result.layer_render_data.contains_key("outline"));
    assert!(!output.result.layer_render_data.contains_key("near"));
}

/// A resume of the generation of a first try that answered `NeedsDeps`
/// builds from what the first try discovered, when the memo kept it; a job
/// with other inputs discovers again.
#[test]
fn a_resume_builds_from_the_memo() {
    use crate::worker::job::{DiscoveryMemo, JobTimes, SourceStyle};

    let memo = DiscoveryMemo::new(4);
    let style = crate::tests::style(&[layer(json!({"id": "icons", "type": "symbol", "layout": {"icon-image": "a"}}))]);
    let images = Arc::new(VecSet::new());
    let job = |style: &Arc<SourceStyle>, images: &Arc<VecSet<String>>, resolved: Resolved, generation| {
        let tile = TileKey { source: SourceKey(1), id: OverscaledTileID::from_zxy(0, 0, 0) };
        let key = DataKey { source: SourceKey(1), tile: CanonicalTileID::new(0, 0, 0), load: 1 };
        let job = LayoutJob {
            style: Arc::clone(style),
            available_images: Arc::clone(images),
            data: DataInput::Full { key, data: Some(data()) },
            params: params(),
            resolved: Arc::new(resolved),
        };
        Run { id: JobId { tile, generation }, job: Job::Layout(job), superseded: Arc::default() }
    };
    let run = |run: Run| {
        let (sender, out) = std::sync::mpsc::channel();
        run_job(run, None, Some(&memo), move |reply, times: JobTimes| {
            let _ = sender.send((reply.result, times));
        });
        out.try_recv().expect("a reply before returning")
    };
    let missing = || Resolved { missing_images: VecSet::from_iter(["a".to_owned()]), ..Resolved::default() };

    let (JobResult::NeedsDeps(_), first) = run(job(&style, &images, Resolved::default(), 1)) else {
        panic!("expected the dependencies");
    };
    assert!(!first.discover.is_zero());
    let (JobResult::Laid(_), resume) = run(job(&style, &images, missing(), 1)) else {
        panic!("expected the layout");
    };
    assert!(resume.discover.is_zero(), "the resume discovered nothing");

    // Another set of images: the memo's discovery doesn't serve it.
    assert!(matches!(run(job(&style, &images, Resolved::default(), 2)), (JobResult::NeedsDeps(_), _)));
    let other = Arc::new(VecSet::new());
    let (JobResult::Laid(_), times) = run(job(&style, &other, missing(), 2)) else {
        panic!("expected the layout");
    };
    assert!(!times.discover.is_zero());
}
