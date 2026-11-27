//! The tests' own layer types: render halves of the specification's layer
//! types, whose style halves are `mln_style`'s, so that the tests' styles
//! parse as they are. Each draws one color: a geometry layer a quad at each
//! of its features' points, a raster or raster-dem layer and a background
//! each tile's square. They meet terrain, blend, turn with the map and read
//! feature state as the built-in types do as far as the engine's tests need,
//! and no further: what the built-in types draw is their own tests'.

use std::marker::PhantomData;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use geometry::Point;
use mln_gfx::shaders::block::uniform_block;
use mln_gfx::shaders::program::{Program, ProgramBuffer, Projection, Stages};
use mln_gfx::shaders::source::{Sources, WgslSource};
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DepthMaskType, DrawMode};
use mln_style::error::Error;
use mln_style::expression::functions::RegisterError;
use mln_style::layer::{PaintSet, StyleLayerType};
use mln_style::layers::{Background, Circle, ColorRelief, Fill, FillExtrusion, Heatmap, Hillshade, Line, Raster};
use mln_style::range::Range;
use mln_style::types::{HillshadeIlluminationAnchorType, TranslateAnchorType};

use crate::RegistryBuilder;
use crate::draw::draws::{Depth, Draw, Draws, LayerFrame};
use crate::draw::indexes::DrawIndexes;
use crate::draw::mesh::{Indexed, add_point_quads};
use crate::draw::objects::{DrawGeometry, Mesh, Textures, raw_vertices};
use crate::draw::segment::SegmentBase;
use crate::draw::vertex_attribute::{SharedAttribute, Uniforms, VertexAttributeArray};
use crate::image::image_store::ImageType;
use crate::layer::bucket::{Bucket, Upload};
use crate::layer::bucket_geometry::{GeometryVec, bucket_indexes, bucket_vertices};
use crate::layer::layer_type::EvaluatedLayer;
use crate::layer::layout::{BuildCx, Layout, LayoutCx};
use crate::layer::paint_property_binder::{LayerBinders, PaintBinders};
use crate::layer::render_layer::{EvaluateCx, Evaluation, Layer, LayerValues, OnTerrain, PrepareCx, TileCx, UpdateCx};
use crate::layer::tile_input::{Features, NoTiles, TileData};
use crate::render::static_data::RenderStaticData;
use crate::render::terrain::SurfaceLight;
use crate::render::tile::FrameTile;
use crate::render::tile_matrix::{projection_data, to_projection_ubo};
use crate::shaders::programs::POS;
use crate::source::{DemTiles, RasterTiles};
use crate::tile::OverscaledTileID;
use crate::tile_store::tile_cover::{TileCoverParameters, tile_cover};

/// Adds the tests' layer types.
pub(crate) fn register_layer_types(builder: &mut RegistryBuilder) -> Result<(), RegisterError> {
    builder.add_layer_type::<TestFeatures<Fill>>()?;
    builder.add_layer_type::<TestFeatures<Line>>()?;
    builder.add_layer_type::<TestFeatures<Circle>>()?;
    builder.add_layer_type::<TestFeatures<Heatmap>>()?;
    builder.add_layer_type::<TestFeatures<FillExtrusion>>()?;
    builder.add_layer_type::<TestRaster>()?;
    builder.add_layer_type::<TestDem<Hillshade>>()?;
    builder.add_layer_type::<TestDem<ColorRelief>>()?;
    builder.add_layer_type::<TestBackground>()
}

uniform_block! {
    /// The test program's color, premultiplied.
    pub struct TestPropsUBO {
        pub color: [f32; 4],
    }
}

const TEST_WGSL: WgslSource = WgslSource {
    vertex: Some(
        r"
@vertex
fn main(in: VertexInput) -> @builtin(position) vec4<f32> {
    return projectTileFor3D(vec2<f32>(f32(in.pos.x), f32(in.pos.y)), 0.0, projectionVector[globalIndex.value]);
}
",
    ),
    fragment: Some(
        r"
@fragment
fn main() -> @location(0) vec4<f32> {
    return props.color;
}
",
    ),
    prelude: None,
};

/// The tests' layers' program, of WebGPU alone: a mesh of tile positions in
/// one color.
pub(crate) static TEST: Program = Program {
    name: "TestShader",
    buffers: &[ProgramBuffer::uniform::<TestPropsUBO>("props", 0, Stages::Fragment)],
    paint_params: None,
    projection: Projection::PerDraw,
    attributes: &[POS],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
    sources: Sources::NONE.wgsl(&TEST_WGSL),
};

/// Pushes a draw of `mesh` for each of `tiles` whose value is a mesh, and
/// the layer's color.
fn push_draws<L: Layer<Tile = Mesh>>(cx: &mut PrepareCx<'_, '_, L>, ids: &[OverscaledTileID], out: &mut Draws) {
    for &tile_id in ids {
        let Some(entry) = cx.tiles.entry(&tile_id) else { continue };
        let Some(mesh) = entry.value.as_ref() else { continue };
        let projection = projection_data(
            &tile_id.to_unwrapped(),
            cx.frame,
            [0.0, 0.0],
            TranslateAnchorType::Viewport,
            false,
            false,
            None,
            false,
        );
        let draw = Draw {
            order: mesh.seq(),
            depth: Depth::Layer { sub_layer: 0, mask: DepthMaskType::ReadOnly },
            ..Draw::DEFAULT
        };
        out.main().push_at(mesh, tile_id, &to_projection_ubo(&projection), draw, ());
    }
    out.main().block(&TestPropsUBO { color: [0.0, 0.0, 1.0, 1.0] });
}

/// The square of a tile, of the test program.
fn square(cx: &mut TileCx<'_, '_, impl Layer>) -> Mesh {
    let vertices = RenderStaticData::tile_vertices();
    let count = vertices.len();
    let geometry = DrawGeometry {
        vertices: raw_vertices("pos", bytemuck::cast_slice(&vertices).to_vec(), count, AttributeDataType::Short2),
        vertex_count: count,
        mode: DrawMode::Triangles(),
        indexes: DrawIndexes::new(RenderStaticData::quad_triangle_indices()),
        segments: RenderStaticData::tile_triangle_segments().iter().map(SegmentBase::copy).collect(),
        instances: None,
    };
    let shader = cx.res.context.shader_key(&TEST, Uniforms::default(), cx.base.projection_variant);
    cx.res.mesh(shader, ColorMode::alpha_blended(), CullFaceMode::disabled(), geometry, Textures::default())
}

/// A geometry layer type whose render half the tests give: how its layers
/// meet terrain and turn with the map, and the binders of its data-driven
/// paint values, which feature state rewrites.
pub(crate) trait TestGeometry: StyleLayerType + Sized {
    type Binders: PaintBinders<Paint = <Self::PaintProperties as PaintSet>::Evaluated>;
    const TERRAIN: OnTerrain = OnTerrain::Flat;
    const VOLUME: bool = false;

    /// The pattern that `layer` draws with, an image its layout waits for.
    fn pattern(_layer: &EvaluatedLayer<Self>) -> Option<String> {
        None
    }

    /// Whether the layer draws by the map's bearing with `paint`.
    fn drapes_by_bearing(_paint: &<Self::PaintProperties as PaintSet>::Evaluated) -> bool {
        false
    }
}

pub(crate) use binders::{
    TestCircleBinders, TestFillBinders, TestFillExtrusionBinders, TestHeatmapBinders, TestLineBinders,
};

/// The binders of one data-driven paint value of each geometry type, which
/// feature state rewrites. The tests' draws don't read them.
#[expect(dead_code, reason = "the binders' draws, which the tests' layers don't make")]
mod binders {
    use mln_style::color::Color;
    use mln_style::layers::circle_layer_properties::CirclePaintProperties;
    use mln_style::layers::fill_extrusion_layer_properties::FillExtrusionPaintProperties;
    use mln_style::layers::fill_layer_properties::FillPaintProperties;
    use mln_style::layers::heatmap_layer_properties::HeatmapPaintProperties;
    use mln_style::layers::line_layer_properties::LinePaintProperties;

    use crate::layer::paint_property_binder::PaintPropertyBinder;

    crate::paint_property_binders!(
        pub struct TestFillBinders for FillPaintProperties::PossiblyEvaluated {
            fill_color: PaintPropertyBinder<Color> => ["color"],
        }
    );

    crate::paint_property_binders!(
        pub struct TestLineBinders for LinePaintProperties::PossiblyEvaluated {
            line_color: PaintPropertyBinder<Color> => ["color"],
        }
    );

    crate::paint_property_binders!(
        pub struct TestCircleBinders for CirclePaintProperties::PossiblyEvaluated {
            circle_color: PaintPropertyBinder<Color> => ["color"],
        }
    );

    crate::paint_property_binders!(
        pub struct TestHeatmapBinders for HeatmapPaintProperties::PossiblyEvaluated {
            heatmap_weight: PaintPropertyBinder<f32> => ["weight"],
        }
    );

    crate::paint_property_binders!(
        pub struct TestFillExtrusionBinders for FillExtrusionPaintProperties::PossiblyEvaluated {
            fill_extrusion_color: PaintPropertyBinder<Color> => ["color"],
        }
    );
}

impl TestGeometry for Fill {
    type Binders = TestFillBinders;
    const TERRAIN: OnTerrain = OnTerrain::Drapes;

    fn pattern(layer: &EvaluatedLayer<Self>) -> Option<String> {
        let pattern = layer.paint.fill_pattern.faded().to.id();
        (!pattern.is_empty()).then(|| pattern.to_owned())
    }
}

impl TestGeometry for Line {
    type Binders = TestLineBinders;
    const TERRAIN: OnTerrain = OnTerrain::Drapes;

    /// A line translated in the viewport turns with the map.
    fn drapes_by_bearing(paint: &<Self::PaintProperties as PaintSet>::Evaluated) -> bool {
        *paint.line_translate_anchor.uniform() == TranslateAnchorType::Viewport
            && *paint.line_translate.uniform() != [0.0, 0.0]
    }
}

impl TestGeometry for Circle {
    type Binders = TestCircleBinders;
}

impl TestGeometry for Heatmap {
    type Binders = TestHeatmapBinders;
}

impl TestGeometry for FillExtrusion {
    type Binders = TestFillExtrusionBinders;
    const VOLUME: bool = true;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
#[repr(C)]
pub(crate) struct TestVertex {
    pos: [i16; 2],
}

/// A tile's quads, one at each point of its features, and their paint
/// values.
#[derive(Clone, mln_wire::Wire)]
pub(crate) struct TestBucket<B: PaintBinders> {
    pub(crate) vertices: GeometryVec<TestVertex>,
    triangles: GeometryVec<u16>,
    segments: Vec<SegmentBase>,
    binders: LayerBinders<B>,
}

impl<B: PaintBinders> Bucket for TestBucket<B> {
    fn upload(&mut self, up: &mut Upload<'_>) {
        up.vec(&mut self.vertices);
        up.indexes(&mut self.triangles);
        up.binders(&mut self.binders);
    }
}

/// The features of a tile that the group's filter selects.
pub(crate) struct TestLayout<S> {
    features: Vec<usize>,
    style: PhantomData<fn() -> S>,
}

impl<S: TestGeometry> Layout<S> for TestLayout<S> {
    type Bucket = TestBucket<S::Binders>;

    fn needs_resources(group: &[Arc<EvaluatedLayer<S>>]) -> bool {
        group.iter().any(|layer| S::pattern(layer).is_some())
    }

    fn new(cx: &mut LayoutCx<'_, S>) -> Result<Self, Error> {
        for layer in cx.layers() {
            if let Some(pattern) = S::pattern(layer) {
                cx.need_image(&pattern, ImageType::Pattern);
            }
        }
        let tile_layer = cx.source_layer()?;
        let selection = cx.select(&tile_layer, cx.filter_context());
        let mut features = Vec::new();
        for i in 0..tile_layer.len() {
            if selection.may_select(i) && selection.selects(&tile_layer.feature(i)?) {
                features.push(i);
            }
        }
        Ok(TestLayout { features, style: PhantomData })
    }

    fn build(self, cx: &mut BuildCx<'_, S>) -> Result<Option<Self::Bucket>, Error> {
        let source_layer = cx.source_layer()?;
        let mut bucket = TestBucket {
            vertices: GeometryVec::default(),
            triangles: GeometryVec::default(),
            segments: Vec::new(),
            binders: cx.binders(),
        };
        for &i in &self.features {
            let feature = source_layer.feature(i)?;
            let geometry = feature.geometry()?;
            let points = geometry.iter().flatten().copied().collect::<Vec<Point<i16>>>();
            let mut triangles = Indexed { indexes: bucket.triangles.get_mut(), segments: &mut bucket.segments };
            add_point_quads(bucket.vertices.get_mut(), &mut triangles, points, |pos| TestVertex { pos });
            cx.populate(&mut bucket.binders, i, &feature, bucket.vertices.len(), None, None);
            cx.index(i, &geometry);
        }
        Ok((!bucket.segments.is_empty()).then_some(bucket))
    }
}

/// The tests' render half of the geometry layer type `S`.
pub(crate) struct TestFeatures<S>(PhantomData<fn() -> S>);

impl<S: TestGeometry> Layer for TestFeatures<S> {
    type Style = S;
    type Input = Features<TestLayout<S>>;
    type Tile = Mesh;
    const TERRAIN: OnTerrain = S::TERRAIN;
    const VOLUME: bool = S::VOLUME;

    fn new(_values: &LayerValues<S>) -> Self {
        TestFeatures(PhantomData)
    }

    fn evaluate(&mut self, cx: EvaluateCx<'_, Self>) -> Evaluation {
        Evaluation { drapes_by_bearing: S::drapes_by_bearing(&cx.evaluated.paint), ..Evaluation::TRANSLUCENT }
    }

    fn tile(
        &mut self,
        cx: &mut TileCx<'_, '_, Self>,
        _tile: FrameTile<'_, Self>,
        data: TileData<'_, Self>,
        _previous: Option<Mesh>,
    ) -> Option<Mesh> {
        let bucket = data.bucket;
        let geometry = data.geometry();
        let (vertices, indexes) =
            (bucket_vertices(geometry, &bucket.vertices)?, bucket_indexes(geometry, &bucket.triangles)?);
        let mut attrs = VertexAttributeArray::new();
        attrs.set_shared(
            "pos",
            SharedAttribute::new(vertices, AttributeDataType::Short2, 0, size_of::<TestVertex>() as u32),
        );
        let geometry = DrawGeometry {
            vertices: attrs,
            vertex_count: bucket.vertices.len(),
            mode: DrawMode::Triangles(),
            indexes: indexes.into(),
            segments: bucket.segments.iter().map(SegmentBase::copy).collect(),
            instances: None,
        };
        let shader = cx.res.context.shader_key(&TEST, Uniforms::default(), cx.base.projection_variant);
        Some(cx.res.mesh(shader, ColorMode::alpha_blended(), CullFaceMode::disabled(), geometry, Textures::default()))
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_, '_, Self>, tiles: &[FrameTile<'_, Self>], out: &mut Draws) {
        for tile in tiles {
            if let Some(mut entry) = cx.tiles.entry(&tile.id()) {
                entry.take_changes();
            }
        }
        let ids: Vec<_> = tiles.iter().map(|tile| tile.id()).collect();
        push_draws(cx, &ids, out);
    }
}

/// The tests' render half of the raster layer type.
pub(crate) struct TestRaster;

impl Layer for TestRaster {
    type Style = Raster;
    type Input = RasterTiles;
    type Tile = Mesh;
    const TERRAIN: OnTerrain = OnTerrain::Drapes;

    fn new(_values: &LayerValues<Raster>) -> Self {
        TestRaster
    }

    fn tile(
        &mut self,
        cx: &mut TileCx<'_, '_, Self>,
        _tile: FrameTile<'_, Self>,
        _data: TileData<'_, Self>,
        _previous: Option<Mesh>,
    ) -> Option<Mesh> {
        Some(square(cx))
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_, '_, Self>, tiles: &[FrameTile<'_, Self>], out: &mut Draws) {
        let ids: Vec<_> = tiles.iter().map(|tile| tile.id()).collect();
        push_draws(cx, &ids, out);
    }
}

/// A raster-dem layer type whose render half the tests give.
pub(crate) trait TestDemStyle: StyleLayerType + Sized {
    /// Whether the terrain's surface shades the layer's drapes by the map's
    /// bearing with `layer`'s values.
    fn lit_by_bearing(_layer: &EvaluatedLayer<Self>) -> bool {
        false
    }
}

/// A hillshade lit from the viewport.
impl TestDemStyle for Hillshade {
    fn lit_by_bearing(layer: &EvaluatedLayer<Self>) -> bool {
        *layer.paint.hillshade_illumination_anchor.uniform() == HillshadeIlluminationAnchorType::Viewport
    }
}

impl TestDemStyle for ColorRelief {}

/// The tests' render half of the raster-dem layer type `S`: a layer lit
/// by the bearing has the terrain's surface shade its drapes with a light
/// of the frame's bearing.
pub(crate) struct TestDem<S>(PhantomData<fn() -> S>);

impl<S: TestDemStyle> Layer for TestDem<S> {
    type Style = S;
    type Input = DemTiles;
    type Tile = Mesh;
    const TERRAIN: OnTerrain = OnTerrain::Drapes;

    fn new(_values: &LayerValues<S>) -> Self {
        TestDem(PhantomData)
    }

    fn surface_light(&mut self, values: &LayerValues<S>, frame: &LayerFrame<'_>) -> Option<SurfaceLight> {
        let mut light = SurfaceLight::default();
        light.props.azimuths[0] = frame.view().angle() as f32;
        S::lit_by_bearing(&values.evaluated).then_some(light)
    }

    fn evaluate(&mut self, cx: EvaluateCx<'_, Self>) -> Evaluation {
        Evaluation { drapes_by_bearing: S::lit_by_bearing(cx.evaluated), ..Evaluation::TRANSLUCENT }
    }

    fn tile(
        &mut self,
        cx: &mut TileCx<'_, '_, Self>,
        _tile: FrameTile<'_, Self>,
        dem: TileData<'_, Self>,
        _previous: Option<Mesh>,
    ) -> Option<Mesh> {
        dem.has_data().then(|| square(cx))
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_, '_, Self>, tiles: &[FrameTile<'_, Self>], out: &mut Draws) {
        let ids: Vec<_> = tiles.iter().map(|tile| tile.id()).collect();
        push_draws(cx, &ids, out);
    }
}

/// The tests' render half of the background layer type: the tiles that
/// cover the view.
pub(crate) struct TestBackground;

impl Layer for TestBackground {
    type Style = Background;
    type Input = NoTiles;
    type Tile = Mesh;
    const TERRAIN: OnTerrain = OnTerrain::Drapes;
    const PICKS_TILES: bool = true;

    fn new(_values: &LayerValues<Background>) -> Self {
        TestBackground
    }

    fn update(&mut self, mut cx: UpdateCx<'_, '_, Self>, _tiles: &[FrameTile<'_, Self>]) {
        let (view, update) = (cx.parameters.view, cx.parameters.update_parameters);
        let zoom = view.integer_zoom();
        let tiles = tile_cover(
            &TileCoverParameters {
                view: view.clone(),
                tile_lod_min_radius: update.tile_lod_min_radius,
                tile_lod_scale: update.tile_lod_scale,
                tile_lod_pitch_threshold: update.tile_lod_pitch_threshold,
                tile_lod_mode: update.tile_lod_mode,
                requested_zoom: None,
                round_zoom: false,
                max_content_elevation: 0.0,
            },
            zoom,
            Range::new(0u8, zoom),
            None,
        );
        cx.pick_tiles(tiles);
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_, '_, Self>, _tiles: &[FrameTile<'_, Self>], out: &mut Draws) {
        let picked = cx.picked.to_vec();
        for &tile_id in &picked {
            let Some(entry) = cx.tiles.entry(&tile_id) else { continue };
            if entry.value.is_none() {
                let vertices = RenderStaticData::tile_vertices();
                let count = vertices.len();
                let geometry = DrawGeometry {
                    vertices: raw_vertices(
                        "pos",
                        bytemuck::cast_slice(&vertices).to_vec(),
                        count,
                        AttributeDataType::Short2,
                    ),
                    vertex_count: count,
                    mode: DrawMode::Triangles(),
                    indexes: DrawIndexes::new(RenderStaticData::quad_triangle_indices()),
                    segments: RenderStaticData::tile_triangle_segments().iter().map(SegmentBase::copy).collect(),
                    instances: None,
                };
                let context = &mut *cx.res.context;
                let shader = context.shader_key(&TEST, Uniforms::default(), cx.base.projection_variant);
                let mesh = Mesh::new(
                    context,
                    shader,
                    ColorMode::alpha_blended(),
                    CullFaceMode::disabled(),
                    geometry,
                    Textures::default(),
                );
                *entry.value = Some(mesh);
            }
        }
        push_draws(cx, &picked, out);
    }
}
