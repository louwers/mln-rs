//! The `dot` layer type: a layer of a geometry source that draws each point
//! as a disc of a data-driven color and radius, in pixels.

use mln_rs::layer::AttributeDataType::{Float2, Float4, Short2};
use mln_rs::layer::source::wgsl::Wgsl;
use mln_rs::layer::{
    Attribute, Bucket, BuildCx, Color, Draw, Draws, Error, Features, FrameTile, GpuVec, Indexes, Layer, LayerBinders,
    LayerBucket, LayerStyle, LayerTypeInfo, Layout, LayoutCx, Mesh, MeshDesc, NewCx, NoProperties, PaintPropertyBinder,
    PrepareCx, Program, Projection, Property, PropertyInfo, PropertyTable, PropertyVisitor, SourceKind, Sources,
    Stages, StyleLayerType, TileCx, TileTransform, Upload, program,
};

mln_rs::layer::properties!(
    pub mod paint: paint { dot_color: data_driven Color = Color::black(), dot_radius: data_driven f32 = 5.0, }
    use mln_rs::mln_util::Color;
);

/// The `dot` layer type's style half.
pub struct Dot;

impl StyleLayerType for Dot {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "dot", source: SourceKind::Geometry, from_json: true };
    const PROPERTIES: &'static PropertyTable = &PropertyTable::new(
        &[PropertyInfo::paint("dot-color").data_driven(), PropertyInfo::paint("dot-radius").data_driven()],
        &[0, 1],
    );
    type LayoutProperties = NoProperties;
    type PaintProperties = paint::Transitionable;
    type Data = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, v: V) -> Option<V::Output> {
        Some(match slot {
            0 => v.paint(|s| &s.paint.dot_color, |s| &mut s.paint.dot_color),
            1 => v.paint(|s| &s.paint.dot_radius, |s| &mut s.paint.dot_radius),
            _ => return None,
        })
    }
}

mln_rs::layer::paint_property_binders!(
    pub struct DotBinders for paint::PossiblyEvaluated {
        dot_color: PaintPropertyBinder<Color> => ["color"],
        dot_radius: PaintPropertyBinder<f32> => ["radius"],
    }
);

mln_rs::layer::uniform_block! {
    /// Each draw's: a pixel in clip space, and the mix of each property's zoom stops.
    pub struct DotDraw { pub extrude: [f32; 2], pub color_t: f32, pub radius_t: f32 }
}

mln_rs::layer::uniform_block! {
    /// The layer's: the properties where they are the same for every point.
    pub struct DotProps { pub color: [f32; 4], pub radius: f32, #[padding] pub pad: f32, #[padding] pub pad2: [f32; 2] }
}

/// Each corner of a point's quad is the point doubled, with the corner's
/// right and top sides in the low bits.
pub static DOT: Program = Program {
    name: "dot",
    buffers: &[
        program::Buffer::storage::<DotDraw>("draws", Stages::Vertex),
        program::Buffer::uniform::<DotProps>("props", 0),
    ],
    attributes: &[
        Attribute { id: 0, format: Short2 },
        Attribute { id: 1, format: Float4 },
        Attribute { id: 2, format: Float2 },
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[Property::color("color", 1), Property::float("radius", 2)],
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
    out.position = center + vec4<f32>(out.extrude * drawable.extrude * $props.radius * center.w, 0.0, 0.0);
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

#[derive(Clone, mln_rs::mln_wire::Wire)]
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
        let mut mesh = MeshDesc::new(&DOT);
        mesh.vertices(laid.buffer(&laid.bucket.corners))
            .indexes(laid.indexes(&laid.bucket.triangles))
            .paint(laid.binders(&laid.bucket.binders), &cx.layer.evaluated.paint);
        Some(cx.res.mesh(&mesh))
    }

    fn prepare(&self, cx: &PrepareCx<'_, Self>, out: &mut Draws) {
        let (paint, zoom, main) = (&cx.layer.evaluated.paint, cx.frame.camera.zoom() as f32, out.main());
        let color = paint.dot_color.constant_or(&Color::black()).to_float_array();
        main.block(&DotProps { color, radius: paint.dot_radius.constant_or(&0.0), ..DotProps::default() });
        for (tile, laid, mesh) in cx.tiles() {
            let (at, b) = (tile.project(cx.frame, &TileTransform::default()), laid.binders(&laid.bucket.binders));
            let (color_t, radius_t) = (b.dot_color.interpolation_factor(zoom), b.dot_radius.interpolation_factor(zoom));
            main.push_at(mesh, &at, Draw::DEFAULT, DotDraw { extrude: cx.frame.pixels_to_clip, color_t, radius_t });
        }
    }
}
