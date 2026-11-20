// An image source draws one image, stretched over the quad of its
// coordinates, in the tiles that the quad covers. The raster layers that
// draw it borrow its bucket from the source for the frame.

use std::sync::Arc;

use geometry::Point;
use mln_gfx::shaders::segment::SegmentBase;
use mln_map_types::TileCoordinate;
use mln_map_types::tile_coordinate::TileCoordinatePoint;
use mln_style::Source;
use mln_style::source_type::SourceData;
use mln_style::sources::ImageSourceType;
use mln_tile_id::{GeometryCoordinate, GeometryCoordinates, UnwrappedTileID};
use mln_util::constants;
use mln_util::geo::LatLngBounds;
use mln_util::interpolate::Interpolate;
use mln_util::logging::{Event, Log};
use mln_util::range::Range;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;

use crate::bucket::UploadNeeds;
use crate::buckets::raster_bucket::{RasterBucket, RasterLayoutVertex};
use crate::gfx::context::Context;
use crate::layer_type::ErasedEvaluatedLayer;
use crate::render_source::{RenderSource, RenderSourceBase, RenderTileParameters};
use crate::tile::store::TileStore;
use crate::tile_parameters::TileParameters;
use crate::util::tile_cover::{TileCoverParameters, tile_cover, tile_cover_bounds};

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

fn image_warp(corners: &GeometryCoordinates) -> ImageWarp {
    const PROJECTIVE_FORESHORTENING: f64 = 4.0;
    const BILINEAR_FORESHORTENING: f64 = 512.0;
    let top_left = corners[0];
    let top_right = corners[1];
    let bottom_right = corners[2];
    let bottom_left = corners[3];
    let (tl_x, tl_y) = (i32::from(top_left.x), i32::from(top_left.y));
    let (tr_x, tr_y) = (i32::from(top_right.x), i32::from(top_right.y));
    let (br_x, br_y) = (i32::from(bottom_right.x), i32::from(bottom_right.y));
    let (bl_x, bl_y) = (i32::from(bottom_left.x), i32::from(bottom_left.y));
    // A parallelogram is warped affinely either way.
    if tl_x + br_x == tr_x + bl_x && tl_y + br_y == tr_y + bl_y {
        return ImageWarp::default();
    }
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

// Where the unit-square point (u, v) lands on the corners: where a
// projective warp puts it, blended with where a bilinear one does by
// `warp.blend`.
fn warp_position(corners: &GeometryCoordinates, warp: &ImageWarp, u: f64, v: f64) -> Point<f64> {
    let point = |p: GeometryCoordinate| Point::new(f64::from(p.x), f64::from(p.y));
    let top_left = point(corners[0]);
    let top_right = point(corners[1]);
    let bottom_right = point(corners[2]);
    let bottom_left = point(corners[3]);
    let bilinear = top_left.interpolate(&top_right, u).interpolate(&bottom_left.interpolate(&bottom_right, u), v);
    let denominator = warp.perspective_x * u + warp.perspective_y * v + 1.0;
    // The homography's columns: the top edge and the left edge, each scaled
    // by the denominator's change along it.
    let across_top = Point::new(
        top_right.x - top_left.x + top_right.x * warp.perspective_x,
        top_right.y - top_left.y + top_right.y * warp.perspective_x,
    );
    let down_left = Point::new(
        bottom_left.x - top_left.x + bottom_left.x * warp.perspective_y,
        bottom_left.y - top_left.y + bottom_left.y * warp.perspective_y,
    );
    let projective = Point::new(
        (across_top.x * u + down_left.x * v + top_left.x) / denominator,
        (across_top.y * u + down_left.y * v + top_left.y) / denominator,
    );
    projective.interpolate(&bilinear, warp.blend)
}

fn to_vertex_coordinate(value: f64) -> i16 {
    mln_math::clamp_f64(value.round(), f64::from(i16::MIN), f64::from(i16::MAX)) as i16
}

// The image quad as a grid of `cells` x `cells`, textured edge to edge; one
// cell is the flat quad.
fn build_image_mesh(bucket: &mut RasterBucket, corners: &GeometryCoordinates, cells: u32) {
    let warp = image_warp(corners);
    let rows = cells + 1;
    let mut vertices = Vec::with_capacity((rows * rows) as usize);
    for j in 0..rows {
        for i in 0..rows {
            let u = f64::from(i) / f64::from(cells);
            let v = f64::from(j) / f64::from(cells);
            let position = warp_position(corners, &warp, u, v);
            vertices.push(RasterLayoutVertex::new(
                Point::new(to_vertex_coordinate(position.x), to_vertex_coordinate(position.y)),
                Point::new(
                    (u * f64::from(constants::EXTENT)).round() as u16,
                    (v * f64::from(constants::EXTENT)).round() as u16,
                ),
            ));
        }
    }
    let mut indices: Vec<u16> = Vec::with_capacity((cells * cells * 6) as usize);
    for j in 0..cells {
        for i in 0..cells {
            let top_left = (j * rows + i) as u16;
            let top_right = top_left.wrapping_add(1);
            let bottom_left = top_left.wrapping_add(rows as u16);
            let bottom_right = bottom_left.wrapping_add(1);
            indices.extend_from_slice(&[top_left, top_right, bottom_left]);
            indices.extend_from_slice(&[top_right, bottom_left, bottom_right]);
        }
    }
    let segments = vec![SegmentBase::with_lengths(0, 0, vertices.len(), indices.len())];
    bucket.set_geometry(vertices, indices, segments);
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
}

pub(crate) struct RenderImageSource {
    base: RenderSourceBase,
    bucket: Option<RasterBucket>,
    tile_ids: Vec<UnwrappedTileID>,
    mesh_corners: GeometryCoordinates,
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

    fn is_loaded(&self, _store: &TileStore) -> bool {
        self.loaded()
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
                    has_visible_tile = true;
                    // The copy that puts this covering tile on the view's
                    // tile: where the quad crosses the antimeridian, the
                    // covering tile may lie in the world next to the
                    // first's.
                    let wrap = tile.wrap - covering_tile.wrap + tile_cover_ids[0].wrap;
                    let wrapped_tile_id = UnwrappedTileID::with_wrap(wrap, tile_cover_ids[0].canonical);
                    if globe {
                        self.tile_ids = vec![wrapped_tile_id];
                    } else if !self.tile_ids.contains(&wrapped_tile_id) {
                        self.tile_ids.push(wrapped_tile_id);
                    }
                    break;
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
            Some(bucket) if bucket.image.as_ref().is_some_and(|i| Arc::ptr_eq(i, &image)) => bucket,
            slot => slot.insert(RasterBucket::with_shared_image(Some(image))),
        };

        // The globe bends the quad over the sphere with the tile mesh
        // granularity of its tile.
        let cells = if view.is_globe() {
            SubdivisionGranularitySetting::globe().tile.granularity_for_zoom_level(self.tile_ids[0].canonical.z)
        } else {
            1
        };
        if !bucket.geometry().has_quads() || geom_coords != self.mesh_corners || cells != self.mesh_cells {
            build_image_mesh(bucket, &geom_coords, cells);
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
        // nowhere: the drapes that showed it draw again.
        let tile_ids: &[UnwrappedTileID] = if self.base.enabled && self.loaded() { &self.tile_ids } else { &[] };
        let mut bounds = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
        for corner in &self.mesh_corners {
            let (x, y) = (i32::from(corner.x), i32::from(corner.y));
            bounds = [bounds[0].min(x), bounds[1].min(y), bounds[2].max(x), bounds[3].max(y)];
        }
        Some(ImageSourceRenderData { bucket: self.bucket.as_ref(), tile_ids, bounds })
    }

    fn dump_debug_logs(&self) {
        Log::info(Event::General, format!("Image source {}: loaded {}", self.base.style_source.id(), self.loaded()));
    }

    fn max_zoom(&self) -> Option<u8> {
        None
    }
}
