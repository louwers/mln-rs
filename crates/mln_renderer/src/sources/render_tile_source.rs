// The render source of a source with tiles, and the debug overlays of its
// tiles: the tile borders and the parse status and timestamp text, drawn as
// polylines by the line shader.

use std::collections::BTreeMap;
use std::mem::offset_of;
use std::rc::Rc;
use std::sync::Arc;

use geometry::Point;
use glam::Vec2;
use mln_gfx::gpu::{Gpu, ShaderKey};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::line_layer_ubo::{LineDrawableUBO, LineDrawableUnionUBO, LineEvaluatedPropsUBO};
use mln_gfx::shaders::programs;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{
    ID_LINE_BLUR_VERTEX_ATTRIBUTE, ID_LINE_COLOR_VERTEX_ATTRIBUTE, ID_LINE_DATA_VERTEX_ATTRIBUTE, ID_LINE_DRAWABLE_UBO,
    ID_LINE_EVALUATED_PROPS_UBO, ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE, ID_LINE_OFFSET_VERTEX_ATTRIBUTE,
    ID_LINE_OPACITY_VERTEX_ATTRIBUTE, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, ID_LINE_WIDTH_VERTEX_ATTRIBUTE,
    ID_PROJECTION_UBO,
};
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode};
use mln_map_types::MapDebugOptions;
use mln_style::Source;
use mln_style_types::{LineCapType, LineJoinType, TranslateAnchorType};
use mln_tile_id::OverscaledTileID;
use mln_tile_id::geometry_tile_data::GeometryCoordinates;
use mln_util::Color;
use mln_util::constants::EXTENT;
use mln_util::error::Error;
use mln_util::hash::{HashMap, HashSet};
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_vec_map::VecMap;

use crate::bucket::UploadNeeds;
use crate::draw::{DrawGeometry, DrawObjects, Textures};
use crate::gfx::context::Context;
use crate::gfx::indexes::DrawIndexes;
use crate::gfx::line_mesh::{LineLayoutVertex, LineOptions, LineScratch, add_line};
use crate::gfx::mesh::Indexed;
use crate::gfx::vertex_attribute::{SharedAttribute, VertexAttributeArray};
use crate::layer_drawing::{DrawDepth, DrawList, DrawObjectsMut, FrameDraw, KeptDraw, LayerDraws};
use crate::layer_type::ErasedEvaluatedLayer;
use crate::layers::cast_matrix;
use crate::paint_parameters::PaintParameters;
use crate::query::FeatureExtensionValue;
use crate::render_source::{RenderSource, RenderSourceBase, RenderTileParameters, TileOrder};
use crate::render_tile::RenderTile;
use crate::source_type::{OriginUpdate, TileOrigin};
use crate::tile::job::SourceKey;
use crate::tile::source_tiles::{CoverUpdate, SourceTiles};
use crate::tile::store::TileStore;
use crate::tile::tile::TileKind;
use crate::tile_matrix::{projection_data, to_projection_ubo};
use crate::tile_parameters::TileParameters;
use crate::util::subdivision::subdivide_vertex_line;

/// The line properties of a debug overlay's polylines: red tile borders, and
/// black text over a white outline.
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
        pad0: 0.0,
        pad1: 0.0,
    }
}

/// The GPU objects of `lines`, as polylines of `options` drawn with `shader`;
/// `None` where they have no vertices.
fn polyline_objects(
    context: &mut Context,
    shader: &ShaderKey,
    lines: &[GeometryCoordinates],
    options: &LineOptions,
) -> Option<DrawObjects> {
    let mut vertices: Vec<LineLayoutVertex> = Vec::new();
    let mut segments: Vec<SegmentBase> = Vec::new();
    let mut indexes: Vec<u16> = Vec::new();
    let mut scratch = LineScratch::default();
    for line in lines {
        let mut triangles = Indexed { indexes: &mut indexes, segments: &mut segments };
        add_line(&mut vertices, &mut triangles, &mut scratch, line, options);
    }
    if vertices.is_empty() {
        return None;
    }

    let vertex_count = vertices.len();
    let stride = size_of::<LineLayoutVertex>() as u32;
    let shared = context.add_vertices(vertices);
    let mut attrs = VertexAttributeArray::new();
    // The position and normal, then the extrusion data.
    for (id, offset, data_type) in [
        (ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, offset_of!(LineLayoutVertex, a1) as u32, AttributeDataType::Short2),
        (ID_LINE_DATA_VERTEX_ATTRIBUTE, offset_of!(LineLayoutVertex, a2) as u32, AttributeDataType::UByte4),
    ] {
        attrs.set_shared(id, SharedAttribute::new(shared.clone(), data_type, offset, stride));
    }

    let geometry = DrawGeometry {
        vertices: attrs,
        vertex_count,
        mode: DrawMode::Triangles(),
        indexes: DrawIndexes::new(indexes),
        segments,
        instances: None,
    };
    Some(DrawObjects::new(
        shader.clone(),
        ColorMode::alpha_blended(),
        CullFaceMode::disabled(),
        geometry,
        Textures::default(),
    ))
}

// Debug overlays -------------------------------------------------------------

/// A draw of a debug overlay: polylines over a tile of a source.
struct DebugDraw {
    /// The id of the source.
    source: Rc<str>,
    tile: OverscaledTileID,
    /// The lines of text the polylines spell, of a text overlay.
    text: Option<Vec<String>>,
    draw: KeptDraw,
}

/// A draw's entries in a debug overlay's storage buffers.
struct DebugUniforms {
    draw_ubo: LineDrawableUnionUBO,
    projection: ProjectionUBO,
}

/// The kinds of debug overlays.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum DebugType {
    TextOutline,
    Text,
    Border,
}

/// The debug overlays, by debug type.
pub(crate) type DebugLayers = VecMap<DebugType, DebugLayer>;

/// The debug overlay `type_` in `debug_layers`, made if it isn't there.
fn debug_layer(debug_layers: &mut DebugLayers, type_: DebugType) -> &mut DebugLayer {
    debug_layers.entry(type_).or_insert_with(|| DebugLayer::new(type_))
}

/// A debug overlay: polylines over the tiles of the sources, in the color
/// and width of its type.
pub(crate) struct DebugLayer {
    properties: LineEvaluatedPropsUBO,
    /// The overlay's draws, in the order they were made, which is the order
    /// they draw in.
    draws: Vec<DebugDraw>,
    /// The frame's draws as `update_draws` makes them.
    draw_list: DrawList<DebugUniforms, usize>,
    /// The frame's draws of the overlay, each of the overlay's draw at its
    /// index, with the buffers they read.
    frame: LayerDraws<usize>,
}

impl DebugLayer {
    #[must_use]
    pub(crate) fn new(type_: DebugType) -> Self {
        DebugLayer {
            properties: debug_line_properties(type_),
            draws: Vec::new(),
            draw_list: DrawList::default(),
            frame: LayerDraws::default(),
        }
    }

    /// The GPU objects of the overlay's draw at `index`.
    fn objects<'a>(draws: &'a mut [DebugDraw], _: &FrameDraw, index: usize) -> Option<DrawObjectsMut<'a>> {
        Some((&mut draws.get_mut(index)?.draw.objects).into())
    }

    /// Uploads the GPU objects of the frame's draws.
    pub(crate) fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        self.frame.upload(gpu, &mut self.draws[..], Self::objects, context);
    }

    /// Draws the frame's draws of the pass into the open pass of `gpu`, the
    /// context's.
    pub(crate) fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        self.frame.render(&mut self.draws[..], Self::objects, gpu, parameters);
    }

    /// Keeps the draws for which `keep` returns true.
    fn retain(&mut self, keep: impl FnMut(&DebugDraw) -> bool) {
        self.draws.retain(keep);
    }

    /// The tiles with draws of the source `source`.
    fn tiles_of(&self, source: &str) -> HashSet<OverscaledTileID> {
        self.draws.iter().filter(|draw| *draw.source == *source).map(|draw| draw.tile).collect()
    }

    /// Adds a draw of `objects` over `tile_id` of the source `source`,
    /// spelling `text`.
    fn add(&mut self, source: &Rc<str>, tile_id: OverscaledTileID, text: Option<Vec<String>>, objects: DrawObjects) {
        let draw = KeptDraw::new(objects);
        self.draws.push(DebugDraw { source: Rc::clone(source), tile: tile_id, text, draw });
    }

    /// Makes the frame's draws of the overlay, and writes the buffers they
    /// read.
    pub(crate) fn update_draws(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        let zoom = parameters.view.zoom();
        for (index, draw) in self.draws.iter().enumerate() {
            let tile_id = draw.tile.to_unwrapped();
            let projection = projection_data(
                &tile_id,
                parameters,
                [0.0, 0.0],
                TranslateAnchorType::Viewport,
                false,
                false,
                None,
                false,
            );
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
            let frame_draw = FrameDraw { depth: DrawDepth::Off, ..draw.draw.frame_draw(None) };
            let uniforms = DebugUniforms { draw_ubo, projection: to_projection_ubo(&projection) };
            self.draw_list.push_part(frame_draw, index, uniforms);
        }

        let properties = &self.properties;
        let replaced = self.draw_list.finish(&mut self.frame, gpu, parameters.context, |writer| {
            writer.uniform(ID_LINE_EVALUATED_PROPS_UBO, properties);
            writer.storage(ID_LINE_DRAWABLE_UBO, |uniforms| uniforms.draw_ubo);
            writer.storage(ID_PROJECTION_UBO, |uniforms| uniforms.projection);
        });
        if replaced {
            for draw in &mut self.draws {
                draw.draw.objects.drop_bindings();
            }
        }
    }
}

/// The render source of a source with tiles: a view into the tile store,
/// which keeps the tiles, with the render tiles of the last
/// `update_render_tiles`.
pub(crate) struct RenderTileSource {
    pub(crate) base: RenderSourceBase,
    /// The source's id, which its debug overlays' draws carry.
    id: Rc<str>,
    key: SourceKey,
    origin: Box<dyn TileOrigin>,
    fast_pfor_enabled: bool,
    render_tiles: Vec<RenderTile>,
    /// The render tiles in [`TileOrder::ById`], as indexes into them.
    order_by_id: Vec<usize>,
    /// The render tiles in [`TileOrder::ByY`], where the frame's
    /// [`RenderTileParameters::sort_by_y`] asked for them; empty
    /// otherwise.
    order_by_y: Vec<usize>,
}

impl RenderTileSource {
    /// Sorts the render tiles into `order_by_y` in [`TileOrder::ByY`], seen
    /// at `bearing`: the deeper zooms first, then by rotated y and x.
    fn sort_by_y(&mut self, bearing: f32) {
        let tiles = &self.render_tiles;
        let rotation = Vec2::from_angle(bearing);
        let order = &mut self.order_by_y;
        order.extend(0..tiles.len());
        order.sort_by(|&a, &b| {
            let (a, b) = (&tiles[a].id.canonical, &tiles[b].id.canonical);
            let pa = rotation.rotate(Vec2::new(a.x as f32, a.y as f32));
            let pb = rotation.rotate(Vec2::new(b.x as f32, b.y as f32));
            b.z.cmp(&a.z).then(pa.y.total_cmp(&pb.y)).then(pa.x.total_cmp(&pb.x))
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
        let source_id = self.base.style_source.id();
        let terrain_reads = parameters.terrain.filter(|terrain| terrain.source_id == source_id);
        let update = CoverUpdate {
            layers,
            needs_rendering,
            needs_relayout,
            terrain_reads: terrain_reads.map_or(&[], |terrain| &terrain.dem_tiles),
            parameters,
            style_source: &self.base.style_source,
            tile_size: cover.tile_size,
            zoom_range: cover.zoom_range,
            bounds: cover.bounds,
        };
        source.update_cover(&update, ctx);
    }

    fn update_render_tiles(&mut self, store: &mut TileStore, parameters: &RenderTileParameters<'_>) {
        self.render_tiles.clear();
        if let Some(source) = store.source(self.key) {
            self.render_tiles.reserve(source.rendered_count());
            for rendered in source.rendered() {
                let mut render_tile =
                    RenderTile::new(*rendered.id, rendered.tile, rendered.slot, rendered.hold_for_fade);
                render_tile.update(rendered.tile, rendered.needs_rendering, parameters);
                self.render_tiles.push(render_tile);
            }
        }
        let tiles = &self.render_tiles;
        self.order_by_id.clear();
        self.order_by_id.extend((0..tiles.len()).filter(|&i| !tiles[i].hold_for_fade()));
        self.order_by_y.clear();
        if parameters.sort_by_y {
            self.sort_by_y(parameters.transform.view.angle() as f32);
        }
    }

    fn release_render_data(&mut self) {
        self.render_tiles = Vec::new();
        self.order_by_id = Vec::new();
        self.order_by_y = Vec::new();
    }

    fn upload(&mut self, store: &mut TileStore, context: &mut Context, needs: UploadNeeds) {
        store.upload(self.key, &self.render_tiles, context, needs);
    }

    /// Updates the source's draws of the debug overlays.
    ///
    /// The text is polylines, like the tile borders: WebGPU draws line
    /// primitives one pixel wide.
    fn update_debug_draws(&self, debug_layers: &mut DebugLayers, parameters: &mut PaintParameters<'_>) {
        if !parameters
            .debug_options
            .contains(MapDebugOptions::TIMESTAMPS | MapDebugOptions::PARSE_STATUS | MapDebugOptions::TILE_BORDERS)
        {
            debug_layers.clear();
            return;
        }

        let source = &self.id;
        let globe = parameters.view.is_globe();
        let projection_variant = if globe { ProjectionVariant::Globe } else { ProjectionVariant::Mercator };

        let uniforms = [
            ID_LINE_COLOR_VERTEX_ATTRIBUTE,
            ID_LINE_BLUR_VERTEX_ATTRIBUTE,
            ID_LINE_OPACITY_VERTEX_ATTRIBUTE,
            ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE,
            ID_LINE_OFFSET_VERTEX_ATTRIBUTE,
            ID_LINE_WIDTH_VERTEX_ATTRIBUTE,
        ];
        let polyline_shader =
            parameters.context.shader_key(&programs::LINE, uniforms.into_iter().collect(), projection_variant);

        // The tiles to cover, whose draws stay where they were built with
        // the same shader and spell the tile's text: one of the other
        // projection keeps its shader.
        let new_tiles: HashMap<OverscaledTileID, &RenderTile> =
            self.render_tiles.iter().map(|tile| (*tile.overscaled_tile_id(), tile)).collect();
        let keep = |draw: &DebugDraw| {
            if draw.source != *source {
                return true;
            }
            let Some(tile) = new_tiles.get(&draw.tile) else {
                return false;
            };
            let same_shader = *draw.draw.objects.shader() == polyline_shader;
            let same_text = match (&draw.text, &tile.debug_bucket) {
                (Some(text), Some(debug_bucket)) => *text == debug_bucket.text,
                _ => true,
            };
            same_shader && same_text
        };

        // Timestamps or Parse Status
        if parameters.debug_options.contains(MapDebugOptions::TIMESTAMPS | MapDebugOptions::PARSE_STATUS) {
            let text_options = LineOptions { join: LineJoinType::Round, cap: LineCapType::Round, ..Default::default() };
            for type_ in [DebugType::TextOutline, DebugType::Text] {
                let layer = debug_layer(debug_layers, type_);
                layer.retain(keep);

                // add draws of the tiles without any: new tiles, and those
                // whose text changed
                let mut of_tiles = layer.tiles_of(source);
                for tile in &self.render_tiles {
                    let tile_id = *tile.overscaled_tile_id();
                    let Some(debug_bucket) = &tile.debug_bucket else {
                        continue;
                    };
                    if tile.needs_rendering()
                        && !of_tiles.contains(&tile_id)
                        && let Some(objects) =
                            polyline_objects(parameters.context, &polyline_shader, &debug_bucket.strokes, &text_options)
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
            layer.retain(keep);
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
            let border_options = LineOptions { closed: true, ..Default::default() };
            for tile in &self.render_tiles {
                let tile_id = *tile.overscaled_tile_id();
                if tile.debug_bucket.is_none() || !tile.needs_rendering() {
                    continue;
                }
                if of_tiles.contains(&tile_id) {
                    continue;
                }
                let border = if globe {
                    let z = tile_id.canonical.z;
                    globe_borders.entry(z).or_insert_with(|| {
                        let steps = SubdivisionGranularitySetting::globe().tile.granularity_for_zoom_level(z);
                        subdivide_vertex_line(&tile_border, steps, false)
                    })
                } else {
                    &tile_border
                };
                if let Some(objects) = polyline_objects(
                    parameters.context,
                    &polyline_shader,
                    std::slice::from_ref(border),
                    &border_options,
                ) {
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

    fn max_zoom(&self) -> Option<u8> {
        Some(self.origin.max_zoom())
    }
}
