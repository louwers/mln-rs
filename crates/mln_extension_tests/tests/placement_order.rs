//! The order in which the engine calls the participants in placement, from
//! two placing layer types on the facade that record their own calls: every
//! participant prepares, the layers of both types place topmost first, each
//! type that placed commits, and every participant updates. A continuous
//! map's type with nothing to place forgets instead of placing.

mod support;

use std::marker::PhantomData;
use std::sync::Arc;

use mln_rs::layer::placement::{PlaceCx, Placement, PlacementCx, PlacingLayer, Prepared};
use mln_rs::layer::{
    Bucket, BuildCx, Draws, Error, EvaluateCx, Evaluation, Features, Layer, LayerTypeInfo, Layout, LayoutCx, MapMode,
    NewCx, NoProperties, PrepareCx, SourceKind, StyleLayerType, Upload, Wire,
};
use mln_rs::mln_render::{Registry, RegistryBuilder};
use parking_lot::Mutex;
use support::{Fixture, wait};

/// The calls of the participants so far, each `<type> <hook>`, with the
/// layer for a hook of one.
static CALLS: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// Held by each test, which reads `CALLS` alone.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn call(tag: &str, hook: &str) {
    CALLS.lock().push(format!("{tag} {hook}"));
}

/// Which of the two layer types.
trait Tag: Send + Sync + 'static {
    const NAME: &'static str;
}

struct A;
struct B;

impl Tag for A {
    const NAME: &'static str = "A";
}

impl Tag for B {
    const NAME: &'static str = "B";
}

/// The style half: a layer of a geometry source, without properties.
struct Mark<T>(PhantomData<fn() -> T>);

impl<T: Tag> StyleLayerType for Mark<T> {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: T::NAME, source: SourceKind::Geometry, from_json: true };
    type LayoutProperties = NoProperties;
    type PaintProperties = NoProperties;
    type Data = ();
}

/// A bucket of every feature of the tile, which draws nothing.
#[derive(Clone, Wire)]
#[wire(crate = mln_rs::mln_wire)]
struct MarkBucket {
    features: u32,
}

impl Bucket for MarkBucket {
    fn upload(&mut self, _up: &mut Upload<'_>) {}
}

struct MarkLayout(usize);

impl<T: Tag> Layout<Mark<T>> for MarkLayout {
    type Bucket = MarkBucket;

    fn new(cx: &mut LayoutCx<'_, Mark<T>>) -> Result<Self, Error> {
        Ok(MarkLayout(cx.selected()?.len()))
    }

    fn build(self, _cx: &mut BuildCx<'_, Mark<T>>) -> Result<Option<MarkBucket>, Error> {
        Ok((self.0 > 0).then_some(MarkBucket { features: self.0 as u32 }))
    }
}

struct MarkLayer<T>(PhantomData<fn() -> T>);

impl<T: Tag> Layer for MarkLayer<T> {
    type Style = Mark<T>;
    type Input = Features<MarkLayout>;
    type Tile = ();

    fn new(_cx: &mut NewCx<'_, Self>) -> Self {
        MarkLayer(PhantomData)
    }

    fn evaluate(&mut self, _cx: EvaluateCx<'_, Self>) -> Evaluation {
        Evaluation::TRANSLUCENT
    }

    fn prepare(&self, _cx: &PrepareCx<'_, Self>, _out: &mut Draws) {}
}

/// A participant that places nothing and records each call.
struct MarkPlacement<T>(PhantomData<fn() -> T>);

impl<T> Default for MarkPlacement<T> {
    fn default() -> Self {
        MarkPlacement(PhantomData)
    }
}

impl<T: Tag> Placement for MarkPlacement<T> {
    type Layer = MarkLayer<T>;

    fn prepare(&mut self, _cx: &mut PlacementCx<'_, MarkLayer<T>>) -> Prepared {
        call(T::NAME, "prepare");
        Prepared::default()
    }

    fn place_crossing(&mut self, layer: &PlacingLayer<'_, MarkLayer<T>>, _cx: &mut PlaceCx<'_>) {
        call(T::NAME, &format!("place_crossing {}", layer.layer.id()));
    }

    fn place(&mut self, layer: &PlacingLayer<'_, MarkLayer<T>>, _cx: &mut PlaceCx<'_>) {
        call(T::NAME, &format!("place {}", layer.layer.id()));
    }

    fn commit(&mut self, _cx: &mut PlaceCx<'_>) {
        call(T::NAME, "commit");
    }

    fn update(&mut self, _cx: &mut PlacementCx<'_, MarkLayer<T>>) {
        call(T::NAME, "update");
    }

    fn forget_buckets(&mut self) {
        call(T::NAME, "forget_buckets");
    }

    fn forget_placement(&mut self) {
        call(T::NAME, "forget_placement");
    }
}

fn registry() -> Arc<Registry> {
    let mut builder = RegistryBuilder::new();
    mln_rs::register_builtins(&mut builder).unwrap();
    builder.add_placing_layer_type::<MarkPlacement<A>>().unwrap();
    builder.add_placing_layer_type::<MarkPlacement<B>>().unwrap();
    builder.build()
}

/// A style of `layers` over the sources `one` and `two`, each a point at
/// the center, and `empty`. Layers of a type share a layout, and place as
/// one, only on one source.
fn style(layers: &str) -> String {
    let point = r#"{"type":"geojson","data":{"type":"Feature","properties":{},
        "geometry":{"type":"Point","coordinates":[0,0]}}}"#;
    format!(
        r#"{{"version":8,"sources":{{"one":{point},"two":{point},
            "empty":{{"type":"geojson","data":{{"type":"FeatureCollection","features":[]}}}}}},
        "layers":[{layers}]}}"#
    )
}

/// The calls of each frame that placed, each frame's from its first
/// participant's `prepare` on.
fn placing_frames() -> Vec<Vec<String>> {
    let mut frames: Vec<Vec<String>> = Vec::new();
    for call in CALLS.lock().drain(..) {
        match frames.last_mut() {
            Some(frame) if call != "A prepare" => frame.push(call),
            _ => frames.push(vec![call]),
        }
    }
    frames.retain(|frame| frame.iter().any(|call| call.contains(" place")));
    frames
}

#[test]
fn a_tile_places_every_layer_topmost_first_in_both_rounds_across_types() {
    let _one = ONE_AT_A_TIME.lock();
    CALLS.lock().clear();
    let layers = r#"{"id":"a1","type":"A","source":"one"},{"id":"b1","type":"B","source":"one"},
        {"id":"a2","type":"A","source":"two"},{"id":"b2","type":"B","source":"two"}"#;
    let fixture = Fixture::with_mode(registry(), &style(layers), MapMode::Tile);
    wait(fixture.map.request_still_image());

    let frames = placing_frames();
    let expected = [
        "A prepare",
        "B prepare",
        "B place_crossing b2",
        "A place_crossing a2",
        "B place_crossing b1",
        "A place_crossing a1",
        "B place b2",
        "A place a2",
        "B place b1",
        "A place a1",
        "A commit",
        "B commit",
        "A forget_buckets",
        "B forget_buckets",
        "A update",
        "B update",
    ];
    assert_eq!(Some(&expected.map(str::to_owned).to_vec()), frames.last(), "{frames:#?}");
}

#[test]
fn a_continuous_maps_type_with_nothing_to_place_forgets_its_placement() {
    let _one = ONE_AT_A_TIME.lock();
    CALLS.lock().clear();
    let layers = r#"{"id":"a1","type":"A","source":"one"},{"id":"b1","type":"B","source":"empty"},
        {"id":"a2","type":"A","source":"two"},{"id":"b2","type":"B","source":"empty"}"#;
    // The fixture renders until the map is loaded and placed.
    drop(Fixture::new(registry(), &style(layers)));

    // A frame may place before both of A's sources load, and then places
    // the layer of the one that did.
    let frames = placing_frames();
    let (before, after) =
        (["A prepare", "B prepare", "B forget_buckets", "B forget_placement"], ["A commit", "A update", "B update"]);
    for frame in &frames {
        let places = &frame[before.len()..frame.len() - after.len()];
        assert!(
            frame.starts_with(&before.map(str::to_owned)) && frame.ends_with(&after.map(str::to_owned)),
            "{frame:#?}"
        );
        assert!(
            ["A place a2", "A place a1"].iter().filter(|call| places.contains(&(**call).to_owned())).eq(places),
            "{frame:#?}"
        );
    }
    let last: Vec<&str> = frames.last().expect("a frame that placed").iter().map(String::as_str).collect();
    assert_eq!(last, [&before[..], &["A place a2", "A place a1"], &after[..]].concat());
}
