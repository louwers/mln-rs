// layers: debug: the debug overlays of a source's tiles, the tile borders
// and the parse status and timestamp text, drawn as polylines by the line
// shader.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use geometry::Point;
use glam::Vec2;
use mln_gfx::index_vector::IndexVectorBase;
use mln_gfx::polyline_generator::{PolylineGenerator, PolylineGeneratorOptions};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shader_group::StringIDSetsPair;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::line_layer_ubo::{
    LINE_EXPRESSION_UBO_SIZE, LineDrawableUBO, LineDrawableUnionUBO, LineEvaluatedPropsUBO, line_expression_mask,
};
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{
    ID_LINE_BLUR_VERTEX_ATTRIBUTE, ID_LINE_COLOR_VERTEX_ATTRIBUTE, ID_LINE_DATA_VERTEX_ATTRIBUTE, ID_LINE_DRAWABLE_UBO,
    ID_LINE_EVALUATED_PROPS_UBO, ID_LINE_EXPRESSION_UBO, ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE,
    ID_LINE_OFFSET_VERTEX_ATTRIBUTE, ID_LINE_OPACITY_VERTEX_ATTRIBUTE, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE,
    ID_LINE_WIDTH_VERTEX_ATTRIBUTE, ID_PROJECTION_UBO,
};
use mln_gfx::shaders::shader_program::ShaderProgramPtr;
use mln_gfx::vertex_vector::{VertexVector, VertexVectorBasePtr};
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode, VertexAttributeArray};
use mln_map_types::MapDebugOptions;
use mln_style::Source;
use mln_style_types::{LineCapType, LineJoinType, TranslateAnchorType};
use mln_tile_id::OverscaledTileID;
use mln_tile_id::geometry_tile_data::GeometryCoordinates;
use mln_util::Color;
use mln_util::constants::EXTENT;
use mln_util::error::Error;
use mln_util::geometry::FeatureType;
use mln_util::hash::{HashMap, HashSet};
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;

use crate::bucket::UploadNeeds;
use crate::buckets::line_bucket::{LineBucket, LineLayoutVertex};
use crate::draw::{DrawGeometry, DrawObjects, Textures};
use crate::draw_cache::{DrawDepth, DrawSlots, Draws, FrameDraw, KeptDraw};
use crate::layer_type::ErasedEvaluatedLayer;
use crate::layers::cast_matrix;
use crate::paint_parameters::PaintParameters;
use crate::query::FeatureExtensionValue;
use crate::render_source::{RenderSource, RenderSourceBase, SourcePrepareParameters, TileOrder};
use crate::render_tile::RenderTile;
use crate::render_tree::{DebugLayers, DebugType, debug_layer};
use crate::source_type::{OriginUpdate, TileOrigin};
use crate::tile::job::SourceKey;
use crate::tile::source_tiles::{CoverUpdate, SourceTiles};
use crate::tile::store::TileStore;
use crate::tile::tile::TileKind;
// layers: debug
use crate::tile_matrix::{get_projection_data, to_projection_ubo};
use crate::tile_parameters::TileParameters;
use crate::util::subdivision::subdivide_vertex_line;

/// The line properties of a debug overlay's polylines: red tile borders, and
/// black text over a white outline (layers: debug).
#[must_use]
fn debug_line_properties(type_: DebugType) -> LineEvaluatedPropsUBO {
    let (color, width) = match type_ {
        DebugType::TextOutline => (Color::white(), 4.0),
        DebugType::Text => (Color::black(), 2.0),
        DebugType::Border => (Color::red(), 4.0),
    };
    LineEvaluatedPropsUBO {
        color: color.to_float_array(),
        blur: 0.0,
        opacity: 1.0,
        gapwidth: 0.0,
        offset: 0.0,
        width,
        floorwidth: 0.0,
        expression_mask: line_expression_mask::NONE,
        pad1: 0.0,
    }
}

/// The GPU objects of `lines`, as polylines of `options` drawn with `shader`;
/// `None` where they have no vertices.
fn polyline_objects(
    shader: &ShaderProgramPtr,
    lines: &[GeometryCoordinates],
    options: &PolylineGeneratorOptions,
) -> Option<DrawObjects> {
    let mut vertices: Vec<LineLayoutVertex> = Vec::new();
    let mut segments: Vec<SegmentBase> = Vec::new();
    let mut indexes: Vec<u16> = Vec::new();
    for line in lines {
        let mut generator = PolylineGenerator::new(
            &mut vertices,
            LineBucket::layout_vertex,
            &mut segments,
            &SegmentBase::new,
            |segment: &mut SegmentBase| segment,
            &mut indexes,
        );
        generator.generate(line, options);
    }
    if vertices.is_empty() {
        return None;
    }

    let vertex_count = vertices.len();
    let stride = size_of::<LineLayoutVertex>() as u32;
    let shared: VertexVectorBasePtr = Rc::new(VertexVector::from_vec(vertices));
    let mut attrs = VertexAttributeArray::new();
    // The position and normal, then the extrusion data (offsetof(LineLayoutVertex, a2)).
    for (id, offset, data_type) in [
        (ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, 0, AttributeDataType::Short2),
        (ID_LINE_DATA_VERTEX_ATTRIBUTE, 4, AttributeDataType::UByte4),
    ] {
        if let Some(attr) = attrs.set(id, -1, AttributeDataType::Invalid, 0) {
            attr.set_shared_raw_data(Some(Rc::clone(&shared)), offset, 0, stride, data_type);
        }
    }

    let geometry = DrawGeometry {
        vertices: attrs,
        vertex_count,
        mode: DrawMode::Triangles(),
        indexes: Rc::new(RefCell::new(IndexVectorBase::new(indexes))),
        segments,
        instances: None,
    };
    Some(DrawObjects::new(
        Rc::clone(shader),
        ColorMode::alpha_blended(),
        CullFaceMode::disabled(),
        geometry,
        Textures::default(),
    ))
}

// layers: debug --------------------------------------------------------------

/// A draw of a debug overlay: polylines over a tile of a source.
struct DebugDraw {
    /// The id of the source.
    source: Rc<str>,
    tile: OverscaledTileID,
    /// The lines of text the polylines spell, of a text overlay.
    text: Option<Vec<String>>,
    draw: KeptDraw,
}

/// The draws of a debug overlay, in the order they were made, which is the
/// order they draw in, each in the slot of its index.
#[derive(Default)]
pub(crate) struct DebugDraws(Vec<DebugDraw>);

impl DrawSlots for DebugDraws {
    fn slot_mut(&mut self, slot: u32) -> Option<&mut DrawObjects> {
        self.0.get_mut(slot as usize).map(|draw| &mut draw.draw.objects)
    }
}

/// A debug overlay: polylines over the tiles of the sources, in the color
/// and width of its type (layers: debug).
pub(crate) struct DebugLayer {
    properties: LineEvaluatedPropsUBO,
    /// The overlay's draws, and the frame's draws of them, with the buffers
    /// they read.
    pub(crate) draws: Draws<(), DebugDraws>,
}

impl DebugLayer {
    #[must_use]
    pub(crate) fn new(type_: DebugType) -> Self {
        DebugLayer { properties: debug_line_properties(type_), draws: Draws::default() }
    }

    /// The overlay's draws, in drawing order.
    fn draws(&mut self) -> &mut Vec<DebugDraw> {
        &mut self.draws.caches.layer_cache_mut().0
    }

    /// The tiles with draws of the source `source`.
    fn tiles_of(&mut self, source: &str) -> HashSet<OverscaledTileID> {
        self.draws().iter().filter(|draw| *draw.source == *source).map(|draw| draw.tile).collect()
    }

    /// Adds a draw of `objects` over `tile_id` of the source `source`,
    /// spelling `text`.
    fn add(&mut self, source: &Rc<str>, tile_id: OverscaledTileID, text: Option<Vec<String>>, objects: DrawObjects) {
        let draw = KeptDraw::new(objects);
        self.draws().push(DebugDraw { source: Rc::clone(source), tile: tile_id, text, draw });
    }

    /// Makes the frame's draws of the overlay, and writes the buffers they
    /// read.
    pub(crate) fn prepare(&mut self, parameters: &PaintParameters<'_>) {
        let context = parameters.context;
        let Draws { caches, frame } = &mut self.draws;
        frame.draws.clear();
        let draws = &caches.layer_cache().0;
        if draws.is_empty() {
            return;
        }

        let buffers = &mut frame.uniform_buffers;
        buffers.create_or_update_pod(ID_LINE_EVALUATED_PROPS_UBO, &self.properties, context);
        // We would need to set up `idLineExpressionUBO` if the expression
        // mask isn't empty
        debug_assert_eq!(self.properties.expression_mask, line_expression_mask::NONE);
        // No GPU expressions; only line shaders of `Caps::line_expressions`
        // read it.
        if context.caps().line_expressions && buffers.get(ID_LINE_EXPRESSION_UBO).is_none() {
            buffers.set(ID_LINE_EXPRESSION_UBO, context.create_uniform_buffer(&[0; LINE_EXPRESSION_UBO_SIZE]));
        }

        let zoom = parameters.state.get_zoom();
        let mut draw_ubos = Vec::with_capacity(draws.len());
        let mut projection_ubos: Vec<ProjectionUBO> = Vec::with_capacity(draws.len());
        for (slot, draw) in draws.iter().enumerate() {
            let tile_id = draw.tile.to_unwrapped();
            let projection = get_projection_data(
                &tile_id,
                parameters,
                [0.0, 0.0],
                TranslateAnchorType::Viewport,
                false,
                false,
                None,
                false,
            );
            projection_ubos.push(to_projection_ubo(&projection));
            let mut draw_ubo = LineDrawableUnionUBO::default();
            draw_ubo.set(&LineDrawableUBO {
                matrix: cast_matrix(&projection.fallback_matrix),
                ratio: 1.0 / tile_id.pixels_to_tile_units(1.0, zoom as f32),
                color_t: 0.0,
                blur_t: 0.0,
                opacity_t: 0.0,
                gapwidth_t: 0.0,
                offset_t: 0.0,
                width_t: 0.0,
                pad1: 0.0,
            });
            draw_ubos.push(draw_ubo);
            let frame_draw = FrameDraw::new(None, slot as u32, draw.draw.id);
            frame.draws.push(FrameDraw { depth: DrawDepth::Off, ..frame_draw });
        }
        let buffers = &mut frame.uniform_buffers;
        buffers.write_storage(ID_LINE_DRAWABLE_UBO, bytemuck::cast_slice(&draw_ubos), context);
        buffers.write_storage(ID_PROJECTION_UBO, bytemuck::cast_slice(&projection_ubos), context);
    }
}

/// The render source of a source with tiles: a view into the tile store,
/// which keeps the tiles, with the render tiles of the last `prepare`.
pub(crate) struct RenderTileSource {
    pub(crate) base: RenderSourceBase,
    /// The source's id, which its debug overlays' draws carry.
    id: Rc<str>,
    key: SourceKey,
    origin: Box<dyn TileOrigin>,
    fast_pfor_enabled: bool,
    pub(crate) render_tiles: Vec<RenderTile>,
    /// The render tiles in each [`TileOrder`], as indexes into them.
    order_by_id: Vec<usize>,
    order_by_y: Vec<usize>,
}

impl RenderTileSource {
    /// Sorts the render tiles into each [`TileOrder`], seen at `bearing`.
    fn sort_tiles(&mut self, bearing: f32) {
        let tiles = &self.render_tiles;
        self.order_by_id.clear();
        self.order_by_id.extend((0..tiles.len()).filter(|&i| !tiles[i].hold_for_fade()));
        let rotation = Vec2::from_angle(bearing);
        self.order_by_y.clear();
        self.order_by_y.extend(0..tiles.len());
        self.order_by_y.sort_by(|&a, &b| {
            let (a, b) = (&tiles[a].id.canonical, &tiles[b].id.canonical);
            let par = rotation.rotate(Vec2::new(a.x as f32, a.y as f32));
            let pbr = rotation.rotate(Vec2::new(b.x as f32, b.y as f32));
            // std::tie(b.z, par.y, par.x) < std::tie(a.z, pbr.y, pbr.x)
            b.z.cmp(&a.z).then(par.y.total_cmp(&pbr.y)).then(par.x.total_cmp(&pbr.x))
        });
    }

    /// The render source of `style_source`, whose tiles of `kind` go in `store`
    /// and get their data through `origin`. A tile of a `single_layer`
    /// source holds one source layer.
    pub(crate) fn new(
        style_source: Source,
        kind: TileKind,
        single_layer: bool,
        origin: Box<dyn TileOrigin>,
        store: &mut TileStore,
    ) -> Self {
        let key = store.add_source(style_source.id().to_owned(), kind, single_layer);
        RenderTileSource {
            id: Rc::from(style_source.id()),
            base: RenderSourceBase::new(style_source),
            key,
            origin,
            fast_pfor_enabled: false,
            render_tiles: Vec::new(),
            order_by_id: Vec::new(),
            order_by_y: Vec::new(),
        }
    }
}

impl RenderSource for RenderTileSource {
    fn base(&self) -> &RenderSourceBase {
        &self.base
    }

    fn is_loaded(&self, store: &TileStore) -> bool {
        store.source(self.key).is_none_or(SourceTiles::is_loaded)
    }

    fn update(
        &mut self,
        store: &mut TileStore,
        style_source: Source,
        layers: &[Arc<dyn ErasedEvaluatedLayer>],
        needs_rendering: bool,
        needs_relayout: bool,
        parameters: &TileParameters<'_>,
    ) {
        let previous = std::mem::replace(&mut self.base.style_source, style_source);
        self.base.enabled = needs_rendering;
        let Some((source, ctx)) = store.source_mut(self.key) else {
            return;
        };
        let origin_update = OriginUpdate { parameters, needs_relayout, fast_pfor_enabled: self.fast_pfor_enabled };
        let Some(cover) = self.origin.update(&previous, &self.base.style_source, source, ctx, &origin_update) else {
            return;
        };
        let update = CoverUpdate {
            layers,
            needs_rendering,
            needs_relayout,
            parameters,
            style_source: &self.base.style_source,
            tile_size: cover.tile_size,
            zoom_range: cover.zoom_range,
            bounds: cover.bounds,
        };
        source.update_cover(&update, ctx);
    }

    fn prepare(&mut self, store: &mut TileStore, parameters: &SourcePrepareParameters<'_>) {
        self.render_tiles.clear();
        if let Some(source) = store.source(self.key) {
            self.render_tiles.reserve(source.rendered_count());
            for rendered in source.rendered() {
                let mut render_tile =
                    RenderTile::new(*rendered.id, rendered.tile, rendered.slot, rendered.hold_for_fade);
                render_tile.prepare(rendered.tile, rendered.needs_rendering, parameters);
                self.render_tiles.push(render_tile);
            }
        }
        self.sort_tiles(parameters.transform.state.get_bearing() as f32);
    }

    fn release_render_data(&mut self) {
        self.render_tiles = Vec::new();
        self.order_by_id = Vec::new();
        self.order_by_y = Vec::new();
    }

    fn upload(&mut self, store: &mut TileStore, needs: UploadNeeds) {
        store.upload(self.key, &self.render_tiles, needs);
    }

    /// Updates the source's draws of the debug overlays (layers: debug).
    ///
    /// The text is polylines, like the tile borders: WebGPU draws line
    /// primitives one pixel wide.
    fn update_debug_draws(&self, debug_layers: &mut DebugLayers, parameters: &PaintParameters<'_>) {
        if !parameters
            .debug_options
            .contains(MapDebugOptions::TIMESTAMPS | MapDebugOptions::PARSE_STATUS | MapDebugOptions::TILE_BORDERS)
        {
            debug_layers.clear();
            return;
        }

        let shaders = &*parameters.static_data.shaders;
        let source = &self.id;
        let globe = parameters.state.is_globe_rendering();
        let projection_variant = if globe { ProjectionVariant::Globe } else { ProjectionVariant::Mercator };

        let Some(polyline_shader) = shaders.shader_group("LineShader").map(|shader_group| {
            let properties_as_uniforms: StringIDSetsPair = (
                ["a_color", "a_blur", "a_opacity", "a_gapwidth", "a_offset", "a_width"]
                    .into_iter()
                    .map(String::from)
                    .collect(),
                [
                    ID_LINE_COLOR_VERTEX_ATTRIBUTE,
                    ID_LINE_BLUR_VERTEX_ATTRIBUTE,
                    ID_LINE_OPACITY_VERTEX_ATTRIBUTE,
                    ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE,
                    ID_LINE_OFFSET_VERTEX_ATTRIBUTE,
                    ID_LINE_WIDTH_VERTEX_ATTRIBUTE,
                ]
                .into_iter()
                .collect(),
            );
            shader_group.shader(&properties_as_uniforms, projection_variant)
        }) else {
            return;
        };

        // The tiles to cover, whose draws stay where they were built with
        // the same shader and spell the tile's text: one of the other
        // projection keeps its shader.
        let new_tiles: HashMap<OverscaledTileID, &RenderTile> =
            self.render_tiles.iter().map(|tile| (*tile.get_overscaled_tile_id(), tile)).collect();
        let keep = |draw: &DebugDraw| {
            if draw.source != *source {
                return true;
            }
            let Some(tile) = new_tiles.get(&draw.tile) else {
                return false;
            };
            let same_shader = std::ptr::addr_eq(Rc::as_ptr(draw.draw.objects.shader()), Rc::as_ptr(&polyline_shader));
            let same_text = match (&draw.text, &tile.debug_bucket) {
                (Some(text), Some(debug_bucket)) => *text == debug_bucket.text,
                _ => true,
            };
            same_shader && same_text
        };

        // Timestamps or Parse Status
        if parameters.debug_options.contains(MapDebugOptions::TIMESTAMPS | MapDebugOptions::PARSE_STATUS) {
            let text_options = PolylineGeneratorOptions {
                join_type: LineJoinType::Round,
                begin_cap: LineCapType::Round,
                end_cap: LineCapType::Round,
                ..Default::default()
            };
            for type_ in [DebugType::TextOutline, DebugType::Text] {
                let layer = debug_layer(debug_layers, type_);
                layer.draws().retain(keep);

                // add draws of the tiles without any: new tiles, and those
                // whose text changed
                let mut of_tiles = layer.tiles_of(source);
                for tile in &self.render_tiles {
                    let tile_id = *tile.get_overscaled_tile_id();
                    let Some(debug_bucket) = &tile.debug_bucket else {
                        continue;
                    };
                    if tile.get_needs_rendering()
                        && !of_tiles.contains(&tile_id)
                        && let Some(objects) = polyline_objects(&polyline_shader, &debug_bucket.strokes, &text_options)
                    {
                        of_tiles.insert(tile_id);
                        layer.add(source, tile_id, Some(debug_bucket.text.clone()), objects);
                    }
                }
            }
        } else {
            // tile texts are not required, erase their overlays
            debug_layers.remove(&DebugType::TextOutline);
            debug_layers.remove(&DebugType::Text);
        }

        // Tile Borders
        if parameters.debug_options.contains(MapDebugOptions::TILE_BORDERS) {
            let layer = debug_layer(debug_layers, DebugType::Border);
            layer.draws().retain(keep);
            let mut of_tiles = layer.tiles_of(source);

            // add new draws
            let extent = EXTENT as i16;
            let tile_border: GeometryCoordinates = vec![
                Point::new(0, 0),
                Point::new(extent, 0),
                Point::new(extent, extent),
                Point::new(0, extent),
                Point::new(0, 0),
            ];
            // On the globe the edges bend with the sphere: the border
            // subdivided at the tile mesh's granularity, one per zoom.
            let mut globe_borders: BTreeMap<u8, GeometryCoordinates> = BTreeMap::new();
            let border_options = PolylineGeneratorOptions { type_: FeatureType::Polygon, ..Default::default() };
            for tile in &self.render_tiles {
                let tile_id = *tile.get_overscaled_tile_id();
                if tile.debug_bucket.is_none() || !tile.get_needs_rendering() {
                    continue;
                }
                if of_tiles.contains(&tile_id) {
                    continue;
                }
                let border = if globe {
                    let z = tile_id.canonical.z;
                    globe_borders.entry(z).or_insert_with(|| {
                        let steps = SubdivisionGranularitySetting::globe().tile.get_granularity_for_zoom_level(z);
                        subdivide_vertex_line(&tile_border, steps, false)
                    })
                } else {
                    &tile_border
                };
                if let Some(objects) = polyline_objects(&polyline_shader, std::slice::from_ref(border), &border_options)
                {
                    of_tiles.insert(tile_id);
                    layer.add(source, tile_id, None, objects);
                }
            }
        } else {
            // if tile borders are not required, erase their overlay
            debug_layers.remove(&DebugType::Border);
        }
    }

    fn tiles(&self) -> Option<SourceKey> {
        Some(self.key)
    }

    fn render_tiles(&self) -> &[RenderTile] {
        &self.render_tiles
    }

    fn tile_order(&self, order: TileOrder) -> Option<&[usize]> {
        Some(match order {
            TileOrder::ById => &self.order_by_id,
            TileOrder::ByY => &self.order_by_y,
        })
    }

    fn query_feature_extensions(
        &self,
        feature: &mln_util::feature::Feature,
        extension: &str,
        extension_field: &str,
        args: Option<&feature::PropertyMap>,
    ) -> Result<FeatureExtensionValue, Error> {
        self.origin.query_feature_extensions(feature, extension, extension_field, args)
    }

    fn set_fast_pfor_enabled(&mut self, enable: bool) {
        self.fast_pfor_enabled = enable;
    }

    fn dump_debug_logs(&self) {}

    fn get_max_zoom(&self) -> u8 {
        self.origin.max_zoom()
    }
}
