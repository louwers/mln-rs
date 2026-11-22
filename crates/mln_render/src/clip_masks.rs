use std::collections::BTreeMap;

use mln_gfx::gpu::{Gpu, StencilState, StencilTest};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::types::StencilOpType;
use mln_gfx::{ColorMode, CullFaceMode};
use mln_tile_id::UnwrappedTileID;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;

use crate::draw_objects::Mesh;
use crate::encode::{Depth, Draw, Group};
use crate::gfx::context::Resources;
use crate::globe::grid;
use crate::handles::TileSurface;
use crate::mesh_desc::MeshDesc;
use crate::paint_parameters::{DepthTest, PaintParameters, Stencil};
use crate::shaders::programs;
use crate::tile_matrix::TileProjection;

/// What a mask's mesh is of: on the globe the zoom, whether the tile is on
/// the top or the bottom row (with a pole cap) and whether it has a border;
/// elsewhere [`SQUARE`].
type MeshKey = (u8, bool, bool, bool);

/// The key of the square, which every mask off the globe draws.
const SQUARE: MeshKey = (0, false, false, false);

/// A mask writes its value over its tile, whatever the stencil holds there.
const WRITE: StencilState = StencilState {
    test: StencilTest::Always,
    write_mask: 0xFF,
    fail: StencilOpType::Keep,
    depth_fail: StencilOpType::Keep,
    pass: StencilOpType::Replace,
};

/// The tile clipping masks: for each tile that the frame's draw groups can
/// clip to, the meshes of the clipping mask program that write a stencil
/// value over the tile and nothing else.
pub(crate) struct ClipMasks {
    /// Whether the masks are the globe's.
    globe: bool,
    /// The vertices and triangles of the masks, which tiles alike share.
    surfaces: BTreeMap<MeshKey, TileSurface>,
    /// Each tile's meshes, each with the tile's projection: on the globe
    /// with a border and then without, elsewhere the square.
    masks: BTreeMap<UnwrappedTileID, Vec<Mesh>>,
    /// A draw of each of the masks' meshes, which writes its projection.
    draws: Group,
}

impl Default for ClipMasks {
    fn default() -> Self {
        ClipMasks { globe: false, surfaces: BTreeMap::new(), masks: BTreeMap::new(), draws: Group::new() }
    }
}

impl ClipMasks {
    /// Makes and uploads the masks of `tiles`, sorted and without repeats,
    /// that `projection` projects, and drops the others. Each mask is drawn
    /// over the area that `projection` gives its tile.
    pub(crate) fn update(
        &mut self,
        gpu: &mut dyn Gpu,
        context: &mut Resources,
        globe: bool,
        tiles: &[UnwrappedTileID],
        projection: impl Fn(&UnwrappedTileID) -> TileProjection,
    ) {
        if globe != self.globe {
            *self = ClipMasks { globe, ..ClipMasks::default() };
        }
        self.masks.retain(|tile, _| tiles.binary_search(tile).is_ok());
        self.draws.begin(0.0, false, false);
        for tile in tiles {
            let ClipMasks { surfaces, masks, draws, .. } = self;
            let meshes = masks.entry(*tile).or_insert_with(|| {
                let canonical = &tile.canonical;
                let keys = if globe {
                    let (top, bottom) = (canonical.y == 0, canonical.y == (1u32 << canonical.z) - 1);
                    vec![(canonical.z, top, bottom, true), (canonical.z, top, bottom, false)]
                } else {
                    vec![SQUARE]
                };
                keys.into_iter()
                    .map(|key| {
                        let surface = surfaces.entry(key).or_insert_with(|| surface(context, tile, key, globe));
                        mesh(context, surface, globe)
                    })
                    .collect()
            });
            let at = projection(tile);
            for mesh in meshes {
                draws.push_at(mesh, &at, Draw { depth: Depth::Off, ..Draw::DEFAULT }, ());
            }
        }
        self.draws.finish(gpu, context, &(), []);
        self.draws.upload(gpu, context);
        if self.masks.is_empty() {
            self.surfaces.clear();
        }
    }

    /// Draws the mask of each tile of `masks` with its stencil value into
    /// `parameters`' open pass. Every tile must be one `update` made a mask
    /// for.
    pub(crate) fn draw(
        &self,
        gpu: &mut dyn Gpu,
        masks: &[(UnwrappedTileID, i32)],
        parameters: &mut PaintParameters<'_>,
    ) {
        // On the globe each mask is drawn with a border first, then exactly:
        // where a tile meets finer ones, its coarser edge leaves a sliver
        // between the masks that only a neighbor's border covers.
        let passes = if self.globe { 2 } else { 1 };
        for pass in 0..passes {
            for (tile, reference) in masks {
                let Some(mesh) = self.masks.get(tile).and_then(|meshes| meshes.get(pass)) else {
                    debug_assert!(false, "no clipping mask for {tile:?}");
                    continue;
                };
                let stencil = Stencil { state: WRITE, reference: *reference as u32 };
                self.draws.draw_mesh(gpu, parameters, mesh, 0, DepthTest::DISABLED, stencil);
            }
        }
    }
}

/// The vertices and triangles of the masks of `key`, as `tile`'s are: on
/// the globe its grid, elsewhere the square.
fn surface(context: &mut Resources, tile: &UnwrappedTileID, key: MeshKey, globe: bool) -> TileSurface {
    if globe {
        let (_, _, _, borders) = key;
        let canonical = &tile.canonical;
        let granularity = SubdivisionGranularitySetting::globe().stencil.granularity_for_zoom_level(canonical.z);
        let grid = grid(canonical, granularity, borders);
        TileSurface {
            vertices: context.buffer(&grid.vertices),
            triangles: context.index_buffer(grid.triangles.as_flattened()),
        }
    } else {
        context.square_surface()
    }
}

/// A mask's mesh of `surface`, whose draws write the projection of their
/// tile to its buffers.
fn mesh(context: &mut Resources, surface: &TileSurface, globe: bool) -> Mesh {
    let mut mesh = MeshDesc::new(&programs::CLIPPING_MASK, &surface.triangles);
    // The far side of a limb tile projects inside the disc; cull it or it
    // overwrites the tile in front.
    let cull = if globe { CullFaceMode::back_ccw() } else { CullFaceMode::disabled() };
    let variant = if globe { ProjectionVariant::Globe } else { ProjectionVariant::Mercator };
    mesh.vertices(&surface.vertices).blend(ColorMode::disabled()).cull(cull).projection(variant);
    context.mesh(&mesh)
}
