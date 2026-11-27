//! The tests' `lends` layer type, whose layers lend the frame's commands to
//! a host before the frame's passes and in its translucent pass, and tell a
//! host that drew how the renderer stopped, once the GPU is done with what
//! it drew.

use std::any::Any;
use std::sync::{Arc, LazyLock};

use mln_engine::draw::{AfterGpu, Draws, GpuEnd, LendAt, Lent};
use mln_engine::layer::{FrameTile, Layer, LayerValues, NoTiles, PrepareCx};
use mln_engine::{Registry, RegistryBuilder};
use mln_runtime::{Applied, Completion, Error, Map, RuntimeOptions};
use mln_style::layer::{Layer as StyleLayer, LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use mln_style::property::table::{PropertyTable, PropertyVisitor};

use super::TestRuntime;

/// What draws a `lends` layer, on the thread that renders the map.
pub(crate) trait LendingHost: Send + Sync + 'static {
    /// Draws with `raw`, the backend's commands that the frame lends `at`
    /// its time.
    fn lend(&self, _at: LendAt, _raw: &mut dyn Any) {}

    /// The renderer stopped after the host drew, as `end` says.
    fn end(&self, _end: GpuEnd) {}
}

/// The built-ins and the `lends` layer type.
fn registry() -> Arc<Registry> {
    static REGISTRY: LazyLock<Arc<Registry>> = LazyLock::new(|| {
        let mut builder = RegistryBuilder::new();
        mln_builtins::register_builtins(&mut builder).unwrap();
        builder.add_layer_type::<Lending>().unwrap();
        builder.build()
    });
    Arc::clone(&REGISTRY)
}

/// A runtime whose maps can have `lends` layers.
pub(crate) fn runtime() -> TestRuntime {
    TestRuntime(mln_runtime::Runtime::with_registry(&RuntimeOptions::default(), registry()).unwrap())
}

/// Adds a `lends` layer named `id` that `host` draws above `map`'s layers.
pub(crate) fn add_layer(map: &Map, id: &str, host: impl LendingHost) -> Result<Completion<Applied>, Error> {
    let mut style = LayerStyle::<Lends>::new(id.to_owned(), String::new());
    style.extra = Some(Arc::new(host));
    map.add_style_layer(StyleLayer::from_style(Arc::new(style), Arc::clone(registry().functions())), None)
}

struct Lends;

static PROPERTIES: PropertyTable = PropertyTable::new(&[], &[]);

impl StyleLayerType for Lends {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "lends", source: SourceKind::None, from_json: false };
    const PROPERTIES: &'static PropertyTable = &PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = NoProperties;
    type Extra = Option<Arc<dyn LendingHost>>;

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(_slot: usize, _visitor: V) -> Option<V::Output> {
        None
    }
}

struct Lending {
    host: Option<Arc<dyn LendingHost>>,
    /// Where the host's end waits for the GPU, once it drew.
    drew: Option<AfterGpu>,
}

impl Drop for Lending {
    fn drop(&mut self) {
        if let (Some(host), Some(after_gpu)) = (self.host.take(), self.drew.take()) {
            after_gpu.push(move |end| host.end(end));
        }
    }
}

impl Layer for Lending {
    type Style = Lends;
    type Input = NoTiles;
    type Tile = ();

    fn new(values: &LayerValues<Lends>) -> Self {
        Lending { host: values.style.extra.clone(), drew: None }
    }

    fn prepare(&mut self, _cx: &mut PrepareCx<'_, '_, Self>, _tiles: &[FrameTile<'_, Self>], out: &mut Draws) {
        out.lend(LendAt::BeforePasses).lend(LendAt::Pass);
    }

    fn lend(&mut self, at: LendAt, lent: Lent<'_>) {
        let Some(host) = &self.host else { return };
        self.drew.get_or_insert_with(|| lent.after_gpu.clone());
        host.lend(at, lent.raw);
    }
}
