//! Extension members that a crate other than the engine registers: a map's
//! style reads and edits them, a layer's renderer gets a change to its
//! members, and a layer's update reads the root's members of the frame's
//! style.

use std::sync::Arc;

use mln_engine::RegistryBuilder;
use mln_engine::exec::pool::Pool;
use mln_engine::exec::queue::Queue;
use mln_engine::layer::{EvaluateCx, Evaluation, FrameTile, Layer, LayerValues, NoTiles, UpdateCx};
use mln_engine::map::{Map, MapMode, MapOptions};
use mln_headless::HeadlessFrontend;
use mln_style::conversion::Error;
use mln_style::expression::functions::RegisterError;
use mln_style::layer::{Layer as StyleLayer, LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use mln_style::member::Member;
use mln_style::property::table::{PropertyTable, PropertyVisitor};
use mln_style::size::Size;
use parking_lot::Mutex;

struct Theme(String);

struct Group(String);

static THEME: Member<Theme> = Member::root("test:theme", |value, _| {
    value.to_string().map(Theme).ok_or_else(|| Error::new("expected a theme's name"))
});
static GROUP: Member<Group> = Member::layer("test:group", |value, _| {
    value.to_string().map(Group).ok_or_else(|| Error::new("expected a group's name"))
});

/// What a `test.seen` layer's hooks last read: its group in its
/// evaluation, and the root's theme in its update.
#[derive(Default)]
struct Seen {
    group: Mutex<Option<String>>,
    theme: Mutex<Option<String>>,
}

struct Seeing;

static PROPERTIES: PropertyTable = PropertyTable::new(&[], &[]);

impl StyleLayerType for Seeing {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "test.seen", source: SourceKind::None, from_json: false };
    const PROPERTIES: &'static PropertyTable = &PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = NoProperties;
    type Extra = Arc<Seen>;

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(_slot: usize, _visitor: V) -> Option<V::Output> {
        None
    }
}

struct SeeingLayer(Arc<Seen>);

impl Layer for SeeingLayer {
    type Style = Seeing;
    type Input = NoTiles;
    type Tile = ();

    fn new(values: &LayerValues<Seeing>) -> Self {
        SeeingLayer(Arc::clone(&values.style.extra))
    }

    fn evaluate(&mut self, cx: EvaluateCx<'_, Self>) -> Evaluation {
        *self.0.group.lock() = cx.values.style.members().get(&GROUP).map(|group| group.0.clone());
        Evaluation::TRANSLUCENT
    }

    fn update(&mut self, cx: UpdateCx<'_, '_, Self>, _tiles: &[FrameTile<'_, Self>]) {
        let theme = cx.parameters.update_parameters.style.members().get(&THEME);
        *self.0.theme.lock() = theme.map(|theme| theme.0.clone());
    }
}

const STYLE: &str = r#"{"version":8,"test:theme":"night","sources":{},"layers":[]}"#;

/// A map of the style with a `test.seen` layer in a continuous mode, which
/// `frontend` draws, and what the layer saw.
fn session(queue: &Queue<()>, pool: &Pool) -> (HeadlessFrontend, Map, Arc<Seen>) {
    let mut builder = RegistryBuilder::new();
    mln_builtins::register_builtins(&mut builder).unwrap();
    builder.add_layer_type::<SeeingLayer>().unwrap();
    builder.add_member(&THEME).unwrap();
    builder.add_member(&GROUP).unwrap();
    assert_eq!(builder.add_member(&GROUP), Err(RegisterError::Duplicate("layer member test:group".to_owned())));
    let registry = builder.build();

    let size = Size::new(16, 16);
    let mut frontend = HeadlessFrontend::new(size, 1.0, queue, pool).expect("a headless frontend");
    let options = MapOptions { map_mode: MapMode::Continuous, size, ..Default::default() };
    let mut map = Map::new(options, registry, pool).unwrap();
    frontend.attach(&mut map);
    map.load_style_json(STYLE);
    let seen = Arc::new(Seen::default());
    let mut style = LayerStyle::<Seeing>::new("seen".to_owned(), String::new());
    style.extra = Arc::clone(&seen);
    map.edit_style(|style_| {
        let layer = StyleLayer::from_style(Arc::new(style), Arc::clone(style_.registry().functions()));
        style_.add_layer(layer, None)
    })
    .unwrap();
    (frontend, map, seen)
}

#[test]
fn a_change_to_a_layers_members_reaches_its_renderer() {
    let (queue, pool) = (Queue::new(), Pool::new(1));
    let (mut frontend, mut map, seen) = session(&queue, &pool);
    assert!(frontend.render_update());
    assert_eq!(None, *seen.group.lock());

    map.edit_style(|style| style.set_layer_property_json("seen", "test:group", r#""top""#)).unwrap();
    assert!(frontend.render_update(), "the edit brings a frame");
    assert_eq!(Some("top"), seen.group.lock().as_deref());

    map.edit_style(|style| style.set_layer_property_json("seen", "test:group", "null")).unwrap();
    assert!(frontend.render_update());
    assert_eq!(None, *seen.group.lock());
}

#[test]
fn a_layers_update_reads_the_root_members_of_the_frames_style() {
    let (queue, pool) = (Queue::new(), Pool::new(1));
    let (mut frontend, mut map, seen) = session(&queue, &pool);
    assert!(frontend.render_update());
    assert_eq!(Some("night"), seen.theme.lock().as_deref());

    map.edit_style(|style| style.set_root_property_json("test:theme", r#""day""#)).unwrap();
    assert!(frontend.render_update(), "the edit brings a frame");
    assert_eq!(Some("day"), seen.theme.lock().as_deref());

    map.edit_style(|style| style.set_root_property_json("test:theme", "null")).unwrap();
    assert!(frontend.render_update());
    assert_eq!(None, *seen.theme.lock());
}
