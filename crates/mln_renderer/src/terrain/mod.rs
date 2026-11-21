//! 3D terrain: the style's `terrain` drawn as a surface whose heights come
//! from a raster-dem source, with the layers that drape over it drawn into
//! textures that the surface samples.
//!
//! Terrain draws when the style's terrain names a raster-dem source whose
//! tileset has loaded, on a Mercator view, in continuous and static modes.
//! In tile mode, and on the globe, the map draws flat. A frame without
//! terrain takes none of the paths below: its tile covers, matrices and
//! draws are a flat map's.
//!
//! # A frame with terrain
//!
//! The map owns the camera and runs on its own thread; the renderer, which
//! may run on another, owns [`RenderTerrain`] through its orchestrator, and
//! with it the cover, the drapes and the layers' drawings in them, the
//! surface's draws, the DEM textures, and the last ground it told the map
//! of, which the standing layers stand on. In each update:
//!
//! 1. Before the sources update, [`RenderTerrain::update`] covers the view
//!    with terrain tiles and lists the DEM tiles they read. Every source
//!    then covers the view the same way, culled by the same elevation
//!    ranges. The terrain's source updates last, once the standing layers'
//!    tiles are known, whose DEM tiles off the surface's it lists too
//!    ([`RenderTerrain::request_standing`]), and keeps its DEM tiles loaded,
//!    whether or not a layer draws them. They count toward the frame's
//!    being loaded.
//! 2. Once the sources have updated, [`RenderTerrain::bind`] binds each
//!    terrain tile's DEM, and tells the map of the ground the tiles draw, if
//!    it changed.
//! 3. In the encode, the ground's DEM textures are made first
//!    ([`RenderTerrain::prepare_ground`]): those the surface reads, and in a
//!    frame where a layer stands on the ground, those its tiles read off
//!    the surface's too; in a frame where a layer that the ground hides
//!    draws, so is the target of the ground's depth. The layers that drape (background, fill, line, hillshade,
//!    color relief, and raster, an image source's too) make no draws in the
//!    frame: they draw in each drape of their stack whose key changed
//!    ([`RenderTerrain::begin_drapes`]). The surface makes a draw per tile
//!    and stack that has a drape there ([`RenderTerrain::update_draws`]).
//! 4. The ground's depth ([`RenderTerrain::render_ground_depth`]) and the
//!    drapes draw into their textures before the frame's pass, and the
//!    heatmaps' kernels between them. The
//!    frame clears to transparent, and its translucent pass draws each
//!    stack's surface where the stack's first layer draws, and the layers
//!    that don't drape between the stacks: those that stand on the ground
//!    as it draws (see **Standing layers**), which are circles, heatmap
//!    kernels, fill extrusions and symbols, and custom layers, flat. They
//!    draw with their layer depth off, but fill extrusions, which test and
//!    write the 3D depth that the surface writes.
//!
//! # Contracts
//!
//! - **The cover** ([`TerrainCover`]) is GL JS's Mercator `coveringTiles`
//!   with a zoom for each tile: terrain tiles twice the DEM's tile size, of
//!   zooms 0 to 22, nearest the center first. Each is culled by a box over
//!   the elevation range of the DEM under it, with the exaggeration, or by
//!   GL JS's flat range where none has loaded. While no tile holds the
//!   view's center, the cover keeps the tile under it, undrawn, so that the
//!   camera has a DEM to stand on.
//! - **The elevation ranges** ([`ElevationRanges`]) are those of every DEM
//!   tile of the terrain's tileset that has loaded while terrain drew,
//!   remembered after the source lets a tile go. A range that culls the
//!   tiles reading its DEM tile keeps culling them, so a camera at rest
//!   settles on one cover rather than flip between it and the cover of an
//!   ancestor's wider range. The ranges are forgotten when the tileset
//!   changes or terrain stops, and, past a bound, those of the tiles the
//!   source no longer holds.
//! - **The DEM bindings.** A tile at zoom z reads the DEM tile at z − 1,
//!   within the DEM's zoom levels; while that loads, the sub-rect of the
//!   deepest ancestor that has loaded; else it draws flat. A frame binds the
//!   DEM tiles it has, so a loaded frame draws each tile with its own, but
//!   where the DEM tile is missing or its request failed: that tile isn't
//!   loading, and draws the ancestor until the file source's retry brings
//!   its DEM. A DEM tile that reloads draws its old DEM until the new one is
//!   in. The surface reads each DEM's bordered RGBA8 image texel by texel
//!   and decodes it by the height rule of `mln_util::dem`, as the CPU does.
//! - **The stacks** ([`DrapeStacks`]) are GL JS's: each a run of the
//!   frame's layers that drape, ended by one of the frame's layers that
//!   doesn't, such as a symbol, circle, heatmap, fill extrusion or custom
//!   layer. A layer the frame doesn't draw, hidden or out of its zooms,
//!   splits no stack. A style whose layers don't drape has no stack, and
//!   then no drapes and no surface.
//! - **Standing layers** (circle, heatmap, fill extrusion and symbol) stand
//!   on the surface as it draws, whether or not a stack draws it: over
//!   terrain, their draws are of the [`ProjectionVariant::Terrain`] variant,
//!   which reads the height of the ground in the vertex stage, under a
//!   circle's center as it draws (translated), a kernel's, the centroid of a
//!   fill extrusion's polygon as it draws (GL JS's: the floored mean of its
//!   rings' vertices), and a symbol's anchor as it draws, by the height rule
//!   the surface reads its DEM by. A symbol stands its `symbol-height-offset`
//!   above that ground, or above sea level where its `symbol-height-anchor`
//!   is `absolute`, which its draw reads as flat ground; each vertex of a
//!   map-pitched label along a line stands on the ground under it. Placement
//!   reads the same ground on the CPU ([`TileGround::height`]): it collides a
//!   point label's box, all of it at its anchor's height, and lays out a
//!   label at a variable anchor, where the label draws. A new ground
//!   ([`RenderTerrain::ground`]) makes placement stale, even with the camera
//!   at rest, and a continuous map places again when placement is next due,
//!   so a label can stand on the new ground for up to one placement period
//!   before its box follows, as it can behind a moving camera; a still
//!   image places in every update, on the ground of that update. A label
//!   along a line stands on the ground with no height offset: each update
//!   lays its glyphs out along its line as it stands on the ground under each
//!   vertex, and placement collides each of its circles at the ground under
//!   it, both reading its tile's ground, clamped to its DEM tile past its
//!   edges. Collision debug boxes and circles draw at sea
//!   level. A polygon's roof and base rise by the ground at its centroid. Where its base is at or
//!   below 0, each end of a wall's foot sinks to 10 m below the lower of the
//!   ground at the centroid and the ground under that end, so that no gap
//!   shows under a wall on any slope. Its vertical gradient shades by its own
//!   base and height, carried on below the base to a foot that sinks, so a
//!   wall shades where it shows as it would on level ground. Over terrain
//!   the extrusions project as the surface does, and test and write the
//!   depth it writes, so the ground hides them, and they hide it, pixel by
//!   pixel; on a flat map they keep a near plane further out, for the depth
//!   buffer's precision.
//!   Their buckets keep a centroid for each polygon, which only a terrain
//!   variant's draws expand to their vertices, and are laid out with them
//!   only for a style with terrain: a tile laid out without them lays out
//!   again while the style has terrain, holding a still as any tile that lays
//!   out does, and draws nothing over terrain until then. Each tile T of a
//!   standing layer reads one DEM tile ([`TerrainElevation::ground`]), as in
//!   GL JS, MapLibre Native and orbis: where a drawn terrain tile holds T,
//!   that tile's, at T's place in it, as the surface draws it, its own or,
//!   while that loads, the ancestor the surface reads in its place. Where
//!   drawn terrain tiles are inside T, as over a source tiled coarser than
//!   the DEM, or none is over it, T reads the DEM tile that holds it at its
//!   own zoom, or at the DEM's deepest if that is shallower, which the cover
//!   requests for each such tile of a standing layer the frame draws, and a
//!   still waits for; while it loads, T reads the deepest ancestor of it
//!   that has loaded. Where the DEM has no tile that holds T, its lowest
//!   zoom deeper than T's, T stands at sea level. A known limit: features of
//!   such a T stand on a DEM tile coarser than the surface's under them, off
//!   the surface by what the DEM tile differs from the finer ones. A point
//!   layer's T (circles, heatmap kernels and symbols) draws only the
//!   features whose anchor is in T, from 0 to under the extent on both axes:
//!   a copy in T's buffer, which a still's circles keep, is left to the tile
//!   that holds it, and is absent where that tile doesn't draw, as on a
//!   continuous flat map ([`TileFeatures::Anchored`]). A fill extrusion's T
//!   draws every piece of a polygon that its bucket cut at T's edges, its
//!   centroid in T or not ([`TileFeatures::All`]). The CPU reads a feature's
//!   height ([`TileGround::height`]) from the DEM tile and coordinates its
//!   draw reads, by the same height rule. A draw's ground (DEM coordinates,
//!   the anchors it draws, decode and exaggeration) rides in its projection
//!   entry, which every standing program's vertex stage reads
//!   ([`TerrainProjectionUBO`]); its DEM texture binds at
//!   [`ID_GROUND_DEM_TEXTURE`], a slot of the draw's own textures that no
//!   standing program's other textures take, so a label's glyphs and icons keep
//!   0 and 1. A frame without terrain makes no terrain variant, writes no
//!   ground, binds no ground texture and expands no centroid.
//! - **What the ground hides.** Fill extrusions hide behind the ground pixel
//!   by pixel, by the depth they test. A symbol or a circle hides whole, by
//!   its anchor or center as it draws, raised: in a frame where terrain
//!   draws and a layer of theirs does, a pass before the
//!   frame's draws every terrain tile, skirts included, into a texture at the
//!   frame's logical pixels, with a depth test that keeps each pixel's
//!   nearest ground, as its eye distance's f32 bits ([`ID_GROUND_DEPTH_TEXTURE`]).
//!   The terrain variant reads it where its anchor is seen, the reciprocals of
//!   the four texels around it interpolated as a plane's are, so that ground
//!   seen at a grazing angle doesn't hide what stands on it, and fades the
//!   feature out over GL JS's ramp in its NDC depth, scaled by the frame's
//!   near and far planes ([`TerrainPropsUBO::depth_factor`]), averaged with
//!   the point 0.01 NDC above where it is hidden more than a little; where no
//!   ground is drawn, nothing is hidden. The depth is drawn every frame that
//!   needs it, from that frame's ground, so it holds as DEM tiles arrive, the
//!   cover changes and the camera moves; its texture stays while the frame's
//!   size does, and binds as the frame's global buffers do, from the context
//!   ([`Context::ground_depth`]), so no flat program's layout or draw
//!   changes for it. A hidden label stays in the collision grid. A frame
//!   without terrain, or without a layer that the ground hides, makes no
//!   depth and draws no pass for it. A known limit: an axonometric view, whose
//!   clip w doesn't vary with depth, hides nothing. The ground hides no
//!   heatmap kernel: a kernel adds its density wherever it draws, with ground
//!   in front of it or not, as in GL JS, so that density doesn't pop in and
//!   out as the camera moves over hills. The draws of circles and heatmap
//!   kernels stack in the frame's order of their tiles, whenever each draw
//!   was made, so a view draws alike whatever path the camera took to it;
//!   what shows of fill extrusions is the depth's to decide.
//! - **The surface** draws once for each stack, its tiles nearest the
//!   center first, testing and writing the 3D depth range with less or
//!   equal and blending its drapes over what is under it. A stack above the
//!   first draws where the first left the nearest ground, and only the
//!   tiles where it has a drape; the first draws every tile, transparent
//!   where it has none, for that depth. Where the first
//!   stack's drape is translucent, a farther slope drawn before a nearer
//!   one in the same draw shows through it, and skirts blend over the
//!   neighbouring tiles, as in GL JS.
//! - **The fog** is GL JS's terrain fog, over the surface only: past the
//!   sky's `fog-ground-blend`, the surface turns toward `fog-color`, and
//!   past `horizon-fog-blend` toward `horizon-color`, mixed through a gamma
//!   of 2.2, by a depth that runs from the camera's distance to sea level
//!   to the far plane, read for each pixel from its distance along the view.
//!   It fades in from 60° to 70° of pitch, and there is none without a sky,
//!   in an axonometric view, with a ground blend of 1, or where the far
//!   plane comes before the camera's distance to sea level. A horizon blend
//!   of 1 fogs with `fog-color` alone. The fog tints what each stack draws
//!   and never changes how much of a pixel it covers: a color's alpha is
//!   the strength of its tint, so a transparent fog leaves the ground as it
//!   is, and ground that the stacks leave transparent stays transparent.
//!   Each stack fogs what it draws, so every stack's share of a pixel is
//!   fogged once, however the stacks overlap; over opaque ground in one
//!   stack this is GL JS's fog. A known limit: each stack is fogged through
//!   the gamma before it blends over the one below, so a translucent draped
//!   layer at fog distance comes out a slightly different color by whether
//!   standing layers put it in a stack of its own or in its ground's. The
//!   layers that don't drape aren't fogged, as in GL JS. The fog is in no
//!   drape's key: the frame's evaluated sky and the view reach only the
//!   surface's draws, which every frame makes again, so a change of the sky
//!   draws no drape again.
//! - **The drapes** ([`Drapes`]): a texture for each drawn tile and stack
//!   whose layers draw something in the tile, which the stack's draped
//!   layers draw into with the map's camera, through their own draw
//!   updates. Where their draw updates make no draw, as a stack of lines
//!   does in a tile they don't cross, or one whose tiles haven't loaded,
//!   the drape has no texture and draws no pass until its key changes.
//!   Each stack's drape of a tile keeps its own key, so a change in one
//!   stack leaves the others' drapes as they were. A drape draws again
//!   only when its key differs from the one it was drawn from: its stack's
//!   layers' styles,
//!   paint revisions and tiles, the overlapping tiles' buckets and
//!   generations (an image source's bucket and quad, in each copy of the
//!   image that overlaps the drape's tile), and, with the camera at rest,
//!   the layers' evaluated values, the zoom, and the bearing where a layer
//!   draws by it. A change of the style's images, or of the feature
//!   state of a bucket a drape shows, draws it again too. While the camera
//!   of a continuous map moves, a drape drawn at another zoom or bearing
//!   stays; a still draws it again. At rest, a
//!   continuous map draws the drapes stale only by the camera one tile a
//!   frame, as GL JS does once the zoom settles: the tile drawn longest
//!   ago, the nearest the center of those, so that a camera that jumps
//!   every frame, at rest in each, draws its tiles in turn. A frame that
//!   leaves such drapes for later asks for the next and reports itself
//!   partial, so the map isn't idle until they have drawn. A drape that
//!   fails to draw fails the frame as any GPU failure does, and after a
//!   frame that failed every drape draws again. A drape whose content
//!   changed draws at once, with the camera-stale drapes of its tile, and a
//!   still draws them all.
//! - **The drapes' sides.** A tile's drapes take the power of two that
//!   covers its longest side on screen, level with the view's center, at
//!   two texels a logical pixel or one a physical pixel, whichever is
//!   more: GL JS's side (twice the terrain tile's), or the device's largest
//!   texture's if that is smaller, where the tile shows at half its size or
//!   larger, down to a sixteenth of it. Where the drapes with a texture,
//!   with the depth and stencil of each side and the layer composite of a
//!   side whose drapes composite, would hold more than a byte cap (256 MiB
//!   on phones and in the browser, 384 MiB elsewhere), the largest side
//!   allowed falls a step at a time for the tiles that show smallest, below
//!   a sixteenth if the cap needs it; no tile loses its drape. The frame's
//!   sides are settled once, after its layers have updated the drapes that
//!   draw again from their key, when it is known which drapes hold a
//!   texture: a still's are exact, a stack that draws nothing in a tile
//!   takes none of the cap there, and no frame's drapes hold more than the
//!   cap. A drape's drawings are always those of the key it draws in the
//!   frame or of the key it was drawn from, whatever its side, so a drape
//!   whose side changes and that the frame doesn't update draws again at
//!   the new side from the drawings it has, and shows what it showed: at
//!   once where it shrinks, and where it grows, at rest, in its tile's
//!   turn, as a drape stale by the camera does.
//! - **The drape composite.** A draped layer with a layer opacity below 1
//!   or a layer blend other than `normal` draws as a whole inside each
//!   drape, as in 2D (`LayerComposite`): into a texture of the drape's
//!   size, composited onto the drape. The drapes of a side share one such
//!   composite, which those that draw again in a frame use in turn, and
//!   which drops its textures in a frame where none of them composites,
//!   and a depth and stencil, kept while a drape has that side and its
//!   tile has it too: a drape that waits to grow keeps none. A drape
//!   with a layer that blends in the shader draws into the composite's
//!   frame texture, which is copied into the drape before its mips. A
//!   blend's backdrop is the drape's own texels.
//! - **The elevation snapshot** ([`TerrainElevation`]) holds the drawn
//!   tiles and the DEM images and sub-rects they read, and the DEM tiles
//!   that the standing layers' tiles read off theirs. Its images don't
//!   change once it shares them, so the map
//!   reads it on its thread. It answers
//!   four questions ([`Elevation`]): the height at a location, `None` while
//!   its DEM loads, which a clamped center stands on; the floor of the tile
//!   there, which the view's planes fit; the surface as drawn, 0 on a tile
//!   drawn flat while its DEM loads and `None` where nothing drew, which
//!   rays, the eye's clearance and screen points over terrain meet; and the
//!   range of the drawn heights, which bounds a ray. It finds the tile at a
//!   point through an index of the drawn tiles by zoom, finest first, which
//!   a ray samples hundreds of times: for the surface, of the tiles drawn
//!   in the point's world copy, and for the height and floor, which the
//!   center reads, of any copy. The renderer
//!   makes a new one only when the tiles, the DEM images they read, where
//!   they read them, whether their own DEM loads, the standing layers' DEM
//!   tiles, or the decode change: a fixed camera over loaded tiles keeps
//!   one snapshot. It reaches the map
//!   as [`RendererEvent::TerrainElevation`] before the frame of its update
//!   finishes. The renderer sends its current one again for an update
//!   whose snapshot differs from it at the view's center, such as an update
//!   of a map that let go of the ground when its style dropped the terrain.
//!   `None` reaches the map once terrain stops, and for an update that
//!   carries a snapshot while terrain doesn't draw.
//! - **Stills.** An update carries the snapshot the map's camera stands on
//!   ([`UpdateParameters::elevation`]), and the map answers each new one
//!   with an update that carries it. A frame is loaded only for an update
//!   whose camera wouldn't move on the renderer's current snapshot. A
//!   camera at rest reads only the height and floor ([`Elevation::at`] and
//!   [`Elevation::min_at`]) at the view's center, so for it the two
//!   snapshots need only agree there, or there is neither, without
//!   terrain; a snapshot that changes only away from its center holds no
//!   frame back. A camera that a gesture or a command moves
//!   ([`UpdateParameters::camera_moving`]), even a gesture held still,
//!   reads more (the surface under its eye and near plane, rays through the
//!   screen, the ground at a command's end), so its update must carry the
//!   current snapshot itself. The move onto the ground that ends a motion
//!   reads them once more, in the update the camera comes to rest in,
//!   which no later snapshot redoes. A still image draws only such an update's
//!   frame, and a continuous frame of any other reports itself partial, so
//!   it doesn't make the map idle.

mod cover;
mod dem_textures;
mod drapes;
mod draws;
mod elevation;
mod fog;
mod ground;
mod ground_depth;
mod mesh;
mod stacks;

use std::rc::Rc;
use std::sync::Arc;

pub(crate) use cover::{ElevationRanges, TerrainCover};
#[cfg(test)]
pub(crate) use cover::{REMEMBERED_RANGES, TerrainTile, terrain_tile_size};
pub(crate) use drapes::Drapes;
pub(crate) use elevation::TerrainElevation;
pub(crate) use ground::{GroundBinding, GroundBindings, TileFeatures, TileGround, write_projection};
use mln_gfx::gpu::Gpu;
#[cfg(doc)]
use mln_gfx::projection_variant::ProjectionVariant;
#[cfg(doc)]
use mln_gfx::shaders::shader_defines::{ID_GROUND_DEM_TEXTURE, ID_GROUND_DEPTH_TEXTURE};
#[cfg(doc)]
use mln_gfx::shaders::terrain_ubo::TerrainProjectionUBO;
use mln_gfx::shaders::terrain_ubo::TerrainPropsUBO;
use mln_map_types::{Elevation, MapMode, TerrainSkirtLength};
use mln_style::source_type::SourceData;
use mln_style::sources::RasterDemSourceType;
use mln_util::constants;
use mln_util::dem::DemUnpack;
use mln_util::geo::{LatLng, WrapMode};
use mln_util::logging::{Event, Log};
use mln_util::tileset::RasterEncoding;
pub(crate) use stacks::DrapeStacks;

use crate::gfx::context::Context;
use crate::layer_drawing::ChangedBuckets;
use crate::paint_parameters::PaintParameters;
use crate::render_sky::EvaluatedSky;
use crate::renderer_event::RendererEvent;
use crate::terrain::cover::dem_bucket;
use crate::terrain::dem_textures::DemTextures;
use crate::terrain::draws::TerrainDraws;
use crate::terrain::ground_depth::GroundDepth;
use crate::tile::source_tiles::SourceTiles;
use crate::update_parameters::UpdateParameters;

/// How far the skirts of terrain tiles reach down at `zoom`, in meters
/// (GL JS's `getSkirtLength`); `None` for tiles without skirts.
fn skirt_length(length: TerrainSkirtLength, zoom: f64) -> Option<f64> {
    match length {
        TerrainSkirtLength::Auto => Some(constants::M2PI * constants::EARTH_MEAN_RADIUS_M / zoom.max(0.0).exp2() / 5.0),
        TerrainSkirtLength::None => None,
    }
}

/// Whether a camera at rest whose center is at `center` stands on the
/// ground `seen` as it would on `drawn`: both give the same height and
/// floor there, which is all such a camera takes from the ground, or
/// neither is there. Ground that differs elsewhere doesn't move it.
fn stands_alike(seen: Option<&dyn Elevation>, drawn: Option<&dyn Elevation>, center: LatLng) -> bool {
    match (seen, drawn) {
        (None, None) => true,
        (Some(seen), Some(drawn)) => {
            std::ptr::addr_eq(seen, drawn)
                || (seen.at(center) == drawn.at(center) && seen.min_at(center) == drawn.min_at(center))
        }
        _ => false,
    }
}

/// How the terrain's surface draws in a frame: how its DEM decodes, its
/// exaggeration and its skirts.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Surface {
    /// The terrain's uniform block.
    pub(crate) props: TerrainPropsUBO,
    /// Whether the tiles have skirts.
    pub(crate) skirts: bool,
}

impl Surface {
    /// The surface of a DEM of `encoding`, raised by `exaggeration`, with
    /// skirts of `skirt_length` meters or none.
    fn new(encoding: RasterEncoding, exaggeration: f32, skirt_length: Option<f64>) -> Surface {
        let DemUnpack { weights: [r, g, b], base, scale } = DemUnpack::of(encoding);
        let props = TerrainPropsUBO {
            unpack: [r, g, b, base],
            unpack_scale: scale,
            exaggeration,
            skirt_length: skirt_length.unwrap_or(0.0) as f32,
            depth_factor: 0.0,
        };
        Surface { props, skirts: skirt_length.is_some() }
    }

    /// How the DEM decodes.
    fn unpack(&self) -> DemUnpack {
        let [r, g, b, base] = self.props.unpack;
        DemUnpack { weights: [r, g, b], base, scale: self.props.unpack_scale }
    }
}

/// The terrain of the frames the renderer draws: whether it draws, the
/// tiles that cover the view, their drapes and their draws.
#[derive(Default)]
pub(crate) struct RenderTerrain {
    cover: Option<TerrainCover>,
    /// How the surface of the cover draws.
    surface: Surface,
    drapes: Drapes,
    draws: TerrainDraws,
    /// The ground the last update's cover drew, as the renderer last told
    /// the map, which the standing layers stand on; `None` while terrain
    /// doesn't draw.
    elevation: Option<Arc<TerrainElevation>>,
    /// The textures of the DEM tiles the frame's draws read.
    dem_textures: DemTextures,
    /// The depth of the drawn ground, in a frame where a layer that terrain
    /// hides draws.
    ground_depth: Option<GroundDepth>,
    /// Whether it has logged that terrain doesn't draw on the globe.
    logged_globe: bool,
    /// The drapes' byte cap in place of [`drapes::BYTE_CAP`].
    #[cfg(test)]
    pub(crate) drape_byte_cap: Option<u64>,
}

impl RenderTerrain {
    /// Brings the terrain to `parameters`, before the sources update: checks
    /// whether it draws, takes in the elevation ranges of the DEM tiles that
    /// have loaded, which the last cover's ranges remember, covers the view
    /// with its tiles and lists the DEM tiles they read. `dem` finds the
    /// tiles of a source by its id. Returns the cover while terrain draws;
    /// its tiles bind their DEM in [`bind`]. While terrain doesn't draw, it
    /// remembers no ranges.
    ///
    /// [`bind`]: Self::bind
    pub(crate) fn update<'a>(
        &mut self,
        parameters: &UpdateParameters,
        dem: impl FnOnce(&str) -> Option<&'a SourceTiles>,
    ) -> Option<&TerrainCover> {
        let mut ranges = self.cover.take().map(|cover| cover.ranges).unwrap_or_default();
        let style = &parameters.style;
        let Some(terrain) = style.get_terrain() else {
            // Layers count their paint revisions only for a style with
            // terrain, so drapes kept from before can't tell what changes
            // until the terrain is back.
            self.drapes.redraw();
            return None;
        };
        if parameters.mode == MapMode::Tile {
            return None;
        }
        // Terrain draws only once its source's tileset has loaded, and not at
        // all from a source that isn't raster-dem.
        let source = style.get_source(&terrain.source)?;
        let data = SourceData::<RasterDemSourceType>::recover(source)?;
        let tileset = data.description.as_ref().filter(|tileset| !tileset.tiles.is_empty())?;
        if parameters.view.is_globe() {
            if !self.logged_globe {
                self.logged_globe = true;
                Log::warning(Event::Render, "Terrain draws flat on the globe");
            }
            return None;
        }

        let encoding = tileset.raster_encoding.unwrap_or(RasterEncoding::Mapbox);
        let skirt_length = skirt_length(parameters.terrain_skirt_length, parameters.view.zoom());
        self.surface = Surface::new(encoding, terrain.exaggeration, skirt_length);

        let dem_zooms = tileset.zoom_range;
        ranges.update(tileset, dem(&terrain.source), f64::from(terrain.exaggeration));
        let tile_size = data.options.tile_size;
        let view = &parameters.view;
        self.cover = Some(TerrainCover::new(view, &terrain.source, tile_size, dem_zooms, ranges));
        self.cover.as_ref()
    }

    /// Binds each terrain tile's DEM, once the sources have updated, from
    /// the tiles of the terrain's source that `dem` finds by its id. A
    /// frame's bindings are of the DEM tiles it has, so that a frame that
    /// has loaded draws each tile with its own.
    ///
    /// When the ground the tiles draw changed, it tells the map in `events`
    /// ([`RendererEvent::TerrainElevation`]), before the frame finishes:
    /// the tiles, the DEM data they read, or how it decodes. It tells the
    /// map again, though the ground didn't change, when the camera of the
    /// update ([`UpdateParameters::elevation`]) stands on a ground that
    /// differs from it where the camera reads it, as the camera of a map
    /// that let go of the ground when its style dropped the terrain does.
    /// While terrain doesn't draw, it tells the map that there is no ground
    /// once it stops, and for each update whose camera stood on a ground,
    /// which another renderer may have drawn.
    ///
    /// Returns whether the update's camera stands as it would on the ground
    /// the tiles now draw, as it does on none while terrain doesn't draw: a
    /// camera at rest where the two agree at its center (`stands_alike`),
    /// and one that a gesture or a command moves
    /// ([`UpdateParameters::camera_moving`]), which reads the ground away
    /// from its center too, only on that ground itself.
    pub(crate) fn bind<'a>(
        &mut self,
        dem: impl FnOnce(&str) -> Option<&'a SourceTiles>,
        parameters: &UpdateParameters,
        events: &mut Vec<RendererEvent>,
    ) -> bool {
        let seen = parameters.elevation.as_deref();
        let Some(cover) = &mut self.cover else {
            if self.elevation.take().is_some() || seen.is_some() {
                events.push(RendererEvent::TerrainElevation(None));
            }
            return seen.is_none();
        };
        let tiles = dem(&cover.source_id);
        cover.bind(tiles);
        let (unpack, exaggeration) = (self.surface.unpack(), self.surface.props.exaggeration);
        let (drawn, new) = match &self.elevation {
            Some(drawn) if drawn.draws_as(cover, tiles, unpack, exaggeration) => (drawn, false),
            _ => (&*self.elevation.insert(Arc::new(TerrainElevation::new(cover, tiles, unpack, exaggeration))), true),
        };
        let center = parameters.view.center(WrapMode::Wrapped);
        let alike = stands_alike(seen, Some(&**drawn), center);
        if new || !alike {
            events.push(RendererEvent::TerrainElevation(Some(Arc::<TerrainElevation>::clone(drawn))));
        }
        // The map answers the ground it was told of with an update that
        // carries it, so a held gesture's camera comes to stand on it.
        alike && (!parameters.camera_moving || seen.is_some_and(|seen| std::ptr::addr_eq(seen, &raw const **drawn)))
    }

    /// How the surface of the last update's cover draws.
    #[cfg(test)]
    pub(crate) fn surface(&self) -> Surface {
        self.surface
    }

    /// The cover of the last update, while terrain draws: [`update`]'s.
    ///
    /// [`update`]: Self::update
    pub(crate) fn cover(&self) -> Option<&TerrainCover> {
        self.cover.as_ref()
    }

    /// Requests, while terrain draws, the DEM tiles that `tiles`, the tiles
    /// that a standing layer draws, read off the surface's
    /// ([`TerrainCover::request_standing`]), once their source has updated
    /// and before the terrain's own source does.
    pub(crate) fn request_standing(&mut self, tiles: impl IntoIterator<Item = mln_tile_id::UnwrappedTileID>) {
        if let Some(cover) = &mut self.cover {
            cover.request_standing(tiles);
        }
    }

    /// The ground the standing layers of the last update stand on, the
    /// ground its cover drew, as [`bind`](Self::bind) left it; `None` while
    /// terrain doesn't draw. [`TerrainElevation::ground`] gives the ground
    /// under a tile.
    pub(crate) fn ground(&self) -> Option<&Arc<TerrainElevation>> {
        self.elevation.as_ref()
    }

    /// Makes what the encode's draws of the frame of the last update bind of
    /// its ground: the textures of the DEM tiles of the terrain's source,
    /// which `dem` finds by its id, that the surface reads, for the surface
    /// and, where a layer of the frame `stands`, the standing layers, with
    /// those of the DEM tiles their tiles read off the drawn tiles'; and,
    /// where a layer of the frame `hides` behind
    /// the ground, the depth of the drawn ground, at the frame's logical
    /// pixels, which [`render_ground_depth`](Self::render_ground_depth)
    /// draws and the context binds for the frame
    /// ([`Context::ground_depth`]). `None`, dropping the DEM textures and
    /// the ground's depth, for a frame without terrain.
    pub(crate) fn prepare_ground<'a>(
        &mut self,
        parameters: &mut PaintParameters<'_>,
        dem: impl FnOnce(&str) -> Option<&'a SourceTiles>,
        stands: bool,
        hides: bool,
    ) -> Option<Rc<GroundBindings>> {
        let (Some(ground), Some(cover)) = (&self.elevation, &self.cover) else {
            self.dem_textures = DemTextures::default();
            self.ground_depth = None;
            parameters.context.set_ground_depth(None);
            return None;
        };
        let (size, pixel_ratio) = (parameters.renderable_size, parameters.pixel_ratio);
        let context = &mut *parameters.context;
        // The depth keeps its texture, and the draws that bind it their
        // bindings, while the frame's size stays.
        self.ground_depth = hides.then(|| match self.ground_depth.take() {
            Some(depth) if depth.fits(size, pixel_ratio) => depth,
            _ => GroundDepth::new(context, size, pixel_ratio),
        });
        context.set_ground_depth(self.ground_depth.as_ref().map(GroundDepth::texture));
        let mut props = self.surface.props;
        if self.ground_depth.is_some() {
            let view = parameters.view;
            let (near, far) = (view.near_z(), view.far_z());
            props.depth_factor = (2.0 * far * near / (far - near)) as f32;
        }
        let tiles = dem(&cover.source_id);
        let mut textures = mln_util::hash::HashMap::default();
        for read in ground.dems(stands) {
            if let Some(bucket) = tiles.and_then(|tiles| dem_bucket(tiles.tile(&read.id)?)) {
                textures.insert(read.id.canonical, (self.dem_textures.texture(context, bucket), read.dim));
            }
        }
        self.dem_textures.finish_frame();
        let flat = self.dem_textures.flat(context);
        Some(Rc::new(GroundBindings::new(Arc::clone(ground), props, textures, flat)))
    }

    /// Starts the encode's update of the drapes of the frame of the last
    /// update, of `stacks` stacks of draped layers, whose layers then make
    /// their draws in them ([`Drapes::update_layer`]); `None`, dropping the
    /// drapes, for a frame that doesn't draw terrain, a backend without the
    /// terrain's shader, or no stack.
    pub(crate) fn begin_drapes(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        stacks: usize,
    ) -> Option<&mut Drapes> {
        let cover = self.cover.as_ref().filter(|_| stacks > 0);
        let Some(cover) = cover.filter(|_| self.draws.supported(gpu, parameters)) else {
            self.drapes = Drapes::default();
            return None;
        };
        #[cfg(not(test))]
        let byte_cap = drapes::BYTE_CAP;
        #[cfg(test)]
        let byte_cap = self.drape_byte_cap.unwrap_or(drapes::BYTE_CAP);
        let camera = (parameters.view, &parameters.transform_params.proj_matrix, parameters.pixel_ratio);
        let max_texture_size = parameters.context.caps().max_texture_size;
        self.drapes.begin(camera, cover, stacks, (byte_cap, max_texture_size));
        Some(&mut self.drapes)
    }

    /// Makes the draws of the frame of the last update, a set for each stack
    /// of the drapes the encode began ([`begin_drapes`]), which read the DEM
    /// textures of the frame's ground ([`PaintParameters::ground`]), sample
    /// their stack's drapes and fog as the frame's `sky` says. Without a
    /// stack, which a frame without terrain has too, it makes none, and drops
    /// what the draws hold.
    ///
    /// [`begin_drapes`]: Self::begin_drapes
    pub(crate) fn update_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        sky: Option<&EvaluatedSky>,
    ) {
        let ground = parameters.ground.clone();
        let cover = self.cover.as_ref().zip(ground.as_deref());
        let depth = self.ground_depth.is_some();
        self.draws.update(gpu, parameters, cover, self.surface, sky, &self.drapes, depth);
    }

    /// Draws the depth of the frame's drawn ground, outside any pass, before
    /// the draws that read it ([`prepare_ground`](Self::prepare_ground)):
    /// every terrain tile with its skirts, whether or not a stack draws the
    /// surface. A frame without it draws nothing.
    pub(crate) fn render_ground_depth(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        let Some(depth) = self.ground_depth.as_mut().filter(|_| self.draws.draws_depth()) else {
            return;
        };
        let draws = parameters.context.rendering_stats().num_draw_calls;
        if let Some(frame_scissor) = depth.begin(gpu, parameters) {
            self.draws.render_depth(gpu, parameters);
            GroundDepth::end(gpu, parameters, frame_scissor);
        }
        let stats = parameters.context.rendering_stats();
        stats.terrain.ground_depth_draw_calls += usize::try_from(stats.num_draw_calls - draws).unwrap_or(0);
    }

    /// The surface's draws.
    #[cfg(test)]
    pub(crate) fn draws(&self) -> &TerrainDraws {
        &self.draws
    }

    /// Records that the buckets in `changed` changed, for the drapes' draws
    /// built from them.
    pub(crate) fn mark_changed(&mut self, changed: &ChangedBuckets) {
        self.drapes.mark_changed(changed);
    }

    /// Makes every drape draw again in the next frame, for a change that
    /// their keys don't tell: the style's images, or a frame that failed.
    pub(crate) fn redraw_drapes(&mut self) {
        self.drapes.redraw();
    }

    /// The tiles and stacks of the drapes drawn since the last call.
    #[cfg(test)]
    pub(crate) fn take_drape_passes(&mut self) -> Vec<(mln_tile_id::OverscaledTileID, usize)> {
        self.drapes.take_passes()
    }

    /// How many DEM textures it holds, made or waiting to upload, and how
    /// many DEM tiles the drawn tiles of the last update read.
    #[cfg(test)]
    pub(crate) fn dem_textures(&self) -> (usize, usize) {
        let read = self.elevation.as_ref().map_or(0, |ground| ground.dems(false).count());
        (self.dem_textures.len(), read)
    }

    /// The tiles of the drapes and the side their drapes draw at.
    #[cfg(test)]
    pub(crate) fn drape_sizes(&self) -> Vec<(mln_tile_id::OverscaledTileID, u32)> {
        self.drapes.sizes()
    }

    /// Drops the drapes once the map's center crossed the antimeridian: the
    /// tiles are the same ones under new ids.
    pub(crate) fn handle_wrap_jump(&mut self) {
        self.drapes = Drapes::default();
    }

    /// Whether the frame left drapes stale by the camera only, or smaller
    /// than their tile's side, for a later frame to draw
    /// ([`Drapes::waiting`]), which the frame then asks for: a frame that did
    /// isn't complete.
    pub(crate) fn drapes_waiting(&self) -> bool {
        self.drapes.waiting()
    }

    /// Uploads the GPU objects of the frame's draws.
    pub(crate) fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        self.draws.upload(gpu, context);
    }

    /// Makes what the drapes draw with, and uploads their draws, with the
    /// uniforms of a pattern atlas of `pattern_atlas_size`, through `gpu`,
    /// the context of `parameters`, the frame's.
    pub(crate) fn upload_drapes(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        pattern_atlas_size: mln_util::size::Size,
    ) {
        self.drapes.upload(gpu, parameters, pattern_atlas_size);
    }

    /// Draws the drapes into their textures, outside any pass, before the
    /// frame's passes, whose surface samples them.
    pub(crate) fn render_drapes(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        let stats = parameters.context.rendering_stats();
        let (passes, draws) = (stats.num_render_passes, stats.num_draw_calls);
        self.drapes.render(gpu, parameters);
        let stats = parameters.context.rendering_stats();
        stats.terrain.drape_passes += usize::try_from(stats.num_render_passes - passes).unwrap_or(0);
        stats.terrain.drape_draw_calls += usize::try_from(stats.num_draw_calls - draws).unwrap_or(0);
    }

    /// Draws the surface of `stack`, which samples the stack's drapes, into
    /// the open pass of `gpu`, the context's.
    pub(crate) fn render(&mut self, stack: usize, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        let draws = parameters.context.rendering_stats().num_draw_calls;
        self.draws.render(stack, gpu, parameters);
        let stats = parameters.context.rendering_stats();
        stats.terrain.surface_draw_calls += usize::try_from(stats.num_draw_calls - draws).unwrap_or(0);
    }

    /// Counts in the rendering stats of `context` what the terrain holds
    /// once a frame has drawn: its stacks and drapes, and the bytes of the
    /// drapes and what they draw with, of the DEM textures, of the ground's
    /// depth, of the DEM images of the ground it last told the map of, and
    /// of the elevation ranges it remembers.
    pub(crate) fn count(&self, context: &mut Context) {
        self.drapes.count(context);
        let dem_textures = self.dem_textures.bytes(context);
        let stats = &mut context.rendering_stats().terrain;
        stats.mem_dem_textures = dem_textures;
        stats.mem_ground_depth =
            self.ground_depth.as_ref().map_or(0, |depth| usize::try_from(depth.bytes()).unwrap_or(usize::MAX));
        stats.mem_dem_data = self.elevation.as_deref().map_or(0, TerrainElevation::dem_bytes);
        stats.mem_height_ranges = self.cover.as_ref().map_or(0, |cover| cover.ranges.bytes());
    }

    /// Drops all it made from the map's data, for the renderer's
    /// `clear_data`: the cover and the elevation ranges it remembers, the
    /// drapes and the layers' drawings in them, the draws and the DEM
    /// textures. If it had told the map of the ground, it tells it in
    /// `events` that the ground is gone.
    pub(crate) fn clear_data(&mut self, events: &mut Vec<RendererEvent>) {
        if self.elevation.is_some() {
            events.push(RendererEvent::TerrainElevation(None));
        }
        *self = RenderTerrain {
            logged_globe: self.logged_globe,
            #[cfg(test)]
            drape_byte_cap: self.drape_byte_cap,
            ..RenderTerrain::default()
        };
    }

    /// Drops the draws and the drapes, which hold the renderer's shaders,
    /// and the DEM textures; the next frame makes them again.
    pub(crate) fn release_shaders(&mut self) {
        self.drapes = Drapes::default();
        self.draws = TerrainDraws::default();
        self.dem_textures = DemTextures::default();
    }
}
