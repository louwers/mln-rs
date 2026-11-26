// An image source draws one image, stretched over the quad of its
// coordinates, in the tiles that the quad covers. The raster layers that
// draw it borrow its bucket from the source for the frame.

use std::sync::Arc;

use geometry::Point;
use mln_style::feature::GeometryCoordinates;
use mln_style::geo::LatLngBounds;
use mln_style::range::Range;
use mln_style::source::SourceData;
use mln_style::sources::ImageSourceType;
use mln_style::subdivision_granularity::SubdivisionGranularitySetting;
use mln_style::{Source, constants};
use mln_tile_id::UnwrappedTileID;

use crate::buckets::raster_bucket::{RasterBucket, RasterLayoutVertex};
use crate::camera::TileCoordinate;
use crate::camera::tile_coordinate::TileCoordinatePoint;
use crate::draw::context::Context;
use crate::draw::segment::SegmentBase;
use crate::layer::bucket::UploadNeeds;
use crate::layer::layer_type::ErasedEvaluatedLayer;
use crate::log_target;
use crate::source::render_source::{RenderSource, RenderSourceBase, RenderTileParameters};
use crate::tile_store::store::TileStore;
use crate::tile_store::tile_cover::{TileCoverParameters, tile_cover, tile_cover_bounds};
use crate::tile_store::tile_parameters::TileParameters;

// The mapping from the unit square onto the image's corners: the perspective
// terms of the projective warp and how far it is blended towards the
// bilinear one, as the "auto" warp blends them, after Heckbert,
// "Fundamentals of Texture Mapping and Image Warping" (1989), 2.2.3 and A.2.
#[derive(Clone, Copy, Debug)]
struct ImageWarp {
    perspective_x: f64,
    perspective_y: f64,
    blend: f64,
}

impl Default for ImageWarp {
    fn default() -> Self {
        ImageWarp { perspective_x: 0.0, perspective_y: 0.0, blend: 1.0 }
    }
}

/// The cells across a side of the grid that draws a warp blended towards
/// the bilinear one, GL JS's `SUBDIVIDED_QUAD_GRANULARITY`.
const BLENDED_WARP_CELLS: u32 = 16;

fn is_parallelogram(corners: &GeometryCoordinates) -> bool {
    let [top_left, top_right, bottom_right, bottom_left] =
        [corners[0], corners[1], corners[2], corners[3]].map(|p| (i32::from(p.x), i32::from(p.y)));
    top_left.0 + bottom_right.0 == top_right.0 + bottom_left.0
        && top_left.1 + bottom_right.1 == top_right.1 + bottom_left.1
}

/// The corner of a concave quad of `corners` (top left, top right, bottom
/// right, bottom left) that points into it, by its index; `None` for a
/// quad that isn't concave.
fn concave_corner(corners: &GeometryCoordinates) -> Option<usize> {
    let point = |i: usize| (i64::from(corners[i % 4].x), i64::from(corners[i % 4].y));
    // Which way the outline turns at each corner: a concave quad turns
    // against the other three at one.
    let turns = [0, 1, 2, 3].map(|i| {
        let ((ax, ay), (bx, by), (cx, cy)) = (point(i + 3), point(i), point(i + 1));
        ((bx - ax) * (cy - by) - (by - ay) * (cx - bx)).signum()
    });
    (0..4).find(|&i| turns[i] != 0 && (0..4).all(|j| j == i || turns[j] == -turns[i]))
}

fn image_warp(corners: &GeometryCoordinates) -> ImageWarp {
    const PROJECTIVE_FORESHORTENING: f64 = 4.0;
    const BILINEAR_FORESHORTENING: f64 = 512.0;
    // A parallelogram is warped affinely either way.
    if is_parallelogram(corners) {
        return ImageWarp::default();
    }
    let top_left = corners[0];
    let top_right = corners[1];
    let bottom_right = corners[2];
    let bottom_left = corners[3];
    let (tl_x, tl_y) = (i32::from(top_left.x), i32::from(top_left.y));
    let (tr_x, tr_y) = (i32::from(top_right.x), i32::from(top_right.y));
    let (br_x, br_y) = (i32::from(bottom_right.x), i32::from(bottom_right.y));
    let (bl_x, bl_y) = (i32::from(bottom_left.x), i32::from(bottom_left.y));
    let sum_x = f64::from(tl_x - tr_x + br_x - bl_x);
    let sum_y = f64::from(tl_y - tr_y + br_y - bl_y);
    let right_x = f64::from(tr_x - br_x);
    let right_y = f64::from(tr_y - br_y);
    let down_x = f64::from(bl_x - br_x);
    let down_y = f64::from(bl_y - br_y);
    let determinant = right_x * down_y - right_y * down_x;
    let perspective_x = (sum_x * down_y - down_x * sum_y) / determinant;
    let perspective_y = (right_x * sum_y - sum_x * right_y) / determinant;
    // The homogeneous denominator at the four corners, normalized to one at
    // the top left; its spread is the foreshortening, and it stays finite
    // and positive only while the quad is a perspective view of the image.
    let denominators = [1.0, 1.0 + perspective_x, 1.0 + perspective_x + perspective_y, 1.0 + perspective_y];
    // The first largest and smallest, which comparisons with a NaN keep.
    let mut largest = denominators[0];
    let mut smallest = denominators[0];
    for &denominator in &denominators[1..] {
        if largest < denominator {
            largest = denominator;
        }
        if denominator < smallest {
            smallest = denominator;
        }
    }
    let foreshortening = largest / smallest;
    let blend_term = (1.0 - PROJECTIVE_FORESHORTENING / foreshortening)
        / (1.0 - PROJECTIVE_FORESHORTENING / BILINEAR_FORESHORTENING);
    let blend = if 0.0 < blend_term { blend_term } else { 0.0 };
    if !foreshortening.is_finite() || foreshortening < 1.0 || foreshortening > BILINEAR_FORESHORTENING || blend >= 1.0 {
        return ImageWarp::default();
    }
    ImageWarp { perspective_x, perspective_y, blend }
}

/// Where the raster shader puts the unit square that a draw's vertices span
/// (0 to `EXTENT` on both axes), in its tile's units: onto the quad of four
/// corners, warped by an [`ImageWarp`]. The shader evaluates the warp per
/// vertex and interpolates the texture coordinates projectively, so a
/// projective warp draws exactly on two triangles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ImageQuad {
    /// The top left corner's x and y, and the top right's.
    pub(crate) coords_top: [f32; 4],
    /// The bottom left corner's x and y, and the bottom right's.
    pub(crate) coords_bottom: [f32; 4],
    /// The warp's perspective terms and its blend towards the bilinear warp,
    /// then padding.
    pub(crate) warp: [f32; 4],
}

impl ImageQuad {
    /// The quad of a tile itself, which draws the vertices where they are.
    pub(crate) const TILE: ImageQuad = {
        let extent = constants::EXTENT as f32;
        ImageQuad {
            coords_top: [0.0, 0.0, extent, 0.0],
            coords_bottom: [0.0, extent, extent, extent],
            warp: [0.0, 0.0, 1.0, 0.0],
        }
    };

    fn new(corners: &GeometryCoordinates, warp: &ImageWarp) -> ImageQuad {
        let [top_left, top_right, bottom_right, bottom_left] =
            [corners[0], corners[1], corners[2], corners[3]].map(|p| [f32::from(p.x), f32::from(p.y)]);
        ImageQuad {
            coords_top: [top_left[0], top_left[1], top_right[0], top_right[1]],
            coords_bottom: [bottom_left[0], bottom_left[1], bottom_right[0], bottom_right[1]],
            warp: [warp.perspective_x as f32, warp.perspective_y as f32, warp.blend as f32, 0.0],
        }
    }
}

fn to_vertex_coordinate(value: f64) -> i16 {
    mln_style::math::clamp_f64(value.round(), f64::from(i16::MIN), f64::from(i16::MAX)) as i16
}

// The unit square as a grid of `cells` x `cells`, textured edge to edge,
// which the raster shader puts on the image's quad, each cell split along
// its diagonal from the top right, or from the top left if `from_top_left`.
fn build_image_mesh(bucket: &mut RasterBucket, cells: u32, from_top_left: bool) {
    let rows = cells + 1;
    let extent = constants::EXTENT as u32;
    let mut vertices = Vec::with_capacity((rows * rows) as usize);
    for j in 0..rows {
        for i in 0..rows {
            let (x, y) = ((i * extent / cells) as u16, (j * extent / cells) as u16);
            vertices.push(RasterLayoutVertex::new(Point::new(x as i16, y as i16), Point::new(x, y)));
        }
    }
    let mut indices: Vec<u16> = Vec::with_capacity((cells * cells * 6) as usize);
    for j in 0..cells {
        for i in 0..cells {
            let top_left = (j * rows + i) as u16;
            let top_right = top_left.wrapping_add(1);
            let bottom_left = top_left.wrapping_add(rows as u16);
            let bottom_right = bottom_left.wrapping_add(1);
            if from_top_left {
                indices.extend_from_slice(&[top_left, top_right, bottom_right]);
                indices.extend_from_slice(&[top_left, bottom_right, bottom_left]);
            } else {
                indices.extend_from_slice(&[top_left, top_right, bottom_left]);
                indices.extend_from_slice(&[top_right, bottom_left, bottom_right]);
            }
        }
    }
    let segments = vec![SegmentBase::with_lengths(0, 0, vertices.len(), indices.len())];
    bucket.set_mesh(vertices, indices, segments);
}

/// What a frame draws of an image source: its bucket, once the image has
/// arrived, in each of the tiles it covers.
#[derive(Clone, Copy)]
pub(crate) struct ImageSourceRenderData<'a> {
    pub(crate) bucket: Option<&'a RasterBucket>,
    /// The tile the image draws in, in each world copy it draws in: one
    /// canonical tile under different wraps.
    pub(crate) tile_ids: &'a [UnwrappedTileID],
    /// The box of the quad's corners, in the tile units of `tile_ids`'
    /// canonical tile: min x, min y, max x, max y.
    pub(crate) bounds: [i32; 4],
    /// Where the bucket's vertices draw, in the same units.
    pub(crate) quad: ImageQuad,
}

impl ImageSourceRenderData<'_> {
    /// Whether the image drawn in `tile_id`, one of `tile_ids`, covers part
    /// of the tile `drape`. An image that only touches its edge doesn't.
    pub(crate) fn overlaps(&self, tile_id: &UnwrappedTileID, drape: &UnwrappedTileID) -> bool {
        // Both boxes in the tile units of the finer of the two zooms, from
        // the west edge of world copy 0.
        let zoom = tile_id.canonical.z.max(drape.canonical.z);
        let place = |id: &UnwrappedTileID| {
            let scale = 1i64 << (zoom - id.canonical.z);
            let extent = i64::from(constants::EXTENT) * scale;
            let x = ((i64::from(id.wrap) << id.canonical.z) + i64::from(id.canonical.x)) * extent;
            (x, i64::from(id.canonical.y) * extent, scale)
        };
        let (left, top, scale) = place(tile_id);
        let [min_x, min_y, max_x, max_y] = self.bounds.map(|bound| i64::from(bound) * scale);
        let (drape_left, drape_top, drape_scale) = place(drape);
        let drape_extent = i64::from(constants::EXTENT) * drape_scale;
        left + min_x < drape_left + drape_extent
            && drape_left < left + max_x
            && top + min_y < drape_top + drape_extent
            && drape_top < top + max_y
    }

    /// The copies of the image that cover part of the tile `drape`
    /// ([`overlaps`](Self::overlaps)): of `tile_ids`, or on the `globe`,
    /// whose one copy stands for every world copy, of that copy in the
    /// drape's world and the worlds either side, which a quad that crosses
    /// the antimeridian reaches into.
    pub(crate) fn copies_over(
        &self,
        drape: &UnwrappedTileID,
        globe: bool,
    ) -> impl Iterator<Item = UnwrappedTileID> + use<'_> {
        let drape = *drape;
        let copies = self.tile_ids.iter().flat_map(move |copy| {
            let wraps = if globe { drape.wrap - 1..=drape.wrap + 1 } else { copy.wrap..=copy.wrap };
            wraps.map(|wrap| UnwrappedTileID::with_wrap(wrap, copy.canonical))
        });
        copies.filter(move |copy| self.overlaps(copy, &drape))
    }
}

pub(crate) struct RenderImageSource {
    base: RenderSourceBase,
    bucket: Option<RasterBucket>,
    tile_ids: Vec<UnwrappedTileID>,
    mesh_corners: GeometryCoordinates,
    /// The box of `mesh_corners`, as [`ImageSourceRenderData::bounds`].
    bounds: [i32; 4],
    quad: ImageQuad,
    mesh_cells: u32,
}

impl RenderImageSource {
    #[must_use]
    pub(crate) fn new(style_source: Source) -> Self {
        RenderImageSource {
            base: RenderSourceBase::new(style_source),
            bucket: None,
            tile_ids: Vec::new(),
            mesh_corners: GeometryCoordinates::new(),
            bounds: [0; 4],
            quad: ImageQuad::TILE,
            mesh_cells: 0,
        }
    }

    /// Whether the source's image has arrived. A source without an image
    /// (one made without a url) has nothing to load, so a still render of it
    /// finishes.
    fn loaded(&self) -> bool {
        self.bucket.is_some() || self.data().is_none_or(|data| data.description.is_none())
    }

    fn data(&self) -> Option<&SourceData<ImageSourceType>> {
        SourceData::recover(&self.base.style_source)
    }
}

impl RenderSource for RenderImageSource {
    fn base(&self) -> &RenderSourceBase {
        &self.base
    }

    // A source that draws nowhere holds no frame back: the style tracks
    // the load of its image.
    fn is_loaded(&self, _store: &TileStore) -> bool {
        !self.base.enabled || self.loaded()
    }

    fn update(
        &mut self,
        _store: &mut TileStore,
        style_source: Source,
        _layers: &[Arc<dyn ErasedEvaluatedLayer>],
        needs_rendering: bool,
        _needs_relayout: bool,
        parameters: &TileParameters<'_>,
    ) {
        self.base.enabled = needs_rendering;
        if !needs_rendering {
            return;
        }

        let view = parameters.view;
        self.base.style_source = style_source;

        let Some(data) = self.data() else {
            self.base.enabled = false;
            return;
        };
        let coords = data.options.coordinates;
        let image = data.description.clone();

        let Some(image) = image.filter(|image| image.valid()) else {
            self.base.enabled = false;
            return;
        };

        // Compute the z0 tile coordinates for the given LatLngs
        let mut ne_point: TileCoordinatePoint = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        let mut sw_point: TileCoordinatePoint = Point::new(f64::INFINITY, f64::INFINITY);
        let mut tile_coordinates: Vec<TileCoordinatePoint> = Vec::new();
        for lat_lng in &coords {
            let point = TileCoordinate::from_lat_lng(0.0, lat_lng).p;
            tile_coordinates.push(point);
            sw_point.x = if point.x < sw_point.x { point.x } else { sw_point.x };
            ne_point.x = if ne_point.x < point.x { point.x } else { ne_point.x };
            sw_point.y = if point.y < sw_point.y { point.y } else { sw_point.y };
            ne_point.y = if ne_point.y < point.y { point.y } else { ne_point.y };
        }

        // Calculate the optimum zoom level to determine the tile ids to use
        // for transforms
        let dx = ne_point.x - sw_point.x;
        let dy = ne_point.y - sw_point.y;
        let d_max = if dx < dy { dy } else { dx };
        let floor = (-d_max.log2()).floor();
        let zoom = (if 0.0 < floor { floor } else { 0.0 }) as u8;

        // Only enable if the long side of the image is > 2 pixels. Resulting
        // in a display of at least 2 x 1 px image
        self.base.enabled = d_max * 2.0f64.powf(view.zoom()) * constants::TILE_SIZE_D > 2.0;
        if !self.base.enabled {
            return;
        }

        let mut image_bounds = LatLngBounds::hull(coords[0], coords[1]);
        image_bounds.extend(&coords[2]);
        image_bounds.extend(&coords[3]);
        let tile_cover_ids = tile_cover_bounds(&image_bounds, zoom);
        self.tile_ids.clear();
        // The cover is empty, for example, when the image lies wholly outside
        // the latitudes a Web Mercator map can show (beyond about ±85.05°).
        if tile_cover_ids.is_empty() {
            self.base.enabled = false;
            return;
        }
        self.tile_ids.push(tile_cover_ids[0]);

        let mut has_visible_tile = false;
        // A wrap does not move a tile on the globe, so the image draws once
        // there, in the world copy the cover chose.
        let globe = view.is_globe();
        // The image draws in each world copy whose quad overlaps the view's
        // tiles: while terrain draws, the terrain's, raised ground's too, as
        // every source covers the view then.
        let ideal_tiles = match parameters.terrain {
            Some(terrain) => terrain.tiles.iter().map(|tile| tile.id).collect(),
            None => tile_cover(
                &TileCoverParameters {
                    view: view.clone(),
                    tile_lod_min_radius: parameters.tile_lod_min_radius,
                    tile_lod_scale: parameters.tile_lod_scale,
                    tile_lod_pitch_threshold: parameters.tile_lod_pitch_threshold,
                    tile_lod_mode: parameters.tile_lod_mode,
                    requested_zoom: None,
                    round_zoom: false,
                    max_content_elevation: 0.0,
                },
                view.integer_zoom(),
                Range::new(0u8, zoom),
                None,
            ),
        };
        for tile in &ideal_tiles {
            for covering_tile in &tile_cover_ids {
                if covering_tile.canonical == tile.canonical
                    || covering_tile.canonical.is_child_of(&tile.canonical)
                    || tile.canonical.is_child_of(&covering_tile.canonical)
                {
                    // The copy that puts this covering tile on the view's
                    // tile: where the quad crosses the antimeridian, the
                    // covering tile may lie in the world next to the
                    // first's, and one view tile may see two copies. A
                    // copy too many worlds from the first has no tile id.
                    let wrap = i32::from(tile.wrap) - i32::from(covering_tile.wrap) + i32::from(tile_cover_ids[0].wrap);
                    let Ok(wrap) = i16::try_from(wrap) else {
                        continue;
                    };
                    has_visible_tile = true;
                    let wrapped_tile_id = UnwrappedTileID::with_wrap(wrap, tile_cover_ids[0].canonical);
                    if globe {
                        self.tile_ids = vec![wrapped_tile_id];
                        break;
                    }
                    if !self.tile_ids.contains(&wrapped_tile_id) {
                        self.tile_ids.push(wrapped_tile_id);
                    }
                }
            }
            if globe && has_visible_tile {
                break;
            }
        }

        self.base.enabled = has_visible_tile;
        if !self.base.enabled {
            return;
        }

        // The corners in the units of the cover's first tile, rounded to the
        // nearest unit as GL JS rounds them, so that an edge falls on the
        // pixels GL JS draws it on.
        let tile = tile_cover_ids[0];
        let scale = 2.0f64.powi(i32::from(tile.canonical.z));
        let extent = f64::from(constants::EXTENT);
        let geom_coords: GeometryCoordinates = tile_coordinates
            .iter()
            .map(|point| {
                let x = point.x * scale - f64::from(tile.canonical.x) - f64::from(tile.wrap) * scale;
                let y = point.y * scale - f64::from(tile.canonical.y);
                Point::new(to_vertex_coordinate(x * extent), to_vertex_coordinate(y * extent))
            })
            .collect();

        // A changed image gets a new bucket, whose id tells the layers to
        // make its texture.
        let bucket = match &mut self.bucket {
            Some(bucket) if Arc::ptr_eq(&bucket.image, &image) => bucket,
            slot => slot.insert(RasterBucket::new(image)),
        };

        // The globe bends the quad over the sphere with the tile mesh
        // granularity of its tile. Two triangles draw a parallelogram's
        // affine warp and a projective warp exactly; one blended towards the
        // bilinear warp is neither, and would bend along their diagonal, so
        // it draws on a grid. A concave quad's bilinear warp folds over
        // itself, out of the quad: it draws on two triangles split along the
        // diagonal inside it.
        let warp = image_warp(&geom_coords);
        let concave = concave_corner(&geom_coords);
        let cells = if view.is_globe() {
            SubdivisionGranularitySetting::globe().tile.granularity_for_zoom_level(self.tile_ids[0].canonical.z)
        } else if warp.blend > 0.0 && !is_parallelogram(&geom_coords) && concave.is_none() {
            BLENDED_WARP_CELLS
        } else {
            1
        };
        if !bucket.has_mesh() || geom_coords != self.mesh_corners || cells != self.mesh_cells {
            build_image_mesh(bucket, cells, matches!(concave, Some(0 | 2)));
            self.quad = ImageQuad::new(&geom_coords, &warp);
            let mut bounds = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
            for corner in &geom_coords {
                let (x, y) = (i32::from(corner.x), i32::from(corner.y));
                bounds = [bounds[0].min(x), bounds[1].min(y), bounds[2].max(x), bounds[3].max(y)];
            }
            self.bounds = bounds;
            self.mesh_corners = geom_coords;
            self.mesh_cells = cells;
        }
    }

    fn update_render_tiles(&mut self, _store: &mut TileStore, _parameters: &RenderTileParameters<'_>) {}

    fn upload(&mut self, _store: &mut TileStore, context: &mut Context, _needs: UploadNeeds) {
        if let Some(bucket) = &mut self.bucket {
            bucket.upload(context);
        }
    }

    fn image_render_data(&self) -> Option<ImageSourceRenderData<'_>> {
        // A disabled source's tile ids and corners may be of other
        // coordinates, and an image shrunk out of sight or out of view draws
        // nowhere.
        let tile_ids: &[UnwrappedTileID] = if self.base.enabled && self.loaded() { &self.tile_ids } else { &[] };
        Some(ImageSourceRenderData { bucket: self.bucket.as_ref(), tile_ids, bounds: self.bounds, quad: self.quad })
    }

    fn dump_debug_logs(&self) {
        log::info!(
            target: log_target::GENERAL,
            "Image source {}: loaded {}",
            self.base.style_source.id(),
            self.loaded()
        );
    }

    fn max_zoom(&self) -> Option<u8> {
        None
    }
}
