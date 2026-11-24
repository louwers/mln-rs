//! How the engine draws a layer's draw groups through the facade: where
//! they sit among the layers.

mod support;

use mln_rs::layer::{Draws, Layer, LayerTypeInfo, NewCx, NoProperties, NoTiles, PrepareCx, SourceKind, StyleLayerType};
use mln_rs::mln_render::RegistryBuilder;
use support::{Fixture, SIZE, wait};

/// A layer type without a source that asks for its overlay and draws
/// nothing in it.
struct Overlaid;

impl StyleLayerType for Overlaid {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "overlaid", source: SourceKind::None, from_json: true };
    type LayoutProperties = NoProperties;
    type PaintProperties = NoProperties;
    type Data = ();
}

impl Layer for Overlaid {
    type Style = Overlaid;
    type Input = NoTiles;
    type Tile = ();

    fn new(_cx: &mut NewCx<'_, Self>) -> Self {
        Overlaid
    }

    fn prepare(&self, _cx: &PrepareCx<'_, Self>, out: &mut Draws) {
        out.overlay();
    }
}

fn fixture(layers: &str) -> Fixture {
    let mut builder = RegistryBuilder::new();
    mln_rs::register_builtins(&mut builder).unwrap();
    builder.add_layer_type::<Overlaid>().unwrap();
    let style = format!(
        r##"{{"version":8,"transition":{{"duration":0}},
        "sources":{{"square":{{"type":"geojson","data":{{"type":"Polygon",
            "coordinates":[[[-20,-20],[20,-20],[20,20],[-20,20],[-20,-20]]]}}}}}},
        "layers":[{layers}]}}"##
    );
    Fixture::new(builder.build(), &style)
}

/// The premultiplied RGBA of the last frame's center pixel.
fn center(fixture: &Fixture) -> [u8; 4] {
    let image = wait(fixture.session.read_texture());
    let offset = ((SIZE.height / 2 * SIZE.width + SIZE.width / 2) * 4) as usize;
    image.data[offset..offset + 4].try_into().unwrap()
}

/// A background above 3D geometry covers it, though a layer above both
/// draws a group more than the layers count.
#[test]
fn an_overlay_above_3d_geometry_leaves_the_layers_below_it_above_the_geometry() {
    let fixture = fixture(
        r##"{"id":"extrusion","type":"fill-extrusion","source":"square",
            "paint":{"fill-extrusion-color":"#ff0000","fill-extrusion-height":100000}},
        {"id":"background","type":"background","paint":{"background-color":"#ffffff"}},
        {"id":"overlaid","type":"overlaid"}"##,
    );
    assert_eq!([255, 255, 255, 255], center(&fixture));
}
