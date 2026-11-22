// An image source draws one image, stretched over the quad of its
// coordinates, in the tiles that the quad covers. Its render tiles are
// those tiles, each of which shows the image, and the layers that draw it
// borrow its contents from the source for the frame.

use std::sync::Arc;

use geometry::Point;
use mln_gfx::shaders::segment::SegmentBase;
use mln_map_types::TileCoordinate;
use mln_map_types::tile_coordinate::TileCoordinatePoint;
use mln_map_types::tile_cover::{TileCoverParameters, tile_cover, tile_cover_bounds};
use mln_style::Source;
use mln_tile_id::{GeometryCoordinate, GeometryCoordinates, OverscaledTileID, UnwrappedTileID};
use mln_util::constants;
use mln_util::geo::LatLngBounds;
use mln_util::interpolate::Interpolate;
use mln_util::logging::{Event, Log};
use mln_util::range::Range;

use crate::erased_layer::ErasedEvaluatedLayer;
use crate::gfx::context::Resources;
use crate::globe::surface_granularity;
use crate::render_source::{RenderSource, RenderSourceBase, RenderTileParameters, TileOrder};
use crate::render_tile::RenderTile;
use crate::source::{ImageOf, SourceImage};
use crate::tile::raster::{RasterContents, RasterLayoutVertex};
use crate::tile::store::TileStore;
use crate::tile_parameters::TileParameters;

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
fn build_image_mesh(image: &mut RasterContents, corners: &GeometryCoordinates, cells: u32) {
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
    image.set_geometry(vertices, indices, segments);
}

pub(crate) struct RenderImageSource {
    base: RenderSourceBase,
    /// Reads the image of each version of the source.
    image_of: ImageOf,
    image: Option<RasterContents>,
    tile_ids: Vec<UnwrappedTileID>,
    /// The render tiles of `tile_ids`, and their indexes in order.
    render_tiles: Vec<RenderTile>,
    order: Vec<usize>,
    mesh_corners: GeometryCoordinates,
    mesh_cells: u32,
}

impl RenderImageSource {
    #[must_use]
    pub(crate) fn new(style_source: Source, image_of: ImageOf) -> Self {
        RenderImageSource {
            base: RenderSourceBase::new(style_source),
            image_of,
            image: None,
            tile_ids: Vec::new(),
            render_tiles: Vec::new(),
            order: Vec::new(),
            mesh_corners: GeometryCoordinates::new(),
            mesh_cells: 0,
        }
    }

    /// Whether the source's image has arrived. A source without an image
    /// (one made without a url) has nothing to load, so a still render of it
    /// finishes.
    fn loaded(&self) -> bool {
        self.image.is_some() || self.source_image().is_none_or(|source| source.image.is_none())
    }

    fn source_image(&self) -> Option<SourceImage<'_>> {
        (self.image_of)(&self.base.style_source)
    }

    /// Brings the source to `style_source` for a frame of `parameters`: the
    /// tiles the image is drawn in, and its image's mesh.
    fn place(&mut self, style_source: Source, needs_rendering: bool, parameters: &TileParameters<'_>) {
        self.base.enabled = needs_rendering;
        if !needs_rendering {
            return;
        }

        let view = parameters.view;
        self.base.style_source = style_source;

        let Some(source) = self.source_image() else {
            self.base.enabled = false;
            return;
        };
        let coords = source.coordinates;
        let image = source.image.cloned();

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
        // Add additional wrapped tile ids if necessary
        let zoom_range = Range::new(0u8, zoom);
        let ideal_tiles = tile_cover(
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
            zoom_range,
            None,
        );
        for tile in &ideal_tiles {
            for covering_tile in &tile_cover_ids {
                if covering_tile.canonical == tile.canonical
                    || covering_tile.canonical.is_child_of(&tile.canonical)
                    || tile.canonical.is_child_of(&covering_tile.canonical)
                {
                    has_visible_tile = true;
                    let wrapped_tile_id = UnwrappedTileID::with_wrap(tile.wrap, tile_cover_ids[0].canonical);
                    if globe {
                        self.tile_ids = vec![wrapped_tile_id];
                    } else if tile.wrap != 0 && !self.tile_ids.contains(&wrapped_tile_id) {
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

        // Calculate Geometry Coordinates based on tile cover at ideal zoom
        let geom_coords: GeometryCoordinates = tile_coordinates
            .iter()
            .map(|tile_coords| TileCoordinate::to_geometry_coordinate(&tile_cover_ids[0], tile_coords))
            .collect();

        // A changed image gets new contents, whose id tells the layers to
        // make its texture.
        let contents = match &mut self.image {
            Some(contents) if contents.image().is_some_and(|i| Arc::ptr_eq(i, &image)) => contents,
            slot => slot.insert(RasterContents::with_shared_image(Some(image))),
        };

        // The globe bends the quad over the sphere with the tile mesh
        // granularity of its tile.
        let cells = if view.is_globe() { surface_granularity(self.tile_ids[0].canonical.z) } else { 1 };
        if !contents.geometry().has_quads() || geom_coords != self.mesh_corners || cells != self.mesh_cells {
            build_image_mesh(contents, &geom_coords, cells);
            self.mesh_corners = geom_coords;
            self.mesh_cells = cells;
        }
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
        self.place(style_source, needs_rendering, parameters);
        // The image draws in the tiles it was last placed in, also once it
        // is too small to see.
        self.render_tiles.clear();
        self.render_tiles.extend(self.tile_ids.iter().map(|&id| {
            let tile_id = OverscaledTileID::new(id.canonical.z, id.wrap, id.canonical);
            RenderTile::of_tile_id(id, tile_id, false)
        }));
        self.order.clear();
        self.order.extend(0..self.render_tiles.len());
    }

    fn update_render_tiles(&mut self, _store: &mut TileStore, _parameters: &RenderTileParameters<'_>) {}

    fn upload(&mut self, _store: &mut TileStore, context: &mut Resources) {
        if let Some(image) = &mut self.image {
            image.upload(context);
        }
    }

    /// The tiles the image is drawn in, once it has arrived.
    fn render_tiles(&self) -> &[RenderTile] {
        if self.loaded() { &self.render_tiles } else { &[] }
    }

    fn tile_order(&self, _order: TileOrder) -> Option<&[usize]> {
        Some(&self.order[..self.render_tiles().len()])
    }

    fn image(&self) -> Option<&RasterContents> {
        self.image.as_ref()
    }

    fn is_image(&self) -> bool {
        true
    }

    fn dump_debug_logs(&self) {
        Log::info(Event::General, format!("Image source {}: loaded {}", self.base.style_source.id(), self.loaded()));
    }

    fn max_zoom(&self) -> Option<u8> {
        None
    }
}
