//! How the engine draws a layer's draw groups through the facade: where
//! they sit among the layers, and what it makes of a mesh with nothing to
//! draw.

mod support;

use mln_extension_tests::stamp::program::{RECT, RectDraw};
use mln_rs::layer::{
    Draw, Draws, Layer, LayerTypeInfo, Mesh, MeshDesc, NewCx, NoProperties, NoTiles, PrepareCx, SourceKind,
    StyleLayerType,
};
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

/// A layer type without a source that draws a mesh of no indexes.
struct Unindexed;

impl StyleLayerType for Unindexed {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "unindexed", source: SourceKind::None, from_json: true };
    type LayoutProperties = NoProperties;
    type PaintProperties = NoProperties;
    type Data = ();
}

/// An `unindexed` layer.
struct UnindexedLayer(Mesh<RectDraw>);

impl Layer for UnindexedLayer {
    type Style = Unindexed;
    type Input = NoTiles;
    type Tile = ();

    fn new(cx: &mut NewCx<'_, Self>) -> Self {
        let corners = cx.res.buffer(&[[0i16, 0], [1, 0], [1, 1], [0, 1]]);
        let indexes = cx.res.index_buffer(&[]);
        let mut desc = MeshDesc::new(&RECT, &indexes);
        desc.vertices(&corners);
        UnindexedLayer(cx.res.mesh(&desc))
    }

    fn prepare(&self, _cx: &PrepareCx<'_, Self>, out: &mut Draws) {
        out.main().push(&self.0, Draw::DEFAULT, RectDraw::default());
    }
}

fn fixture(layers: &str) -> Fixture {
    let mut builder = RegistryBuilder::new();
    mln_rs::register_builtins(&mut builder).unwrap();
    builder.add_layer_type::<Overlaid>().unwrap();
    builder.add_layer_type::<UnindexedLayer>().unwrap();
    let style = format!(
        r#"{{"version":8,"transition":{{"duration":0}},
        "sources":{{"square":{{"type":"geojson","data":{{"type":"Polygon",
            "coordinates":[[[-20,-20],[20,-20],[20,20],[-20,20],[-20,-20]]]}}}}}},
        "layers":[{layers}]}}"#
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

#[test]
fn a_mesh_of_no_indexes_draws_nothing() {
    let fixture = fixture(
        r##"{"id":"background","type":"background","paint":{"background-color":"#ffffff"}},
        {"id":"unindexed","type":"unindexed"}"##,
    );
    fixture.render();
    assert_eq!([255, 255, 255, 255], center(&fixture));
}
