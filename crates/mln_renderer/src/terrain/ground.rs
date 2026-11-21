//! The ground the standing layers of a frame stand on: for each of their
//! tiles, the DEM tiles the surface reads under it, and the part of the tile
//! each one is under.

use std::sync::Arc;

use glam::Vec2;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::shader_defines::ID_PROJECTION_UBO;
use mln_gfx::shaders::terrain_ubo::{GroundDrawableUBO, TerrainProjectionUBO, TerrainPropsUBO};
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::constants::EXTENT;
use mln_util::dem::{DemCoords, DemUnpack};
use mln_util::hash::HashMap;
use smallvec::SmallVec;

use super::elevation::{DrawnDem, DrawnOver, TerrainElevation, dem_height};
use crate::gfx::store::SharedTexture;
use crate::layer_drawing::UniformWriter;

/// A part of a tile, in its tile units: x0, y0, x1, y1. It holds a point at
/// least x0 and y0 and under x1 and y1.
type Rect = [f32; 4];

/// How far past a tile's edges a part of it on an edge reaches where it
/// draws every feature of the tile ([`TileFeatures::All`]), in tile units:
/// so far that the part holds every point past that edge.
const BEYOND: f32 = 1.0e9;

/// Which of the features of a tile of a standing layer its draws over
/// terrain draw, by where the features' anchors are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TileFeatures {
    /// Those whose anchor is in the tile, from 0 to under [`EXTENT`] on both
    /// axes: points, such as circles and heatmap kernels, whose copy in a
    /// neighbour's buffer that neighbour's draws leave to the tile that holds
    /// it.
    Anchored,
    /// All of them: the pieces that the tile's bucket cut features into at
    /// its edges, such as extrusions', whose anchor, a piece's centroid, may
    /// lie outside the tile.
    All,
}

impl TileFeatures {
    /// Where a tile's parts on its low and high edges end, in its tile
    /// units, on either axis.
    fn edges(self) -> (f32, f32) {
        match self {
            TileFeatures::Anchored => (0.0, EXTENT as f32),
            TileFeatures::All => (-BEYOND, BEYOND),
        }
    }

    /// The whole of a tile, with what its parts on its edges reach past
    /// them.
    fn whole(self) -> Rect {
        let (low, high) = self.edges();
        [low, low, high, high]
    }
}

/// Whether `outer` holds `inner`, or is it.
fn holds(outer: CanonicalTileID, inner: CanonicalTileID) -> bool {
    inner.z >= outer.z && {
        let dz = inner.z - outer.z;
        inner.x >> dz == outer.x && inner.y >> dz == outer.y
    }
}

/// The parts of `node`, `tile` or a tile inside it, that read one DEM tile
/// each, pushed to `parts`, of the drawn tiles `under` inside `tile`, each
/// with the DEM it reads (`None` where flat), whose DEM tile `dem_of` gives:
/// a part whose drawn tiles all read one DEM tile, or the same flat ground,
/// reads it, and other parts split in four, so that each drawn tile is in a
/// part that reads its DEM tile. A part that no drawn tile is in reads the
/// DEM tile `held` gives it.
fn partition<D: Copy>(
    node: CanonicalTileID,
    under: &[(CanonicalTileID, Option<D>)],
    dem_of: &impl Fn(D) -> CanonicalTileID,
    held: &impl Fn(CanonicalTileID) -> Option<D>,
    parts: &mut SmallVec<[(CanonicalTileID, Option<D>); 1]>,
) {
    let inside = || under.iter().filter(move |(drawn, _)| holds(node, *drawn)).map(|&(_, dem)| dem);
    let Some(first) = inside().next() else {
        parts.push((node, held(node)));
        return;
    };
    let tile_of = |dem: Option<D>| dem.map(dem_of);
    if inside().all(|dem| tile_of(dem) == tile_of(first)) {
        parts.push((node, first));
        return;
    }
    // Drawn tiles don't overlap, so the parts split no deeper than they are.
    for child in node.children() {
        partition(child, under, dem_of, held, parts);
    }
}

/// The part `part` of `tile`, `tile` or a tile inside it, that draws
/// `features`: reaching out past the edges it shares with `tile` where it
/// draws all the features.
fn rect(tile: CanonicalTileID, part: CanonicalTileID, features: TileFeatures) -> Rect {
    let dz = part.z - tile.z;
    let size = EXTENT as f32 / f32::from(dz).exp2();
    let x = (part.x - (tile.x << dz)) as f32 * size;
    let y = (part.y - (tile.y << dz)) as f32 * size;
    let (low_edge, high_edge) = features.edges();
    let low = |at: f32| if at == 0.0 { low_edge } else { at };
    let high = |at: f32| if at == EXTENT as f32 { high_edge } else { at };
    [low(x), low(y), high(x + size), high(y + size)]
}

/// Where `tile` is in the DEM tile `dem`, which may hold it or be inside it:
/// the scale and offset from the tile's uv to the DEM's.
fn dem_coords(tile: CanonicalTileID, dem: CanonicalTileID) -> DemCoords {
    let scale = (f64::from(dem.z) - f64::from(tile.z)).exp2();
    let offset = |tile: u32, dem: u32| (f64::from(tile) * scale - f64::from(dem)) as f32;
    DemCoords { scale: scale as f32, offset: [offset(tile.x, dem.x), offset(tile.y, dem.y)] }
}

/// What a draw of a part of a tile reads of the ground: the DEM tile, where
/// the tile is in it, and the part.
#[derive(Clone, Copy)]
pub(crate) struct GroundRead<'a> {
    /// `None` where the ground is flat.
    dem: Option<&'a DrawnDem>,
    coords: DemCoords,
    rect: Rect,
}

impl<'a> GroundRead<'a> {
    /// The read of `dem` by a draw of the part `rect` of `tile`.
    fn new(tile: CanonicalTileID, dem: Option<&'a DrawnDem>, rect: Rect) -> GroundRead<'a> {
        let coords = dem.map_or(DemCoords::OWN, |dem| dem_coords(tile, dem.id.canonical));
        GroundRead { dem, coords, rect }
    }

    /// Whether the part holds `tile_point`, in tile units of the tile.
    fn holds(&self, tile_point: Vec2) -> bool {
        let [x0, y0, x1, y1] = self.rect;
        (x0..x1).contains(&tile_point.x) && (y0..y1).contains(&tile_point.y)
    }

    /// The DEM tile it reads; `None` where flat.
    pub(crate) fn dem(&self) -> Option<OverscaledTileID> {
        self.dem.map(|dem| dem.id)
    }

    /// [`GroundDrawableUBO::dem_coords`]: the DEM uv's scale and offset, then
    /// the DEM's width in pixels, 0 where flat.
    pub(crate) fn coords(&self) -> [f32; 4] {
        match self.dem {
            Some(dem) => [self.coords.scale, self.coords.offset[0], self.coords.offset[1], dem.dim as f32],
            None => [0.0; 4],
        }
    }

    /// [`GroundDrawableUBO::rect`].
    pub(crate) fn rect(&self) -> [f32; 4] {
        self.rect
    }
}

/// The reads of a tile's ground: most often one. Unlike a `SmallVec`, it
/// lets a `TileGround` borrowed for less than its DEM images live read as
/// one of that shorter life.
enum Reads<'a> {
    One(GroundRead<'a>),
    Parts(Box<[GroundRead<'a>]>),
}

/// The ground under a tile of a standing layer: the DEM tiles that the
/// surface reads under it, each with the part of the tile it is under
/// ([`TerrainElevation::ground`]). A draw of the tile reads one of them, and
/// draws the features whose untranslated anchor is on its part.
pub(crate) struct TileGround<'a> {
    /// One or more, whose parts hold each point of the tile once, and each
    /// point past its edges once or, for [`TileFeatures::Anchored`], never.
    reads: Reads<'a>,
    /// How the DEM decodes, and the exaggeration.
    decode: (DemUnpack, f32),
}

impl<'a> TileGround<'a> {
    /// What the tile's draws read, a draw each.
    pub(crate) fn reads(&self) -> &[GroundRead<'a>] {
        match &self.reads {
            Reads::One(read) => std::slice::from_ref(read),
            Reads::Parts(reads) => reads,
        }
    }

    /// Whether a draw of the tile draws a feature whose untranslated anchor
    /// is `anchor`, in tile units of the tile.
    pub(crate) fn holds(&self, anchor: Vec2) -> bool {
        self.reads().iter().any(|read| read.holds(anchor))
    }

    /// The height in meters, with the exaggeration, at `at` of a feature
    /// whose untranslated anchor is `anchor`, both in tile units of the tile:
    /// as the draw that draws the feature reads it, by the height rule the
    /// shaders read the DEM by; 0 where flat. `None` where no draw of the
    /// tile draws the feature.
    pub(crate) fn height(&self, anchor: Vec2, at: Vec2) -> Option<f64> {
        let read = self.reads().iter().find(|read| read.holds(anchor))?;
        let Some(dem) = read.dem else {
            return Some(0.0);
        };
        let extent = EXTENT as f32;
        let (unpack, exaggeration) = self.decode;
        Some(dem_height(&dem.image, dem.dim, read.coords, [at.x / extent, at.y / extent], unpack, exaggeration))
    }
}

impl TerrainElevation {
    /// The ground under `tile`, a tile of a standing layer's source whose
    /// draws draw `features`: the DEM tiles the drawn tiles over it read, as
    /// these tiles drew. A tile in a drawn tile reads that one's DEM tile. A
    /// tile that drawn tiles are inside reads, on each of its parts, the DEM
    /// tile the drawn tiles there read, whether their own or, while that
    /// loads, an ancestor ([`partition`]). A part with no drawn tile, off the
    /// cover, reads the deepest DEM tile that holds it of those the cover
    /// reads or loads to cull its tiles, or is flat.
    pub(crate) fn ground(&self, tile: &OverscaledTileID, features: TileFeatures) -> TileGround<'_> {
        let canonical = tile.canonical;
        let reads = match self.drawn_over(tile) {
            DrawnOver::Holding(dem) => Reads::One(GroundRead::new(canonical, dem, features.whole())),
            DrawnOver::Inside(under) => {
                let mut parts = SmallVec::new();
                let dem_of = |dem: &DrawnDem| dem.id.canonical;
                partition(canonical, &under, &dem_of, &|part| self.dem_holding(part), &mut parts);
                let read = |(part, dem)| GroundRead::new(canonical, dem, rect(canonical, part, features));
                match parts.as_slice() {
                    &[part] => Reads::One(read(part)),
                    _ => Reads::Parts(parts.into_iter().map(read).collect()),
                }
            }
        };
        TileGround { reads, decode: (self.unpack(), self.exaggeration()) }
    }
}

/// What a draw of a part of a tile binds of its ground: the DEM texture
/// ([`ID_GROUND_DEM_TEXTURE`](mln_gfx::shaders::shader_defines::ID_GROUND_DEM_TEXTURE)),
/// and the ground of its projection entry
/// ([`TerrainProjectionUBO::ground`](mln_gfx::shaders::terrain_ubo::TerrainProjectionUBO::ground)).
pub(crate) struct GroundBinding {
    pub(crate) texture: SharedTexture,
    pub(crate) ubo: GroundDrawableUBO,
}

/// What the standing layers' draws of a frame bind of its ground: the DEM
/// textures, made for every DEM tile the surface reads, and the decode.
pub(crate) struct GroundBindings {
    elevation: Arc<TerrainElevation>,
    props: TerrainPropsUBO,
    /// The texture of each DEM tile, with its width in pixels.
    textures: HashMap<CanonicalTileID, (SharedTexture, u32)>,
    /// A transparent texture, of flat ground and of a drape that holds
    /// nothing.
    flat: SharedTexture,
}

impl GroundBindings {
    /// The bindings of the ground of `elevation`, whose DEM decodes by
    /// `props`, with the DEM textures `textures`, by their DEM tiles, with
    /// their widths in pixels, and the transparent `flat`.
    pub(crate) fn new(
        elevation: Arc<TerrainElevation>,
        props: TerrainPropsUBO,
        textures: HashMap<CanonicalTileID, (SharedTexture, u32)>,
        flat: SharedTexture,
    ) -> GroundBindings {
        GroundBindings { elevation, props, textures, flat }
    }

    /// What the draws of `tile`, which draw `features`, bind of its ground,
    /// a draw each: one for each DEM tile the surface reads under it
    /// ([`TerrainElevation::ground`]), most often one.
    pub(crate) fn reads(&self, tile: &OverscaledTileID, features: TileFeatures) -> SmallVec<[GroundBinding; 1]> {
        let ground = self.elevation.ground(tile, features);
        let bind = |read: &GroundRead<'_>| {
            let texture = read.dem().and_then(|dem| self.textures.get(&dem.canonical));
            let (texture, dem_coords) = match texture {
                Some((texture, _)) => (texture.clone(), read.coords()),
                None => (self.flat.clone(), [0.0; 4]),
            };
            GroundBinding { texture, ubo: GroundDrawableUBO { dem_coords, rect: read.rect(), props: self.props } }
        };
        ground.reads().iter().map(bind).collect()
    }

    /// The texture of the DEM tile `dem`, with its width in pixels, if the
    /// surface reads it.
    pub(crate) fn dem(&self, dem: &OverscaledTileID) -> Option<(&SharedTexture, u32)> {
        self.textures.get(&dem.canonical).map(|(texture, dim)| (texture, *dim))
    }

    /// A transparent 1×1 texture.
    pub(crate) fn flat(&self) -> &SharedTexture {
        &self.flat
    }
}

/// Writes each draw's [`ID_PROJECTION_UBO`] entry: its `projection`, and,
/// where the draws stand on terrain, the ground under it, of `grounds`, one
/// for each draw in the order the layer pushed them
/// ([`TerrainProjectionUBO`]).
pub(crate) fn write_projection<U>(
    writer: &mut UniformWriter<'_, U>,
    grounds: Option<&[GroundDrawableUBO]>,
    projection: impl Fn(&U) -> ProjectionUBO,
) {
    match grounds {
        Some(grounds) => writer.storage_with(ID_PROJECTION_UBO, grounds, |uniforms, &ground| TerrainProjectionUBO {
            projection: projection(uniforms),
            ground,
        }),
        None => writer.storage(ID_PROJECTION_UBO, projection),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(z: u8, x: u32, y: u32) -> CanonicalTileID {
        CanonicalTileID::new(z, x, y)
    }

    /// The parts of `tile` over the drawn tiles `under` with their DEM
    /// tiles, where `held` is the DEM tile the cover holds over a part with
    /// no drawn tile.
    fn parts(
        tile: CanonicalTileID,
        under: &[(CanonicalTileID, Option<CanonicalTileID>)],
        held: Option<CanonicalTileID>,
    ) -> Vec<(CanonicalTileID, Option<CanonicalTileID>)> {
        let mut parts = SmallVec::new();
        partition(tile, under, &|dem| dem, &|_| held, &mut parts);
        parts.into_vec()
    }

    /// Drawn tiles that each read their parent DEM tile, two levels below
    /// the tile, part it by those DEM tiles, one level below it.
    #[test]
    fn a_tile_over_four_dem_tiles_has_four_parts() {
        let under: Vec<_> = tile(14, 5, 9)
            .children()
            .into_iter()
            .flat_map(|dem| dem.children().map(|drawn| (drawn, Some(dem))))
            .collect();
        let mut expected: Vec<_> = tile(14, 5, 9).children().map(|dem| (dem, Some(dem))).into();
        expected.sort();
        let mut got = parts(tile(14, 5, 9), &under, None);
        got.sort();
        assert_eq!(expected, got);
    }

    /// Drawn tiles whose DEM tiles nest, a DEM tile a level below the tile
    /// read in three quarters of it and one two levels below in the last
    /// quarter, part it by the drawn tiles there, so that each reads its own
    /// DEM tile and no point is in two parts.
    #[test]
    fn nested_dem_tiles_part_by_the_drawn_tiles() {
        let [q0, q1, q2, q3] = tile(14, 0, 0).children();
        let [a, b, c, d] = q0.children();
        let under: Vec<_> = [(a, Some(q0)), (b, Some(q0)), (c, Some(q0))]
            .into_iter()
            .chain(d.children().map(|drawn| (drawn, Some(d))))
            .collect();
        let mut got = parts(tile(14, 0, 0), &under, None);
        got.sort();
        let mut expected = vec![(a, Some(q0)), (b, Some(q0)), (c, Some(q0)), (d, Some(d))];
        expected.extend([(q1, None), (q2, None), (q3, None)]);
        expected.sort();
        assert_eq!(expected, got);
    }

    /// A drawn tile that reads an ancestor while its own DEM tile loads,
    /// among others that read their own, reads it on its part.
    #[test]
    fn a_drawn_tile_reading_an_ancestor_reads_it_on_its_part() {
        let [a, b, c, d] = tile(14, 1, 1).children();
        let ancestor = tile(5, 0, 0);
        let under = [(a, Some(a)), (b, Some(ancestor)), (c, Some(c)), (d, Some(d))];
        let mut got = parts(tile(14, 1, 1), &under, None);
        got.sort();
        assert_eq!(vec![(a, Some(a)), (b, Some(ancestor)), (c, Some(c)), (d, Some(d))], got);
    }

    /// A part where the surface is flat reads flat ground, and a part with
    /// no drawn tile, off the cover, reads the DEM tile the cover holds
    /// over it.
    #[test]
    fn flat_and_uncovered_parts() {
        let [q0, q1, q2, q3] = tile(14, 0, 0).children();
        let under = [(q0, None), (q1, Some(q1)), (q2, Some(q2))];
        let held = tile(10, 0, 0);
        let mut got = parts(tile(14, 0, 0), &under, Some(held));
        got.sort();
        assert_eq!(vec![(q0, None), (q1, Some(q1)), (q2, Some(q2)), (q3, Some(held))], got);
    }

    /// The parts of a tile that draw all its features reach out past its
    /// own edges, and meet each other without a gap.
    #[test]
    fn parts_of_all_features_reach_past_the_tiles_edges() {
        let [q0, _, _, q3] = tile(3, 1, 2).children();
        let all = TileFeatures::All;
        assert_eq!([-BEYOND, -BEYOND, 4096.0, 4096.0], rect(tile(3, 1, 2), q0, all));
        assert_eq!([4096.0, 4096.0, BEYOND, BEYOND], rect(tile(3, 1, 2), q3, all));
        assert_eq!([-BEYOND, -BEYOND, BEYOND, BEYOND], rect(tile(3, 1, 2), tile(3, 1, 2), all));
    }

    /// The parts of a tile that draw the features anchored in it stop at its
    /// edges, so that they hold no point of a neighbour's.
    #[test]
    fn parts_of_anchored_features_stop_at_the_tiles_edges() {
        let [q0, _, _, q3] = tile(3, 1, 2).children();
        let anchored = TileFeatures::Anchored;
        assert_eq!([0.0, 0.0, 4096.0, 4096.0], rect(tile(3, 1, 2), q0, anchored));
        assert_eq!([4096.0, 4096.0, 8192.0, 8192.0], rect(tile(3, 1, 2), q3, anchored));
        assert_eq!([0.0, 0.0, 8192.0, 8192.0], rect(tile(3, 1, 2), tile(3, 1, 2), anchored));
    }

    /// A tile's place in a DEM tile that holds it, and in one inside it.
    #[test]
    fn dem_coords_of_a_dem_tile_holding_the_tile_or_inside_it() {
        let outer = dem_coords(tile(14, 5, 6), tile(12, 1, 1));
        assert_eq!(DemCoords { scale: 0.25, offset: [0.25, 0.5] }, outer);
        let inner = dem_coords(tile(14, 5, 6), tile(15, 11, 12));
        assert_eq!(DemCoords { scale: 2.0, offset: [-1.0, 0.0] }, inner);
    }
}
