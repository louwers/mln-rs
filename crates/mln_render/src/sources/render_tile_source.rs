// The render source of a source with tiles, and the debug overlays of its
// tiles: the tile borders and the parse status and timestamp text, drawn as
// segments by the tile debug shader.

use std::rc::Rc;
use std::sync::Arc;

use geometry::Point;
use mln_gfx::gpu::Gpu;
use mln_gfx::{AttributeDataType, ColorMode, DrawMode};
use mln_map_types::MapDebugOptions;
use mln_style::Source;
use mln_tile_id::OverscaledTileID;
use mln_util::Color;
use mln_util::constants::{EXTENT, TILE_SIZE};
use mln_util::hash::{HashMap, HashSet};
use mln_vec_map::VecMap;

use crate::draw_objects::Mesh;
use crate::encode::{Depth, Draw, Group};
use crate::erased_layer::ErasedEvaluatedLayer;
use crate::erased_source::ErasedOrigin;
use crate::gfx::context::Resources;
use crate::globe::{subdivide_ring, surface_granularity};
use crate::handles::{Buffer, IndexBuffer, Vertex};
use crate::mesh_desc::MeshDesc;
use crate::paint_parameters::PaintParameters;
use crate::render_source::{RenderSource, RenderSourceBase, RenderTileParameters};
use crate::render_tile::RenderTile;
use crate::shaders::programs;
use crate::shaders::ubo::TileDebugPropsUBO;
use crate::tile::job::SourceKey;
use crate::tile::source_tiles::{CoverUpdate, SourceTiles};
use crate::tile::store::TileStore;
use crate::tile_matrix::TileProjection;
use crate::tile_parameters::TileParameters;

/// The color and width of a debug overlay's segments: red tile borders, and
/// black text over a white outline.
#[must_use]
fn debug_style(type_: DebugType) -> (Color, f32) {
    match type_ {
        DebugType::TextOutline => (Color::white(), 4.0),
        DebugType::Text => (Color::black(), 2.0),
        DebugType::Border => (Color::red(), 4.0),
    }
}

/// A segment of a debug overlay, from its first point to its second, in
/// tile units.
type Segment = [Point<i16>; 2];

/// The corners of the quad that each segment draws as an instance of: x is
/// 0 at the segment's start and 1 at its end, y picks its side.
const SEGMENT_QUAD: [[i16; 2]; 4] = [[0, 0], [0, 1], [1, 0], [1, 1]];

/// A segment's two ends, which an instance of [`SEGMENT_QUAD`] reads.
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
struct SegmentEnds([i16; 2], [i16; 2]);

impl Vertex for SegmentEnds {
    const ATTRIBUTES: &'static [AttributeDataType] = &[AttributeDataType::Short2, AttributeDataType::Short2];
}

/// The mesh that draws `segments` as instances of `quad`, the vertices and
/// indexes of [`SEGMENT_QUAD`]; `None` where there are none.
fn segment_mesh(
    context: &mut Resources,
    quad: &(Buffer<[i16; 2]>, IndexBuffer),
    segments: impl IntoIterator<Item = Segment>,
) -> Option<Mesh> {
    let ends: Vec<SegmentEnds> = segments.into_iter().map(|[a, b]| SegmentEnds([a.x, a.y], [b.x, b.y])).collect();
    if ends.is_empty() {
        return None;
    }
    let ends = context.buffer(&ends);
    let (corners, strip) = quad;
    let mut mesh = MeshDesc::new(&programs::TILE_DEBUG, strip);
    mesh.vertices(corners).instances(&ends).primitive(DrawMode::TriangleStrip).blend(ColorMode::alpha_blended());
    Some(context.mesh(&mesh))
}

/// The segments of `lines`, each a polyline.
fn polyline_segments(lines: &[Vec<Point<i16>>]) -> impl Iterator<Item = Segment> {
    lines.iter().flat_map(|line| line.windows(2).map(|pair| [pair[0], pair[1]]))
}

// Debug overlays -------------------------------------------------------------

/// A draw of a debug overlay: segments over a tile of a source.
struct DebugDraw {
    /// The id of the source.
    source: Rc<str>,
    tile: OverscaledTileID,
    /// The lines of text the segments spell, of a text overlay.
    text: Option<Vec<String>>,
    /// Whether the segments are of the globe, where tile borders bend.
    globe: bool,
    mesh: Mesh,
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
    /// The color and width of the overlay's segments.
    style: (Color, f32),
    /// The vertices and indexes of [`SEGMENT_QUAD`] that the overlay's
    /// draws share, once a draw needs them.
    quad: Option<(Buffer<[i16; 2]>, IndexBuffer)>,
    /// The overlay's draws, in the order they were made, which is the order
    /// they draw in.
    draws: Vec<DebugDraw>,
    /// The frame's draws of the overlay, with the buffers they read.
    frame: Group,
}

impl DebugLayer {
    #[must_use]
    pub(crate) fn new(type_: DebugType) -> Self {
        DebugLayer { style: debug_style(type_), quad: None, draws: Vec::new(), frame: Group::new() }
    }

    /// Uploads the meshes of the frame's draws.
    pub(crate) fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Resources) {
        self.frame.upload(gpu, context);
    }

    /// Draws the frame's draws of the pass into the open pass of `gpu`, the
    /// context's.
    pub(crate) fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        self.frame.render(gpu, parameters);
    }

    /// Keeps the draws for which `keep` returns true.
    fn retain(&mut self, keep: impl FnMut(&DebugDraw) -> bool) {
        self.draws.retain(keep);
    }

    /// The tiles with draws of the source `source`.
    fn tiles_of(&self, source: &str) -> HashSet<OverscaledTileID> {
        self.draws.iter().filter(|draw| *draw.source == *source).map(|draw| draw.tile).collect()
    }

    /// Adds a draw of `segments`, of the globe where `globe`, over
    /// `tile_id` of the source `source`, spelling `text`; `false` where
    /// there are no segments.
    fn add(
        &mut self,
        context: &mut Resources,
        globe: bool,
        source: &Rc<str>,
        tile_id: OverscaledTileID,
        text: Option<Vec<String>>,
        segments: impl IntoIterator<Item = Segment>,
    ) -> bool {
        let quad =
            self.quad.get_or_insert_with(|| (context.buffer(&SEGMENT_QUAD), context.index_buffer(&[0, 1, 2, 3])));
        let Some(mesh) = segment_mesh(context, quad, segments) else {
            return false;
        };
        self.draws.push(DebugDraw { source: Rc::clone(source), tile: tile_id, text, globe, mesh });
        true
    }

    /// Makes the frame's draws of the overlay, and writes the buffers they
    /// read.
    pub(crate) fn update_draws(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        let frame = parameters.frame;
        for draw in &self.draws {
            let tile = draw.tile.to_unwrapped();
            let at = TileProjection::of(&tile, frame.camera, &frame.projection_data_for_tile(&tile, false), None);
            self.frame.push_at(&draw.mesh, &at, Draw { depth: Depth::Off, ..Draw::DEFAULT }, ());
        }

        let (color, width) = self.style;
        let world_width = (TILE_SIZE * frame.camera.zoom().exp2()) as f32;
        self.frame.block(&TileDebugPropsUBO { color: color.to_float_array(), width, world_width, pad: [0.0; 2] });
        self.frame.finish(gpu, parameters.context, &(), []);
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
    /// The deepest zoom of the last cover, `None` before the first.
    max_zoom: Option<u8>,
    render_tiles: Vec<RenderTile>,
    /// The render tiles by tile id, without those held for fade, as
    /// indexes into them.
    order_by_id: Vec<usize>,
}

impl RenderTileSource {
    /// The render source of `style_source`, whose tiles go in `store` and
    /// get their data through `origin`.
    pub(crate) fn new(style_source: Source, origin: Box<dyn ErasedOrigin>, store: &mut TileStore) -> Self {
        let key = store.add_source(style_source.id().to_owned(), origin);
        RenderTileSource {
            id: Rc::from(style_source.id()),
            base: RenderSourceBase::new(style_source),
            key,
            max_zoom: None,
            render_tiles: Vec::new(),
            order_by_id: Vec::new(),
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
        self.base.style_source = style_source;
        self.base.enabled = needs_rendering;
        let Some((source, ctx)) = store.source_mut(self.key) else {
            return;
        };
        let Some(cover) = source.update_origin(&self.base.style_source, ctx, parameters, needs_relayout) else {
            return;
        };
        self.max_zoom = Some(cover.zoom_range.max);
        let update = CoverUpdate {
            layers,
            needs_rendering,
            needs_relayout,
            parameters,
            style_source: &self.base.style_source,
            cover,
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
    }

    fn release_render_data(&mut self) {
        self.render_tiles = Vec::new();
        self.order_by_id = Vec::new();
    }

    fn upload(&mut self, store: &mut TileStore, context: &mut Resources) {
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
        let globe = parameters.frame.camera.is_globe();

        // The tiles to cover, whose draws stay where they were built for the
        // frame's projection and spell the tile's text.
        let new_tiles: HashMap<OverscaledTileID, &RenderTile> =
            self.render_tiles.iter().map(|tile| (*tile.overscaled_tile_id(), tile)).collect();
        let keep = |draw: &DebugDraw| {
            if draw.source != *source {
                return true;
            }
            let Some(tile) = new_tiles.get(&draw.tile) else {
                return false;
            };
            let same_shader = draw.globe == globe;
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
                        && layer.add(
                            parameters.context,
                            globe,
                            source,
                            tile_id,
                            Some(debug_bucket.text.clone()),
                            polyline_segments(&debug_bucket.strokes),
                        )
                    {
                        of_tiles.insert(tile_id);
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
            // split as finely as the tile's surface
            let extent = EXTENT as i16;
            let corners = [Point::new(0, 0), Point::new(extent, 0), Point::new(extent, extent), Point::new(0, extent)];
            for tile in &self.render_tiles {
                let tile_id = *tile.overscaled_tile_id();
                if tile.debug_bucket.is_none() || !tile.needs_rendering() || of_tiles.contains(&tile_id) {
                    continue;
                }
                let granularity = if globe { surface_granularity(tile_id.canonical.z) } else { 1 };
                let border = subdivide_ring(&corners, granularity);
                let segments = polyline_segments(std::slice::from_ref(&border));
                if layer.add(parameters.context, globe, source, tile_id, None, segments) {
                    of_tiles.insert(tile_id);
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

    fn tile_order(&self) -> Option<&[usize]> {
        Some(&self.order_by_id)
    }

    fn dump_debug_logs(&self) {}

    fn max_zoom(&self) -> Option<u8> {
        self.max_zoom
    }
}
