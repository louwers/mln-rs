//! A layer type that a crate other than the engine declares and registers
//! on the public layer contract: a layer without a source that draws with
//! the backend's commands that the frame lends it.

use mln_engine::RegistryBuilder;
use mln_engine::draw::{Draws, LendAt, Lent};
use mln_engine::layer::{FrameTile, Layer, LayerValues, NoTiles, PrepareCx};
use mln_style::Style;
use mln_style::expression::functions::{RegisterError, register_builtin_functions};
use mln_style::layer::{LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use mln_style::property::table::{PropertyTable, PropertyVisitor};

/// The style half: a layer type without properties, which style JSON can
/// name.
struct Dot;

static PROPERTIES: PropertyTable = PropertyTable::new(&[], &[]);

impl StyleLayerType for Dot {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "org.example.dot", source: SourceKind::None, from_json: true };
    const PROPERTIES: &'static PropertyTable = &PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = NoProperties;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(_slot: usize, _visitor: V) -> Option<V::Output> {
        None
    }
}

/// The render half, which asks for the frame's commands in its pass.
struct DotLayer {
    lent: usize,
}

impl Layer for DotLayer {
    type Style = Dot;
    type Input = NoTiles;
    type Tile = ();

    fn new(_values: &LayerValues<Dot>) -> Self {
        DotLayer { lent: 0 }
    }

    fn prepare(&mut self, _cx: &mut PrepareCx<'_, '_, Self>, _tiles: &[FrameTile<'_, Self>], out: &mut Draws) {
        out.lend(LendAt::Pass);
    }

    fn lend(&mut self, _at: LendAt, lent: Lent<'_>) {
        assert!(lent.frame.size().width > 0);
        self.lent += 1;
    }
}

#[test]
fn a_layer_type_of_another_crate_registers_and_parses() {
    let mut builder = RegistryBuilder::new();
    register_builtin_functions(builder.functions_mut()).unwrap();
    builder.add_layer_type::<DotLayer>().unwrap();
    assert_eq!(builder.add_layer_type::<DotLayer>(), Err(RegisterError::Duplicate("org.example.dot".to_owned())));
    let registry = builder.build();
    assert!(registry.style().layer_type("org.example.dot").is_some());

    let json = r#"{"version": 8, "sources": {}, "layers": [{"id": "dots", "type": "org.example.dot"}]}"#;
    let style = Style::from_json(registry.style(), json).unwrap();
    let layer = style.get_layer("dots").expect("the layer");
    assert!(LayerStyle::<Dot>::recover(layer.style()).is_some());
}
