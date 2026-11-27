use std::mem::offset_of;
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::gpu::{Gpu, TextureFormat};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode};
use mln_style::StyleValue as _;
use mln_style::bitmap::PremultipliedImage;
use mln_style::color::Color;
use mln_style::expression::{Dependency, EvaluationContext, GlobalStateMap, global_state_refs_intersect};
use mln_style::layer::{ErasedLayerStyle as _, LayerStyle};
use mln_style::layers::Line;
use mln_style::layers::line_layer_properties::LinePaintProperties;
use mln_style::size::Size;
use mln_style::types::{AlignmentType, LineCapType, TranslateAnchorType};

use super::{cast_matrix, refresh_paint_attributes};
use crate::buckets::line_bucket::{LineBinders, LineBucket};
use crate::draw::atlas_binding::AtlasBinding;
use crate::draw::context::Context;
use crate::draw::draws::{Draw, Draws, Group, LayerFrame};
use crate::draw::objects::{DrawGeometry, Mesh, Textures};
use crate::draw::segment::SegmentBase;
use crate::draw::store::SharedTexture;
use crate::draw::texture_size;
use crate::draw::texture2d::Texture2D;
use crate::draw::vertex_attribute::{SharedAttribute, Uniforms, VertexAttributeArray};
use crate::gfx::line_mesh::LineLayoutVertex;
use crate::layer::bucket_geometry::{bucket_indexes, bucket_vertices, paint_vertices};
use crate::layer::layer_type::{EvaluatedLayer, LayerType};
use crate::layer::paint_property_binder::PaintBinders as _;
use crate::layer::render_layer::{
    EvaluateCx, Evaluation, Hosted, LayerValues, OnTerrain, PrepareCx, RenderLayer, TileCx, TypedLayer, UpdateCx,
};
use crate::layer::tile_input::{Features, TileData};
use crate::layers::line::dashes::{LineAtlas, LinePatternCap};
use crate::layers::line::programs;
use crate::layers::line::ubo::{
    LineDrawableUBO, LineEvaluatedPropsUBO, LineGradientDrawableUBO, LinePatternDrawableUBO, LinePatternTilePropsUBO,
    LineSDFDrawableUBO, LineSDFTilePropsUBO,
};
use crate::layout::pattern_layout::{LineLayoutSpec, PatternLayout};
use crate::query::feature_index::QueryRegion;
use crate::render::layer_composite::Composite;
use crate::render::pass::RenderPass;
use crate::render::tile::FrameTile;
use crate::render::tile_matrix::{projection_data, to_projection_ubo};
use crate::tile::{OverscaledTileID, UnwrappedTileID};

/// The slots of the programs' textures that the draws bind.
const DASH_SLOT: usize = programs::LINE_GRADIENT_SDF.texture_slot("image_dash");
const IMAGE_SLOT: usize = programs::LINE_PATTERN.texture_slot("image");

/// What a line draws: its shader and the uniforms it reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineType {
    Simple,
    Gradient,
    Pattern,
    /// Dashes.
    Sdf,
    /// Dashes over a gradient.
    GradientSdf,
}

/// What the layer keeps of a tile: the draw of its bucket's lines, as it was
/// built for the bucket.
pub(crate) struct LineTile {
    /// The lines' mesh, which binds the texture of the tile's icon atlas
    /// before each draw of a pattern.
    mesh: Mesh,
    line_type: LineType,
    /// The passes the draw draws in.
    pass: RenderPass,
    /// The ends of the dashes of the bucket, which pick its pattern in the
    /// layer's dash textures.
    dash_cap: LinePatternCap,
    /// The texture of the layer's color ramp that the mesh of a gradient
    /// binds.
    color_ramp: Option<SharedTexture>,
}

/// The blocks of a draw, of the program of its line type.
enum LineBlocks {
    Simple(LineDrawableUBO),
    Gradient(LineGradientDrawableUBO),
    Pattern(LinePatternDrawableUBO, LinePatternTilePropsUBO),
    Sdf(LineSDFDrawableUBO, LineSDFTilePropsUBO),
}

impl LineBlocks {
    /// Pushes `draw` of `mesh` of the tile `tile`, with the blocks and
    /// `projection`, into `group`.
    fn push(self, group: &mut Group, mesh: &Mesh, tile: OverscaledTileID, projection: &ProjectionUBO, draw: Draw) {
        match self {
            LineBlocks::Simple(drawable) => group.push_at(mesh, tile, projection, draw, drawable),
            LineBlocks::Gradient(drawable) => group.push_at(mesh, tile, projection, draw, drawable),
            LineBlocks::Pattern(drawable, props) => group.push_at(mesh, tile, projection, draw, (drawable, props)),
            LineBlocks::Sdf(drawable, props) => group.push_at(mesh, tile, projection, draw, (drawable, props)),
        };
    }
}

/// Where a dash draw samples its patterns. Where the dash array differs by
/// feature, the shaders divide the scales' x and the gamma by each pattern's
/// length.
struct SdfPatterns {
    patternscale_a: [f32; 2],
    patternscale_b: [f32; 2],
    tex_y_a: f32,
    tex_y_b: f32,
    atlas_height: f32,
    sdfgamma: f32,
}

fn cast_tlbr(a: [u16; 4]) -> [f32; 4] {
    a.map(f32::from)
}

/// The ends of a bucket's dashes. A dash pattern is shared by the whole
/// bucket, so a `line-cap` that varies by feature gets square dashes; each
/// line still gets its own cap at its ends.
fn dash_pattern_cap(bucket: &LineBucket) -> LinePatternCap {
    if bucket.layout.line_cap.constant_or(&LineCapType::Butt) == LineCapType::Round {
        LinePatternCap::Round
    } else {
        LinePatternCap::Square
    }
}

/// The passes the layer of `values` draws its lines in: those of its values
/// of the frame but the opaque one.
fn render_pass(values: &LayerValues<Line>) -> RenderPass {
    RenderPass(values.evaluated.render_passes & !RenderPass::Opaque.underlying_type())
}

pub(crate) fn apply_color_ramp(
    color_value: &mln_style::property::color_ramp::ColorRampPropertyValue,
    image: &mut PremultipliedImage,
    global_state: Option<&GlobalStateMap>,
) -> bool {
    if color_value.is_undefined() {
        return false;
    }

    let length = image.bytes();

    let mut i: u32 = 0;
    while (i as usize) < length {
        let color = color_value.evaluate_ramp(f64::from(i) / length as f64, global_state);
        let i_ = i as usize;
        image.data[i_] = (color.r * 255.0).floor() as u8;
        image.data[i_ + 1] = (color.g * 255.0).floor() as u8;
        image.data[i_ + 2] = (color.b * 255.0).floor() as u8;
        image.data[i_ + 3] = (color.a * 255.0).floor() as u8;
        i += 4;
    }
    true
}

/// A texture of the bucket's dash patterns, for a `line-dasharray` that
/// differs by feature.
fn dash_atlas_texture(gpu: &mut dyn Gpu, context: &mut Context, binders: &LineBinders) -> Option<SharedTexture> {
    let image = binders.line_dasharray.atlas()?.image();
    if !image.valid() {
        return None;
    }
    let texture = context.add_texture_with(|texture| {
        texture.set_format(TextureFormat::A8Unorm);
        texture.set_sampler(SamplerState {
            filter: TextureFilterType::Linear,
            wrap_u: TextureWrapType::Repeat,
            wrap_v: TextureWrapType::Clamp,
            ..Default::default()
        });
    });
    if let Err(error) = context.upload_texture_data(gpu, &texture, &image.data, texture_size(image.size)) {
        context.fail(format!("Uploading the dash patterns failed: {error}"));
        return None;
    }
    Some(texture)
}

impl LayerType for Line {
    type Input = Features<PatternLayout<LineLayoutSpec>>;
    const CROSSFADES: bool = true;

    fn query_radius(data: TileData<'_, Line>, layer: &EvaluatedLayer<Line>) -> f32 {
        data.bucket.query_radius(layer)
    }

    fn render_layer(style: Arc<LayerStyle<Line>>) -> Box<dyn RenderLayer> {
        Hosted::<RenderLineLayer>::boxed(style)
    }
}

pub(crate) struct RenderLineLayer {
    color_ramp: Rc<PremultipliedImage>,
    /// The global-state snapshot used to generate the cached color ramp and
    /// detect when it needs to be rebuilt.
    color_ramp_global_state: Option<Arc<GlobalStateMap>>,
    /// The color ramp's texture, from the first draw that reads it until
    /// the ramp changes. A draw that reads another is built again.
    color_ramp_texture: Option<SharedTexture>,
    /// The textures of the dash patterns of the layer's tiles' dash arrays
    /// that don't differ by feature, and of its own.
    dashes: LineAtlas,
}

impl RenderLineLayer {
    fn update_color_ramp(&mut self, values: &LayerValues<Line>) {
        let color_value = values.unevaluated.line_gradient.get_value().clone();
        let mut image = (*self.color_ramp).clone();
        if !apply_color_ramp(&color_value, &mut image, self.color_ramp_global_state.as_deref()) {
            return;
        }
        // The texture shares the ramp image, so a new one replaces it.
        self.color_ramp = Rc::new(image);

        self.color_ramp_texture = None;
    }

    // query
    fn line_width(
        evaluated: &LinePaintProperties::PossiblyEvaluated,
        feature: &dyn mln_style::feature::GeometryTileFeature,
        zoom: f32,
        feature_state: &mln_style::feature::FeatureState,
    ) -> f32 {
        let line_width = evaluated.line_width.evaluate(&EvaluationContext::with_zoom_feature_state(
            zoom,
            Some(feature),
            Some(feature_state),
        ));
        let gap_width = evaluated.line_gap_width.evaluate(&EvaluationContext::with_zoom_feature_state(
            zoom,
            Some(feature),
            Some(feature_state),
        ));
        if gap_width == 0.0 { line_width } else { gap_width + 2.0 * line_width }
    }
}

// query
fn offset_line(
    rings: &mln_style::feature::GeometryCollection,
    offset: f64,
) -> Vec<mln_style::feature::GeometryCoordinates> {
    use geometry::Point;

    use crate::geo::math::{perp, unit};

    debug_assert!(offset != 0.0);
    debug_assert!(!rings.is_empty());

    let to_f64 = |p: Point<i16>| Point::new(f64::from(p.x), f64::from(p.y));
    let sub = |a: Point<i16>, b: Point<i16>| Point::new(a.x.wrapping_sub(b.x), a.y.wrapping_sub(b.y));

    let zero = Point::new(0.0, 0.0);
    let mut new_rings = Vec::with_capacity(rings.len());
    for ring in rings.iter() {
        let mut new_ring = Vec::with_capacity(ring.len());

        for i in 0..ring.len() {
            let p = ring[i];

            let a_to_b = if i == 0 { zero } else { perp(unit(to_f64(sub(p, ring[i - 1])))) };
            // At the last vertex, use the incoming normal so the endpoint receives the full offset.
            let b_to_c = if i + 1 == ring.len() { a_to_b } else { perp(unit(to_f64(sub(ring[i + 1], p)))) };
            let mut extrude = unit(Point::new(a_to_b.x + b_to_c.x, a_to_b.y + b_to_c.y));

            let cos_half_angle = extrude.x * b_to_c.x + extrude.y * b_to_c.y;
            let scale = if cos_half_angle == 0.0 { 0.0 } else { 1.0 / cos_half_angle };
            extrude = Point::new(extrude.x * scale, extrude.y * scale);

            // The offset rounds toward zero to tile units, which wrap.
            let shifted = Point::new((extrude.x * offset) as i16, (extrude.y * offset) as i16);
            new_ring.push(Point::new(shifted.x.wrapping_add(p.x), shifted.y.wrapping_add(p.y)));
        }
        new_rings.push(new_ring);
    }

    new_rings
}

impl TypedLayer for RenderLineLayer {
    type Type = Line;
    type Tile = LineTile;
    const TERRAIN: OnTerrain = OnTerrain::Drapes;

    fn new(_values: &LayerValues<Line>) -> Self {
        RenderLineLayer {
            color_ramp: Rc::new(PremultipliedImage::new(Size::new(256, 1))),
            color_ramp_global_state: None,
            color_ramp_texture: None,
            dashes: LineAtlas::default(),
        }
    }

    fn evaluate(&mut self, cx: EvaluateCx<'_, Self>) -> Evaluation {
        let parameters = cx.parameters;
        // Build the ramp here rather than in transition(), which has no global
        // state. A new or changed layer always needs one; otherwise only a
        // change to a state key that the ramp reads does.
        let global_state_differs = match (&self.color_ramp_global_state, &parameters.global_state) {
            (None, None) => false,
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            _ => true,
        };
        let ramp_value = cx.values.unevaluated.line_gradient.get_value();
        let ramp_affected = parameters.layer_changed
            || (global_state_differs
                && ramp_value.get_dependencies().intersects(Dependency::GLOBAL_STATE)
                && global_state_refs_intersect(
                    ramp_value.get_global_state_refs(),
                    parameters.changed_global_state_keys.as_deref(),
                ));
        self.color_ramp_global_state = parameters.global_state.clone();
        if ramp_affected {
            self.update_color_ramp(cx.values);
        }
        let evaluated = &cx.evaluated.paint;
        let drapes_by_bearing = *evaluated.line_translate_anchor.uniform() == TranslateAnchorType::Viewport
            && *evaluated.line_translate.uniform() != [0.0, 0.0];
        let evaluation = if *evaluated.line_layer_opacity.uniform() > 0.0
            && evaluated.line_opacity.constant_or(&1.0) > 0.0
            && evaluated.line_color.constant_or(&Color::black()).a > 0.0
            && evaluated.line_width.constant_or(&1.0) > 0.0
        {
            Evaluation::TRANSLUCENT
        } else {
            Evaluation::NONE
        };
        Evaluation { drapes_by_bearing, ..evaluation }
    }

    /// A tile's draw stays while it draws in the layer's passes and, of a
    /// gradient, reads the layer's color ramp.
    fn keeps(&self, values: &LayerValues<Line>, tile: &LineTile) -> bool {
        tile.pass == render_pass(values)
            && !(matches!(tile.line_type, LineType::Gradient | LineType::GradientSdf)
                && tile.color_ramp != self.color_ramp_texture)
    }

    /// The draw of a tile's lines, if its bucket has any and the layer has
    /// what it draws them with.
    fn tile(
        &mut self,
        cx: &mut TileCx<'_, '_, Self>,
        tile: FrameTile<'_, Line>,
        _data: TileData<'_, Line>,
        _previous: Option<LineTile>,
    ) -> Option<LineTile> {
        let render_pass = render_pass(cx.values);
        let render_data = tile.data_for_pass(render_pass)?;
        let bucket = render_data.bucket;
        if bucket.triangles.is_empty() {
            return None;
        }
        let binders = bucket.paint_property_binders.get(&cx.base.style.base().id)?;
        self.build_tile(cx, render_data, binders, &render_data.layer.paint, render_pass)
    }

    /// Pushes the frame's draws of the layer, those of each of its tiles.
    fn prepare(&mut self, cx: &mut PrepareCx<'_, '_, Self>, render_tiles: &[FrameTile<'_, Line>], out: &mut Draws) {
        self.dashes.upload(cx.res.gpu, cx.res.context);
        let paint = &cx.values.evaluated.paint;
        out.main().composite(Composite::of(*paint.line_layer_opacity.uniform(), *paint.line_layer_blend.uniform()));

        let render_pass = render_pass(cx.values);
        for tile in render_tiles {
            let tile_id = tile.draw_id();

            let Some(render_data) = tile.data_for_pass(render_pass) else {
                continue;
            };
            let bucket = render_data.bucket;
            let Some(binders) = bucket.paint_property_binders.get(&cx.base.style.base().id) else {
                continue;
            };
            let evaluated = &render_data.layer.paint;

            let Some(mut entry) = cx.tiles.entry(&tile.id()) else {
                continue;
            };
            // The draw's attributes read the bucket's vertices in the
            // context's store, which upload again when they change.
            let changes = entry.take_changes();
            let Some(cached) = entry.value.as_mut() else {
                continue;
            };
            let context = &mut *cx.res.context;
            refresh_paint_attributes(&cached.mesh, context, changes, |context, attrs| {
                let buffer = &binders.interleaved_vertex_buffer;
                let paint = paint_vertices(context, render_data.geometry(), buffer, binders.depends_on_feature_state());
                binders.bind(evaluated, paint.as_ref(), attrs, &mut Uniforms::default());
            });
            if let Some((blocks, projection)) = Self::uniforms(
                &cx.values.evaluated,
                tile_id.to_unwrapped(),
                render_data,
                binders,
                cached,
                cx.frame,
                context,
                &self.dashes,
            ) {
                // The draws draw in the order their meshes were made.
                let draw = Draw {
                    order: cached.mesh.seq(),
                    opaque: cached.pass & RenderPass::Opaque != RenderPass::None,
                    clip: true,
                    ..Draw::DEFAULT
                };
                blocks.push(out.main(), &cached.mesh, tile_id, &projection, draw);
            }
        }

        out.main().block(&Self::props(&cx.values.evaluated));
    }

    /// Adds the dash patterns of the tiles' buckets to the layer's dash
    /// textures, then the layer's own, which the encode draws a dashed tile
    /// with when its bucket has no per-feature patterns.
    fn update(&mut self, cx: UpdateCx<'_, '_, Self>, tiles: &[FrameTile<'_, Line>]) {
        for tile in tiles {
            let Some(bucket) = tile.data() else {
                continue;
            };
            let dasharray = bucket.layer.paint.line_dasharray.faded();
            if dasharray.from.is_empty() {
                continue;
            }
            let cap = dash_pattern_cap(bucket.bucket);
            self.dashes.add(dasharray.from, dasharray.to, cap);
        }
        // A dashed draw asks for the layer's pattern even where it is empty.
        let layer_dasharray = cx.values.evaluated.paint.line_dasharray.faded();
        for tile in tiles {
            let Some(bucket) = tile.data() else {
                continue;
            };
            let laid_out = &bucket.layer.paint.line_dasharray;
            let dashed = !laid_out.is_constant() || !laid_out.faded().from.is_empty();
            let per_feature = bucket
                .bucket
                .paint_property_binders
                .get(&cx.values.style.base().id)
                .is_some_and(|binders| binders.line_dasharray.atlas().is_some());
            if dashed && !per_feature {
                let cap = dash_pattern_cap(bucket.bucket);
                self.dashes.add(layer_dasharray.from, layer_dasharray.to, cap);
            }
        }
    }

    // query
    fn query_intersects_feature(
        &self,
        values: &LayerValues<Line>,
        region: &QueryRegion<'_>,
        feature: &dyn mln_style::feature::GeometryTileFeature,
        geometry: &mln_style::feature::GeometryCollection,
        feature_state: &mln_style::feature::FeatureState,
    ) -> bool {
        let QueryRegion { geometry: query_geometry, view, pixels_to_tile_units, .. } = *region;
        let zoom = view.zoom() as f32;
        let evaluated = &values.evaluated.paint;
        // Translate query geometry
        let translated_query_geometry = crate::query::feature_index::FeatureIndex::translate_query_geometry(
            query_geometry,
            *evaluated.line_translate.uniform(),
            *evaluated.line_translate_anchor.uniform(),
            view.angle() as f32,
            pixels_to_tile_units,
        );
        let query = translated_query_geometry.as_ref().unwrap_or(query_geometry);

        // Evaluate function
        let offset = evaluated.line_offset.evaluate(&EvaluationContext::with_zoom_feature_state(
            zoom,
            Some(feature),
            Some(feature_state),
        )) * pixels_to_tile_units;
        // Test intersection
        let half_width = (f64::from(Self::line_width(evaluated, feature, zoom, feature_state)) / 2.0
            * f64::from(pixels_to_tile_units)) as f32;

        // Apply offset to geometry
        if offset != 0.0 && !geometry.is_empty() {
            return crate::geo::intersection_tests::polygon_intersects_buffered_multi_line(
                query,
                &offset_line(geometry, f64::from(offset)),
                half_width,
            );
        }

        crate::geo::intersection_tests::polygon_intersects_buffered_multi_line(query, geometry, half_width)
    }
}

impl RenderLineLayer {
    /// The draw of the tile's `bucket`, laid out with `evaluated`, in
    /// `pass`, if the layer has what it draws with.
    fn build_tile(
        &mut self,
        cx: &mut TileCx<'_, '_, Self>,
        tile_bucket: TileData<'_, Line>,
        binders: &LineBinders,
        evaluated: &LinePaintProperties::PossiblyEvaluated,
        pass: RenderPass,
    ) -> Option<LineTile> {
        let (gpu, context) = (&mut *cx.res.gpu, &mut *cx.res.context);
        let (bucket, geometry) = (tile_bucket.bucket, tile_bucket.geometry());
        let buffer = &binders.interleaved_vertex_buffer;
        let paint = paint_vertices(context, geometry, buffer, binders.depends_on_feature_state());
        let mut uniforms = Uniforms::default();
        let mut attrs = VertexAttributeArray::new();
        binders.bind(evaluated, paint.as_ref(), &mut attrs, &mut uniforms);

        let has_dasharray =
            !evaluated.line_dasharray.is_constant() || !evaluated.line_dasharray.faded().from.is_empty();
        // A constant dash array's texture is the layer's, bound with the
        // draw's uniforms.
        let dash_texture = if evaluated.line_dasharray.is_constant() {
            None
        } else {
            Some(dash_atlas_texture(gpu, context, binders)?)
        };
        let has_gradient = !cx.values.unevaluated.line_gradient.get_value().is_undefined();

        let mut textures = Textures::default();
        let mut atlases = None;
        let (line_type, program) = if has_dasharray && has_gradient {
            textures[IMAGE_SLOT] = Some(self.color_ramp_texture(context)?);
            textures[DASH_SLOT] = dash_texture;
            (LineType::GradientSdf, &programs::LINE_GRADIENT_SDF)
        } else if has_dasharray {
            textures[IMAGE_SLOT] = dash_texture;
            (LineType::Sdf, &programs::LINE_SDF)
        } else if !cx.values.unevaluated.line_pattern.is_undefined() {
            let icons = Some(tile_bucket.atlas_textures()).filter(|atlases| atlases.icon().is_some())?;
            atlases = Some(AtlasBinding::new(
                Some(icons.clone()),
                None,
                Some(IMAGE_SLOT),
                /*is_text*/ false,
                /*sdf_icons*/ true, // to force linear filter
                AlignmentType::Auto,
                /*icon_scaled*/ false,
                /*text_size_is_zoom_constant*/ false,
            ));
            (LineType::Pattern, &programs::LINE_PATTERN)
        } else if has_gradient {
            textures[IMAGE_SLOT] = Some(self.color_ramp_texture(context)?);
            (LineType::Gradient, &programs::LINE_GRADIENT)
        } else {
            (LineType::Simple, &programs::LINE)
        };
        let shader = context.shader_key(program, uniforms, cx.base.projection_variant);
        if let Some(atlases) = &atlases {
            // A pattern binds the atlas's texture and no other.
            textures = atlases.textures(&shader);
        }

        let shared = bucket_vertices(geometry, &bucket.vertices)?;
        let stride = size_of::<LineLayoutVertex>() as u32;
        attrs.set_shared(
            "pos_normal",
            SharedAttribute::new(
                shared.clone(),
                AttributeDataType::Short2,
                offset_of!(LineLayoutVertex, a1) as u32,
                stride,
            ),
        );
        attrs.set_shared(
            "data",
            SharedAttribute::new(shared, AttributeDataType::UByte4, offset_of!(LineLayoutVertex, a2) as u32, stride),
        );

        let draw_geometry = DrawGeometry {
            vertices: attrs,
            vertex_count: if bucket.segments.is_empty() { 0 } else { bucket.vertices.len() },
            mode: DrawMode::Triangles(),
            indexes: bucket_indexes(geometry, &bucket.triangles)?.into(),
            segments: bucket.segments.iter().map(SegmentBase::copy).collect(),
            instances: None,
        };
        let color_mode =
            if pass == RenderPass::Translucent { ColorMode::alpha_blended() } else { ColorMode::unblended() };
        let color_ramp = matches!(line_type, LineType::Gradient | LineType::GradientSdf)
            .then(|| textures[IMAGE_SLOT].clone())
            .flatten();
        let mesh = Mesh::new(context, shader, color_mode, CullFaceMode::disabled(), draw_geometry, textures);
        mesh.set_atlases(context, atlases);
        Some(LineTile { mesh, line_type, pass, dash_cap: dash_pattern_cap(bucket), color_ramp })
    }

    /// The texture of the color ramp, which every gradient of the layer
    /// shares, if the ramp is valid.
    fn color_ramp_texture(&mut self, context: &mut Context) -> Option<SharedTexture> {
        if self.color_ramp_texture.is_none() && self.color_ramp.valid() {
            self.color_ramp_texture = Some(context.add_texture_with(|texture| {
                texture.set_image(Some(Rc::clone(&self.color_ramp)));
                texture.set_sampler(SamplerState {
                    filter: TextureFilterType::Linear,
                    wrap_u: TextureWrapType::Clamp,
                    wrap_v: TextureWrapType::Clamp,
                    ..Default::default()
                });
            }));
        }
        self.color_ramp_texture.clone()
    }

    /// The blocks and the projection of `cached`, the draw of
    /// the tile `tile_id` of `tile_bucket`, whose binders are `binders`, with
    /// the layer's values
    /// `layer`: its matrix, the interpolation of the layer's values, and
    /// where its patterns are, in textures of `context`. A dashed draw of a
    /// constant dash array takes the layer's texture of it here; `None`
    /// where it has none yet, and doesn't draw.
    #[expect(
        clippy::too_many_arguments,
        reason = "the layer, the draw, its tile and bucket, the frame, the context and the dashes"
    )]
    fn uniforms(
        layer: &EvaluatedLayer<Line>,
        tile_id: UnwrappedTileID,
        tile_bucket: TileData<'_, Line>,
        binders: &LineBinders,
        cached: &LineTile,
        frame: &LayerFrame<'_>,
        context: &mut Context,
        line_atlas: &LineAtlas,
    ) -> Option<(LineBlocks, ProjectionUBO)> {
        const NEAR_CLIPPED: bool = false;
        const IN_VIEWPORT_PIXEL_UNITS: bool = false;
        let evaluated = &layer.paint;
        let crossfade = layer.crossfade;
        let state = frame.view;
        let zoom = state.zoom() as f32;
        let int_zoom = state.integer_zoom();
        let pixel_scale = state.projection().pixel_scale(state) as f32;

        let projection = projection_data(
            &tile_id,
            frame,
            *evaluated.line_translate.uniform(),
            *evaluated.line_translate_anchor.uniform(),
            NEAR_CLIPPED,
            IN_VIEWPORT_PIXEL_UNITS,
            None,
            false,
        );
        let matrix = cast_matrix(&projection.fallback_matrix);
        let projection = to_projection_ubo(&projection);
        let ratio = pixel_scale / tile_id.pixels_to_tile_units(1.0, zoom);

        let blocks = match cached.line_type {
            LineType::Simple => LineBlocks::Simple(LineDrawableUBO {
                matrix,
                ratio,
                color_t: binders.line_color.interpolation_factor(zoom),
                blur_t: binders.line_blur.interpolation_factor(zoom),
                opacity_t: binders.line_opacity.interpolation_factor(zoom),
                gapwidth_t: binders.line_gap_width.interpolation_factor(zoom),
                offset_t: binders.line_offset.interpolation_factor(zoom),
                width_t: binders.line_width.interpolation_factor(zoom),
                pad1: 0.0,
            }),
            LineType::Gradient => LineBlocks::Gradient(LineGradientDrawableUBO {
                matrix,
                ratio,
                blur_t: binders.line_blur.interpolation_factor(zoom),
                opacity_t: binders.line_opacity.interpolation_factor(zoom),
                gapwidth_t: binders.line_gap_width.interpolation_factor(zoom),
                offset_t: binders.line_offset.interpolation_factor(zoom),
                width_t: binders.line_width.interpolation_factor(zoom),
                pad1: 0.0,
                pad2: 0.0,
            }),
            LineType::Pattern => {
                let texture = cached.mesh.texture(context, IMAGE_SLOT).and_then(|texture| context.texture(texture));
                let texture_size = texture.map(Texture2D::size).unwrap_or_default();
                let drawable = LinePatternDrawableUBO {
                    matrix,
                    ratio,
                    blur_t: binders.line_blur.interpolation_factor(zoom),
                    opacity_t: binders.line_opacity.interpolation_factor(zoom),
                    gapwidth_t: binders.line_gap_width.interpolation_factor(zoom),
                    offset_t: binders.line_offset.interpolation_factor(zoom),
                    width_t: binders.line_width.interpolation_factor(zoom),
                    pattern_from_t: binders.line_pattern.interpolation_factor(zoom).0,
                    pattern_to_t: binders.line_pattern.interpolation_factor(zoom).1,
                    floorwidth_t: binders.line_floor_width.interpolation_factor(zoom),
                    pad1: 0.0,
                    pad2: 0.0,
                    pad3: 0.0,
                };
                let pattern = evaluated.line_pattern.faded();
                let tlbr = |id: &str| tile_bucket.pattern(id).map_or([0.0; 4], |position| cast_tlbr(position.tlbr()));
                let tile_props = LinePatternTilePropsUBO {
                    pattern_from: tlbr(pattern.from.id()),
                    pattern_to: tlbr(pattern.to.id()),
                    scale: [
                        frame.pixel_ratio,
                        1.0 / tile_id.pixels_to_tile_units(1.0, f32::from(int_zoom)),
                        crossfade.from_scale,
                        crossfade.to_scale,
                    ],
                    texsize: [texture_size.width as f32, texture_size.height as f32],
                    fade: crossfade.t,
                    pad1: 0.0,
                };
                LineBlocks::Pattern(drawable, tile_props)
            }
            LineType::Sdf | LineType::GradientSdf => {
                // The gradient keeps its ramp in the image slot, so the dash
                // pattern takes the second.
                let dash_slot = if cached.line_type == LineType::GradientSdf { DASH_SLOT } else { IMAGE_SLOT };
                let tile_ratio = |scale: f32| 1.0 / tile_id.pixels_to_tile_units(scale, f32::from(int_zoom));
                let sdf = if let Some(atlas) = binders.line_dasharray.atlas() {
                    // Per-feature patterns don't fade from the zoom before:
                    // both samples are of this zoom's pattern, at the
                    // cross-fade's two scales.
                    let atlas_size = atlas.image().size;
                    let min_scale = crossfade.from_scale.min(crossfade.to_scale);
                    SdfPatterns {
                        patternscale_a: [tile_ratio(crossfade.from_scale), 0.0],
                        patternscale_b: [tile_ratio(crossfade.to_scale), 0.0],
                        tex_y_a: 0.0,
                        tex_y_b: 0.0,
                        atlas_height: atlas_size.height as f32,
                        sdfgamma: atlas_size.width as f32 / (min_scale * 256.0 * frame.pixel_ratio) / 2.0,
                    }
                } else {
                    let dasharray = evaluated.line_dasharray.faded();
                    let dash_pattern = line_atlas.get(dasharray.from, dasharray.to, cached.dash_cap)?;
                    cached.mesh.set_texture(context, dash_slot, Some(dash_pattern.texture()?));

                    let pos_a = dash_pattern.from();
                    let pos_b = dash_pattern.to();
                    let width_a = pos_a.width * crossfade.from_scale;
                    let width_b = pos_b.width * crossfade.to_scale;
                    let min_width = if width_b < width_a { width_b } else { width_a };
                    SdfPatterns {
                        patternscale_a: [tile_ratio(width_a), -pos_a.height / 2.0],
                        patternscale_b: [tile_ratio(width_b), -pos_b.height / 2.0],
                        tex_y_a: pos_a.y,
                        tex_y_b: pos_b.y,
                        atlas_height: 0.0,
                        sdfgamma: dash_pattern.size().width as f32 / (min_width * 256.0 * frame.pixel_ratio) / 2.0,
                    }
                };

                let drawable = LineSDFDrawableUBO {
                    matrix,
                    patternscale_a: sdf.patternscale_a,
                    patternscale_b: sdf.patternscale_b,
                    tex_y_a: sdf.tex_y_a,
                    tex_y_b: sdf.tex_y_b,
                    ratio,
                    color_t: binders.line_color.interpolation_factor(zoom),
                    blur_t: binders.line_blur.interpolation_factor(zoom),
                    opacity_t: binders.line_opacity.interpolation_factor(zoom),
                    gapwidth_t: binders.line_gap_width.interpolation_factor(zoom),
                    offset_t: binders.line_offset.interpolation_factor(zoom),
                    width_t: binders.line_width.interpolation_factor(zoom),
                    floorwidth_t: binders.line_floor_width.interpolation_factor(zoom),
                    atlas_height: sdf.atlas_height,
                    pad2: 0.0,
                };
                let tile_props = LineSDFTilePropsUBO { sdfgamma: sdf.sdfgamma, mix: crossfade.t, pad1: 0.0, pad2: 0.0 };
                LineBlocks::Sdf(drawable, tile_props)
            }
        };
        Some((blocks, projection))
    }

    /// The block of the layer's values that its draws share.
    fn props(layer: &EvaluatedLayer<Line>) -> LineEvaluatedPropsUBO {
        let evaluated = &layer.paint;
        LineEvaluatedPropsUBO {
            color: evaluated.line_color.constant_or(&LinePaintProperties::Defaults::line_color()).to_float_array(),
            blur: evaluated.line_blur.constant_or(&LinePaintProperties::Defaults::line_blur()),
            opacity: evaluated.line_opacity.constant_or(&LinePaintProperties::Defaults::line_opacity()),
            gapwidth: evaluated.line_gap_width.constant_or(&LinePaintProperties::Defaults::line_gap_width()),
            offset: evaluated.line_offset.constant_or(&LinePaintProperties::Defaults::line_offset()),
            width: evaluated.line_width.constant_or(&LinePaintProperties::Defaults::line_width()),
            floorwidth: evaluated.line_floor_width.constant_or(&LinePaintProperties::Defaults::line_floor_width()),
            pad0: 0.0,
            pad1: 0.0,
        }
    }
}
