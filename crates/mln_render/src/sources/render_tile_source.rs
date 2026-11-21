// The render source of a source with tiles, and the debug overlays of its
// tiles: the tile borders and the parse status and timestamp text, drawn as
// segments by the tile debug shader.

use std::rc::Rc;
use std::sync::Arc;

use geometry::Point;
use glam::Vec2;
use mln_gfx::gpu::{Gpu, ShaderKey};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::programs;
use mln_gfx::shaders::programs::{
    ID_TILE_DEBUG_CORNER_VERTEX_ATTRIBUTE, ID_TILE_DEBUG_END_ATTRIBUTE, ID_TILE_DEBUG_PROPS_UBO,
    ID_TILE_DEBUG_START_ATTRIBUTE,
};
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::ID_PROJECTION_UBO;
use mln_gfx::shaders::tile_debug_ubo::TileDebugPropsUBO;
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode};
use mln_map_types::MapDebugOptions;
use mln_style::Source;
use mln_tile_id::OverscaledTileID;
use mln_util::Color;
use mln_util::constants::{EXTENT, TILE_SIZE};
use mln_util::hash::{HashMap, HashSet};
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_vec_map::VecMap;

use crate::draw_objects::{DrawGeometry, DrawObjects, Textures};
use crate::erased_layer::ErasedEvaluatedLayer;
use crate::erased_source::ErasedOrigin;
use crate::gfx::context::Context;
use crate::gfx::indexes::DrawIndexes;
use crate::gfx::vertex_attribute::{SharedAttribute, Uniforms, VertexAttributeArray};
use crate::layer_drawing::{DrawDepth, DrawList, DrawObjectsMut, FrameDraw, KeptDraw, LayerDraws};
use crate::paint_parameters::PaintParameters;
use crate::render_source::{RenderSource, RenderSourceBase, RenderTileParameters, TileOrder};
use crate::render_tile::RenderTile;
use crate::tile::job::SourceKey;
use crate::tile::source_tiles::{CoverUpdate, SourceTiles};
use crate::tile::store::TileStore;
use crate::tile::tile::TileKind;
use crate::tile_matrix::to_projection_ubo;
use crate::tile_parameters::TileParameters;

/// The color and width of a debug overlay's segments: red tile borders, and
/// black text over a white outline. `update_draws` sets the world's width.
#[must_use]
fn debug_properties(type_: DebugType) -> TileDebugPropsUBO {
    let (color, width) = match type_ {
        DebugType::TextOutline => (Color::white(), 4.0),
        DebugType::Text => (Color::black(), 2.0),
        DebugType::Border => (Color::red(), 4.0),
    };
    TileDebugPropsUBO { color: color.to_float_array(), width, world_width: 0.0, pad: [0.0; 2] }
}

/// A segment of a debug overlay, from its first point to its second, in
/// tile units.
type Segment = [Point<i16>; 2];

/// The corners of the quad that each segment draws as an instance of: x is
/// 0 at the segment's start and 1 at its end, y picks its side.
const SEGMENT_QUAD: [[i16; 2]; 4] = [[0, 0], [0, 1], [1, 0], [1, 1]];

/// The GPU objects that draw `segments` with `shader`; `None` where there
/// are none.
fn segment_objects(
    context: &mut Context,
    shader: &ShaderKey,
    segments: impl IntoIterator<Item = Segment>,
) -> Option<DrawObjects> {
    let ends: Vec<[i16; 4]> = segments.into_iter().map(|[a, b]| [a.x, a.y, b.x, b.y]).collect();
    if ends.is_empty() {
        return None;
    }
    let instance_count = ends.len();
    let corners = context.add_vertices(SEGMENT_QUAD.to_vec());
    let ends = context.add_vertices(ends);
    let point = size_of::<[i16; 2]>() as u32;
    let mut vertices = VertexAttributeArray::new();
    vertices.set_shared(
        ID_TILE_DEBUG_CORNER_VERTEX_ATTRIBUTE,
        SharedAttribute::new(corners, AttributeDataType::Short2, 0, point),
    );
    let mut instances = VertexAttributeArray::new();
    for (id, offset) in [(ID_TILE_DEBUG_START_ATTRIBUTE, 0), (ID_TILE_DEBUG_END_ATTRIBUTE, point)] {
        instances.set_shared(id, SharedAttribute::new(ends.clone(), AttributeDataType::Short2, offset, 2 * point));
    }

    let geometry = DrawGeometry {
        vertices,
        vertex_count: SEGMENT_QUAD.len(),
        mode: DrawMode::TriangleStrip(),
        indexes: DrawIndexes::new(vec![0, 1, 2, 3]),
        segments: vec![SegmentBase { instance_count, ..SegmentBase::with_lengths(0, 0, 4, 4) }],
        instances: Some(instances),
    };
    Some(DrawObjects::new(
        shader.clone(),
        ColorMode::alpha_blended(),
        CullFaceMode::disabled(),
        geometry,
        Textures::default(),
    ))
}

/// The segments of `lines`, each a polyline.
fn polyline_segments(lines: &[Vec<Point<i16>>]) -> impl Iterator<Item = Segment> {
    lines.iter().flat_map(|line| line.windows(2).map(|pair| [pair[0], pair[1]]))
}

/// The segments of a tile's border, each edge split into `steps` segments
/// so that it bends with the globe.
fn border_segments(steps: u32) -> impl Iterator<Item = Segment> {
    let extent = EXTENT;
    let edges = [
        ((0, 0), (extent, 0)),
        ((extent, 0), (extent, extent)),
        ((extent, extent), (0, extent)),
        ((0, extent), (0, 0)),
    ];
    let steps = steps.max(1) as i32;
    edges.into_iter().flat_map(move |((x0, y0), (x1, y1))| {
        let at = move |i: i32| Point::new((x0 + (x1 - x0) * i / steps) as i16, (y0 + (y1 - y0) * i / steps) as i16);
        (0..steps).map(move |i| [at(i), at(i + 1)])
    })
}

// Debug overlays -------------------------------------------------------------

/// A draw of a debug overlay: segments over a tile of a source.
struct DebugDraw {
    /// The id of the source.
    source: Rc<str>,
    tile: OverscaledTileID,
    /// The lines of text the segments spell, of a text overlay.
    text: Option<Vec<String>>,
    draw: KeptDraw,
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

/// A debug overlay: segments over the tiles of the sources, in the color and
/// width of its type.
pub(crate) struct DebugLayer {
    properties: TileDebugPropsUBO,
    /// The overlay's draws, in the order they were made, which is the order
    /// they draw in.
    draws: Vec<DebugDraw>,
    /// The frame's draws as `update_draws` makes them.
    draw_list: DrawList<ProjectionUBO, usize>,
    /// The frame's draws of the overlay, each of the overlay's draw at its
    /// index, with the buffers they read.
    frame: LayerDraws<usize>,
}

impl DebugLayer {
    #[must_use]
    pub(crate) fn new(type_: DebugType) -> Self {
        DebugLayer {
            properties: debug_properties(type_),
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
        for (index, draw) in self.draws.iter().enumerate() {
            let projection = parameters.frame.projection_data_for_tile(&draw.tile.to_unwrapped(), false);
            let frame_draw = FrameDraw { depth: DrawDepth::Off, ..draw.draw.frame_draw(None) };
            self.draw_list.push_part(frame_draw, index, to_projection_ubo(&projection));
        }

        self.properties.world_width = (TILE_SIZE * parameters.frame.view.zoom().exp2()) as f32;
        let properties = &self.properties;
        let replaced = self.draw_list.finish(&mut self.frame, gpu, parameters.context, |writer| {
            writer.uniform(ID_TILE_DEBUG_PROPS_UBO, properties);
            writer.storage(ID_PROJECTION_UBO, |projection| *projection);
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
    fast_pfor_enabled: bool,
    /// The deepest zoom of the last cover, `None` before the first.
    max_zoom: Option<u8>,
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
        origin: Box<dyn ErasedOrigin>,
        store: &mut TileStore,
    ) -> Self {
        let key = store.add_source(style_source.id().to_owned(), kind, single_layer, origin);
        RenderTileSource {
            id: Rc::from(style_source.id()),
            base: RenderSourceBase::new(style_source),
            key,
            fast_pfor_enabled: false,
            max_zoom: None,
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
        let cover = source.update_origin(
            &previous,
            &self.base.style_source,
            ctx,
            parameters,
            needs_relayout,
            self.fast_pfor_enabled,
        );
        let Some(cover) = cover else {
            return;
        };
        self.max_zoom = Some(cover.zoom_range.max);
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

    fn upload(&mut self, store: &mut TileStore, context: &mut Context) {
        store.upload(self.key, &self.render_tiles, context);
    }

    /// Updates the source's draws of the debug overlays.
    fn update_debug_draws(&self, debug_layers: &mut DebugLayers, parameters: &mut PaintParameters<'_>) {
        if !parameters
            .debug_options
            .contains(MapDebugOptions::TIMESTAMPS | MapDebugOptions::PARSE_STATUS | MapDebugOptions::TILE_BORDERS)
        {
            debug_layers.clear();
            return;
        }

        let source = &self.id;
        let globe = parameters.frame.view.is_globe();
        let projection_variant = if globe { ProjectionVariant::Globe } else { ProjectionVariant::Mercator };

        let shader = parameters.context.shader_key(&programs::TILE_DEBUG, Uniforms::default(), projection_variant);

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
            let same_shader = *draw.draw.objects.shader() == shader;
            let same_text = match (&draw.text, &tile.debug_bucket) {
                (Some(text), Some(debug_bucket)) => *text == debug_bucket.text,
                _ => true,
            };
            same_shader && same_text
        };

        // Timestamps or Parse Status
        if parameters.debug_options.contains(MapDebugOptions::TIMESTAMPS | MapDebugOptions::PARSE_STATUS) {
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
                            segment_objects(parameters.context, &shader, polyline_segments(&debug_bucket.strokes))
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

            // add new draws; on the globe the edges bend with the sphere,
            // split as finely as the tile meshes of the zoom
            for tile in &self.render_tiles {
                let tile_id = *tile.overscaled_tile_id();
                if tile.debug_bucket.is_none() || !tile.needs_rendering() || of_tiles.contains(&tile_id) {
                    continue;
                }
                let steps = if globe {
                    SubdivisionGranularitySetting::globe().tile.granularity_for_zoom_level(tile_id.canonical.z)
                } else {
                    1
                };
                if let Some(objects) = segment_objects(parameters.context, &shader, border_segments(steps)) {
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

    fn set_fast_pfor_enabled(&mut self, enable: bool) {
        self.fast_pfor_enabled = enable;
    }

    fn dump_debug_logs(&self) {}

    fn max_zoom(&self) -> Option<u8> {
        self.max_zoom
    }
}
