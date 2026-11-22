//! The `dot` layer type: a layer of a geometry source that draws each point
//! as a disc of a data-driven color and radius, in pixels, which the app
//! scales through the layer's data ([`DotData`]).

use mln_rs::layer::AttributeDataType::{Float2, Float4, Short2};
use mln_rs::layer::intersection_tests::polygon_intersects_buffered_point;
use mln_rs::layer::source::wgsl::Wgsl;
use mln_rs::layer::{
    Attribute, Bucket, BuildCx, Color, Draw, Draws, Error, Features, FrameTile, GpuVec, HitTest, Indexes, Layer,
    LayerBinders, LayerBucket, LayerStyle, LayerTypeInfo, LayerValues, Layout, LayoutCx, Mesh, MeshDesc, NewCx,
    NoProperties, PaintPropertyBinder, PrepareCx, Program, Projection, Property, PropertyTable, PropertyVisitor,
    SourceKind, Sources, Stages, StyleLayerType, TileCx, TileTransform, Upload, View, Wire, program,
};

mln_rs::layer::properties!(
    pub mod paint: paint { dot_color: data_driven Color = Color::black(), dot_radius: data_driven f32 = 5.0, }
    use mln_rs::mln_util::Color;
);

/// The `dot` layer type's style half.
pub struct Dot;

impl StyleLayerType for Dot {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "dot", source: SourceKind::Geometry, from_json: true };
    const PROPERTIES: &'static PropertyTable = &paint::PROPERTIES;
    type LayoutProperties = NoProperties;
    type PaintProperties = paint::Transitionable;
    type Data = DotData;

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        paint::visit(slot, visitor)
    }
}

/// What an app gives a dot layer
/// ([`Map::edit_style_layer_data`](mln_rs::Map::edit_style_layer_data)).
#[derive(Clone, Debug)]
pub struct DotData {
    /// How much larger than its radius each dot draws.
    pub scale: f32,
}

impl Default for DotData {
    fn default() -> Self {
        DotData { scale: 1.0 }
    }
}

mln_rs::layer::paint_property_binders!(
    pub struct DotBinders for paint::PossiblyEvaluated {
        dot_color: PaintPropertyBinder<Color> => ["color"],
        dot_radius: PaintPropertyBinder<f32> => ["radius"],
    }
);

mln_rs::layer::uniform_block! {
    /// Each draw's: a pixel in clip space, and the mix of each property's
    /// zoom stops, which the engine writes.
    pub struct DotDraw { pub extrude: [f32; 2], pub color_t: f32, pub radius_t: f32 }
}

mln_rs::layer::uniform_block! {
    /// The layer's: the properties where they are the same for every point,
    /// which the engine writes, and the scale.
    pub struct DotProps { pub color: [f32; 4], pub radius: f32, pub scale: f32, #[padding] pub pad: [f32; 2] }
}

/// Each corner of a point's quad is the point doubled, with the corner's
/// right and top sides in the low bits.
pub static DOT: Program = Program {
    name: "dot",
    buffers: &[
        program::Buffer::storage::<DotDraw>("draws", Stages::Vertex),
        program::Buffer::uniform::<DotProps>("props", 0),
    ],
    attributes: &[Attribute::new("corner", Short2), Attribute::new("color", Float4), Attribute::new("radius", Float2)],
    instance_attributes: &[],
    textures: &[],
    properties: &[Property::color("color"), Property::float("radius")],
    projection: Projection::PerDraw,
    sources: Sources::NONE.wgsl(&Wgsl {
        prelude: "",
        vertex: "
struct VertexInput {
    @location(0) corner: vec2<i32>,
    #property attribute color
    #property attribute radius
};
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) extrude: vec2<f32>,
    #property varying color 1
};
@vertex fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = draws[drawIndex];
    let corner = vec2<f32>(in.corner);
    #property pass color
    out.extrude = glMod2v(corner, vec2<f32>(2.0)) * 2.0 - 1.0;
    let center = projectTile(floor(corner * 0.5), projectionVector[drawIndex]);
    let radius = $props.radius * props.scale;
    out.position = center + vec4<f32>(out.extrude * drawable.extrude * radius * center.w, 0.0, 0.0);
    return out;
}",
        fragment: "
struct FragmentInput {
    @location(0) extrude: vec2<f32>,
    #property varying color 1
};
@fragment fn main(in: FragmentInput) -> @location(0) vec4<f32> {
    return $props.color * (1.0 - smoothstep(0.9, 1.0, length(in.extrude)));
}",
    }),
};

#[derive(Clone, Wire)]
#[wire(crate = mln_rs::mln_wire)]
pub struct DotBucket {
    corners: GpuVec<[i16; 2]>,
    triangles: Indexes,
    binders: LayerBinders<DotBinders>,
}

impl Bucket for DotBucket {
    fn upload(&mut self, up: &mut Upload<'_>) {
        up.vec(&mut self.corners);
        up.indexes(&mut self.triangles);
        up.binders(&mut self.binders);
    }
}

/// The features the filter selects, by index.
pub struct DotLayout(Vec<usize>);

impl Layout<Dot> for DotLayout {
    type Bucket = DotBucket;

    fn new(cx: &mut LayoutCx<'_, Dot>) -> Result<Self, Error> {
        Ok(DotLayout(cx.selected()?))
    }

    fn build(self, cx: &mut BuildCx<'_, Dot>) -> Result<Option<DotBucket>, Error> {
        let mut b = DotBucket { corners: GpuVec::default(), triangles: Indexes::default(), binders: cx.binders() };
        for i in self.0 {
            let (feature, corners) = (cx.feature(i)?, b.corners.get_mut());
            let geometry = feature.geometry()?;
            for p in geometry.iter().flatten() {
                let (x, y, first) = (p.x * 2, p.y * 2, corners.len());
                corners.extend([[x, y], [x | 1, y], [x | 1, y | 1], [x, y | 1]]);
                b.triangles.add(corners, first, [[0, 1, 2], [0, 3, 2]]);
            }
            cx.populate(&mut b.binders, i, &feature, corners.len());
            cx.index(i, &geometry);
        }
        Ok((!b.triangles.is_empty()).then_some(b))
    }
}

/// A dot layer: a mesh per tile of its bucket's quads.
pub struct DotLayer;

impl Layer for DotLayer {
    type Style = Dot;
    type Input = Features<DotLayout>;
    type Tile = Mesh<DotDraw>;

    fn new(_cx: &mut NewCx<'_, Self>) -> Self {
        DotLayer
    }

    fn tile(
        &mut self,
        cx: &mut TileCx<'_, Self>,
        _: FrameTile,
        laid: LayerBucket<'_, Dot, DotBucket>,
    ) -> Option<Mesh<DotDraw>> {
        let mut mesh = MeshDesc::new(&DOT, laid.indexes(&laid.bucket.triangles));
        mesh.vertices(laid.buffer(&laid.bucket.corners)).paint(laid.binders(&laid.bucket.binders));
        Some(cx.res.mesh(&mesh))
    }

    fn prepare(&self, cx: &PrepareCx<'_, Self>, out: &mut Draws) {
        let main = out.main();
        main.block(&DotProps { scale: cx.layer.style.data.scale, ..DotProps::default() });
        for (tile, _, mesh) in cx.tiles() {
            let at = tile.project(cx.frame, &TileTransform::default());
            main.push_at(mesh, &at, Draw::DEFAULT, DotDraw { extrude: cx.frame.pixels_to_clip, ..DotDraw::default() });
        }
    }

    fn query_radius(&self, layer: &LayerValues<Dot>, laid: LayerBucket<'_, Dot, DotBucket>, _: &View) -> f32 {
        let largest = laid.binders(&laid.bucket.binders).dot_radius.max();
        largest.or(layer.evaluated.paint.dot_radius.constant()).unwrap_or(0.0) * layer.style.data.scale
    }

    fn hit(&self, test: &HitTest<'_, Self>) -> Option<f32> {
        let radius = test.feature_value(&test.layer.evaluated.paint.dot_radius) * test.layer.style.data.scale;
        let reach = radius * test.pixels_to_tile_units;
        let hit =
            test.geometry.iter().flatten().any(|p| polygon_intersects_buffered_point(test.query_geometry, p, reach));
        hit.then_some(0.0)
    }
}
