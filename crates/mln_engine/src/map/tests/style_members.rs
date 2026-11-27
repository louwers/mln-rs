//! Extension members that an app registers: a map's style reads and edits
//! them, a layer's renderer gets a change to its members, and a layer's
//! update reads the root's members of the frame's style.

use std::sync::Arc;

use mln_gfx::frame::FrameTarget;
use mln_gfx::offscreen_target::OffscreenTarget;
use mln_style::conversion::Error;
use mln_style::expression::functions::{RegisterError, register_builtin_functions};
use mln_style::layer::{Layer as StyleLayer, LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use mln_style::member::Member;
use mln_style::property::table::{PropertyTable, PropertyVisitor};
use mln_style::size::Size;
use parking_lot::Mutex;

use crate::layer::{EvaluateCx, Evaluation, FrameTile, Layer, LayerValues, NoTiles, UpdateCx};
use crate::map::{Map, MapMode, MapOptions};
use crate::tests::fake_gpu::FakeGpu;
use crate::{RegistryBuilder, Renderer, UpdateReader};

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
const SIZE: Size = Size::new(16, 16);

/// A continuous map of the style with a `test.seen` layer, the renderer
/// that draws its updates, and what the layer saw.
struct Session {
    map: Map,
    renderer: Renderer,
    gpu: FakeGpu,
    target: OffscreenTarget<FakeGpu>,
    updates: UpdateReader,
    drawn: u64,
    seen: Arc<Seen>,
    _pool: crate::exec::pool::Pool,
}

impl Session {
    fn new() -> Session {
        let mut builder = RegistryBuilder::new();
        register_builtin_functions(builder.functions_mut()).unwrap();
        builder.add_layer_type::<SeeingLayer>().unwrap();
        builder.add_member(&THEME).unwrap();
        builder.add_member(&GROUP).unwrap();
        assert_eq!(builder.add_member(&GROUP), Err(RegisterError::Duplicate("layer member test:group".to_owned())));
        let registry = builder.build();

        let pool = super::pool();
        let options = MapOptions { map_mode: MapMode::Continuous, size: SIZE, ..Default::default() };
        let mut map = Map::new(options, Arc::clone(&registry), &pool).unwrap();
        let gpu = FakeGpu::default();
        let mut renderer = Renderer::new(
            &gpu,
            1.0,
            crate::text::local_fonts::LocalGlyphs::default(),
            registry,
            Arc::new(|| {}),
            &pool,
        );
        renderer.set_event_queue(Some(map.renderer_events()));
        map.load_style_json(STYLE);
        let seen = Arc::new(Seen::default());
        let mut style = LayerStyle::<Seeing>::new("seen".to_owned(), String::new());
        style.extra = Arc::clone(&seen);
        map.edit_style(|style_| {
            let layer = StyleLayer::from_style(Arc::new(style), Arc::clone(style_.registry().functions()));
            style_.add_layer(layer, None)
        })
        .unwrap();
        let updates = map.update_reader();
        Session {
            map,
            renderer,
            gpu,
            target: OffscreenTarget::new(mln_gfx::Size::new(SIZE.width, SIZE.height)),
            updates,
            drawn: 0,
            seen,
            _pool: pool,
        }
    }

    /// Draws the map's latest update, unless it was drawn. Returns whether
    /// there was one to draw.
    fn render_update(&mut self) -> bool {
        self.map.process_events();
        let Some(update) = self.updates.latest().filter(|update| update.generation != self.drawn) else {
            return false;
        };
        self.drawn = update.generation;
        let gpu = &mut self.gpu;
        if let Some(frame) = self.renderer.update(gpu, &update.parameters) {
            let image = self.target.begin_frame(gpu).expect("the frame's image");
            let drawn = self.renderer.render(gpu, &image, frame);
            let handoff = self.target.end_frame(gpu);
            self.renderer.finish_frame(gpu, drawn, handoff);
        }
        true
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.target.tear_down(&mut self.gpu);
    }
}

#[test]
fn a_change_to_a_layers_members_reaches_its_renderer() {
    let mut session = Session::new();
    assert!(session.render_update());
    assert_eq!(None, *session.seen.group.lock());

    session.map.edit_style(|style| style.set_layer_property_json("seen", "test:group", r#""top""#)).unwrap();
    assert!(session.render_update(), "the edit brings a frame");
    assert_eq!(Some("top"), session.seen.group.lock().as_deref());

    session.map.edit_style(|style| style.set_layer_property_json("seen", "test:group", "null")).unwrap();
    assert!(session.render_update());
    assert_eq!(None, *session.seen.group.lock());
}

#[test]
fn a_layers_update_reads_the_root_members_of_the_frames_style() {
    let mut session = Session::new();
    assert!(session.render_update());
    assert_eq!(Some("night"), session.seen.theme.lock().as_deref());

    session.map.edit_style(|style| style.set_root_property_json("test:theme", r#""day""#)).unwrap();
    assert!(session.render_update(), "the edit brings a frame");
    assert_eq!(Some("day"), session.seen.theme.lock().as_deref());

    session.map.edit_style(|style| style.set_root_property_json("test:theme", "null")).unwrap();
    assert!(session.render_update());
    assert_eq!(None, *session.seen.theme.lock());
}
