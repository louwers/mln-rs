//! Registers a layer type of its own, `org.example.dot`, written with
//! `mln_rs::layer` alone, beside the built-in types and `mln_ngon`'s
//! `org.maplibre.ngon`, and parses a style that has a layer of each.

use std::sync::Arc;

use mln_rs::layer::{
    Draws, FrameTile, Layer, LayerStyle, LayerTypeInfo, LayerValues, NoProperties, NoTiles, PrepareCx, PropertyTable,
    PropertyVisitor, RegisterError, Registry, RegistryBuilder, SourceKind, StyleLayerType, layer_properties,
};
use mln_rs::mln_style::Style;

layer_properties!(
    /// The paint properties of an `org.example.dot` layer.
    mod paint: paint {
        dot_radius: data_driven f32 = 4.0,
    }
);

/// The style half of `org.example.dot`: a layer without a source, with one
/// paint property.
struct Dot;

impl StyleLayerType for Dot {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "org.example.dot", source: SourceKind::None, from_json: true };
    const PROPERTIES: &'static PropertyTable = &PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = paint::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        visit(slot, visitor)
    }
}

/// The render half of `org.example.dot`, which draws nothing.
struct DotLayer;

impl Layer for DotLayer {
    type Style = Dot;
    type Input = NoTiles;
    type Tile = ();

    fn new(_values: &LayerValues<Dot>) -> Self {
        DotLayer
    }

    fn prepare(&mut self, _cx: &mut PrepareCx<'_, '_, Self>, _tiles: &[FrameTile<'_, Self>], _out: &mut Draws) {}
}

/// The built-in types, `org.maplibre.ngon` and `org.example.dot`: the
/// registry that a runtime takes (`mln_rs::Runtime::with_registry`).
fn registry() -> Result<Arc<Registry>, RegisterError> {
    let mut builder = RegistryBuilder::new();
    mln_rs::mln_builtins::register_builtins(&mut builder)?;
    mln_ngon::register(&mut builder)?;
    builder.add_layer_type::<DotLayer>()?;
    Ok(builder.build())
}

const STYLE: &str = r#"{
    "version": 8,
    "sources": {"points": {"type": "geojson", "data": {"type": "Point", "coordinates": [0, 0]}}},
    "layers": [
        {"id": "ngons", "type": "org.maplibre.ngon", "source": "points", "paint": {"ngon-corners": 6}},
        {"id": "dots", "type": "org.example.dot", "paint": {"dot-radius": 3}}
    ]
}"#;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let registry = registry()?;
    let style = Style::from_json(registry.style(), STYLE)?;
    for id in ["ngons", "dots"] {
        let layer = style.get_layer(id).ok_or("a layer is missing")?;
        println!("{id}: {}", layer.info().name);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_style_has_a_layer_of_each_type() {
        let registry = registry().unwrap();
        let style = Style::from_json(registry.style(), STYLE).unwrap();
        let dots = LayerStyle::<Dot>::recover(style.get_layer("dots").unwrap().style()).unwrap();
        assert_eq!(Some(&3.0), dots.paint.dot_radius.value.as_constant());
        assert!(LayerStyle::<mln_ngon::Ngon>::recover(style.get_layer("ngons").unwrap().style()).is_some());
    }
}
