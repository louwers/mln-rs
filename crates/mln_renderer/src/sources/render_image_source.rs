// Ports src/mln/renderer/sources/render_image_source.hpp and
// src/mln/renderer/sources/render_image_source.cpp (conformance).
//
// C++ moves the `ImageSourceRenderData` into the render tree in
// `createRenderItem` and the raster layers keep a raw pointer to it from
// `prepare`; here it is an `Rc` both hold.
//
// TODO(port): queryRenderedFeatures / querySourceFeatures (both empty in
// C++).

use std::any::Any;
use std::rc::Rc;
use std::sync::Arc;

use geometry::Point;
use mln_gfx::UploadPass;
use mln_gfx::shaders::segment::SegmentBase;
use mln_map_types::TileCoordinate;
use mln_map_types::tile_coordinate::TileCoordinatePoint;
use mln_style::layer_properties::LayerProperties;
use mln_style::source::{self, static_immutable_cast};
use mln_style::sources::image_source;
use mln_tile_id::{GeometryCoordinate, GeometryCoordinates, UnwrappedTileID};
use mln_util::constants;
use mln_util::geo::LatLngBounds;
use mln_util::immutable::Immutable;
use mln_util::interpolate::Interpolate;
use mln_util::logging::{Event, Log};
use mln_util::range::Range;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;

use crate::bucket::Bucket;
use crate::buckets::raster_bucket::RasterBucket;
use crate::render_pass::RenderPass;
use crate::render_source::{RenderSource, RenderSourceBase, SourcePrepareParameters};
use crate::render_tree::RenderItem;
use crate::tile_parameters::TileParameters;
use crate::util::tile_cover::{TileCoverParameters, tile_cover, tile_cover_bounds};

// The mapping from the unit square onto the image's corners: the perspective
// terms of the projective warp and how far it is blended towards the
// bilinear one. Port of GL JS `calculateImageWarp` for the "auto" warp,
// after Heckbert, "Fundamentals of Texture Mapping and Image Warping"
// (1989), 2.2.3 and A.2.
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
    // std::max_element / std::min_element: the first largest and smallest.
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
    // std::max(0.0, x)
    let blend_term = (1.0 - PROJECTIVE_FORESHORTENING / foreshortening)
        / (1.0 - PROJECTIVE_FORESHORTENING / BILINEAR_FORESHORTENING);
    let blend = if 0.0 < blend_term { blend_term } else { 0.0 };
    if !foreshortening.is_finite() || foreshortening < 1.0 || foreshortening > BILINEAR_FORESHORTENING || blend >= 1.0 {
        return ImageWarp::default();
    }
    ImageWarp { perspective_x, perspective_y, blend }
}

// Where the unit-square point (u, v) lands on the corners; the same blend
// GL JS's raster vertex shader evaluates per vertex.
fn warp_position(corners: &GeometryCoordinates, warp: &ImageWarp, u: f64, v: f64) -> Point<f64> {
    let point = |p: GeometryCoordinate| Point::new(f64::from(p.x), f64::from(p.y));
    let top_left = point(corners[0]);
    let top_right = point(corners[1]);
    let bottom_right = point(corners[2]);
    let bottom_left = point(corners[3]);
    let bilinear = top_left.interpolate(&top_right, u).interpolate(&bottom_left.interpolate(&bottom_right, u), v);
    let denominator = warp.perspective_x * u + warp.perspective_y * v + 1.0;
    // The point arithmetic of mapbox::geometry, a coordinate at a time.
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
fn build_image_mesh(bucket: &RasterBucket, corners: &GeometryCoordinates, cells: u32) {
    let warp = image_warp(corners);
    let rows = cells + 1;
    let mut vertices = Vec::with_capacity((rows * rows) as usize);
    for j in 0..rows {
        for i in 0..rows {
            let u = f64::from(i) / f64::from(cells);
            let v = f64::from(j) / f64::from(cells);
            let position = warp_position(corners, &warp, u, v);
            // std::lround
            vertices.push(RasterBucket::layout_vertex(
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

/// `ImageSourceRenderData`.
pub struct ImageSourceRenderData {
    pub bucket: Option<Arc<RasterBucket>>,
    pub tile_ids: Vec<UnwrappedTileID>,
    name: String,
}

impl ImageSourceRenderData {
    pub fn new(bucket: Option<Arc<RasterBucket>>, tile_ids: Vec<UnwrappedTileID>, name: String) -> Self {
        ImageSourceRenderData { bucket, tile_ids, name }
    }
}

impl RenderItem for ImageSourceRenderData {
    fn upload(&self, upload_pass: &dyn UploadPass) {
        if let Some(bucket) = &self.bucket
            && bucket.needs_upload()
        {
            bucket.upload(upload_pass);
        }
    }

    fn has_render_pass(&self, _pass: RenderPass) -> bool {
        false
    }

    fn get_name(&self) -> &str {
        &self.name
    }
}

/// The render tree's handle on the source's render data.
struct ImageSourceRenderItem(Rc<ImageSourceRenderData>);

impl RenderItem for ImageSourceRenderItem {
    fn upload(&self, upload_pass: &dyn UploadPass) {
        self.0.upload(upload_pass);
    }

    fn has_render_pass(&self, pass: RenderPass) -> bool {
        self.0.has_render_pass(pass)
    }

    fn get_name(&self) -> &str {
        self.0.get_name()
    }
}

/// `RenderImageSource`.
pub struct RenderImageSource {
    base: RenderSourceBase,
    bucket: Option<Arc<RasterBucket>>,
    render_data: Option<Rc<ImageSourceRenderData>>,
    tile_ids: Vec<UnwrappedTileID>,
    mesh_corners: GeometryCoordinates,
    mesh_cells: u32,
}

impl RenderImageSource {
    pub fn new(impl_: Arc<image_source::Impl>) -> Self {
        let impl_: Immutable<dyn source::Impl> = impl_;
        RenderImageSource {
            base: RenderSourceBase::new(impl_),
            bucket: None,
            render_data: None,
            tile_ids: Vec::new(),
            mesh_corners: GeometryCoordinates::new(),
            mesh_cells: 0,
        }
    }

    fn impl_(&self) -> Arc<image_source::Impl> {
        static_immutable_cast::<image_source::Impl>(&self.base.base_impl)
    }
}

impl RenderSource for RenderImageSource {
    fn base(&self) -> &RenderSourceBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut RenderSourceBase {
        &mut self.base
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn is_loaded(&self) -> bool {
        // Unlike C++, a source without an image (one made without a url) has
        // nothing to load, so a still render of it finishes.
        self.bucket.is_some() || self.impl_().get_image().is_none()
    }

    fn update(
        &mut self,
        base_impl: Immutable<dyn source::Impl>,
        _layers: &[Immutable<dyn LayerProperties>],
        needs_rendering: bool,
        _needs_relayout: bool,
        parameters: &TileParameters<'_>,
    ) {
        self.base.enabled = needs_rendering;
        if !needs_rendering {
            return;
        }

        let transform_state = parameters.transform_state;
        self.base.base_impl = base_impl;

        let impl_ = self.impl_();
        let coords = impl_.get_coordinates();
        let image = impl_.get_image();

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
            // std::min / std::max
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
        let floor = (-mln_math::log2(d_max)).floor();
        let zoom = (if 0.0 < floor { floor } else { 0.0 }) as u8;

        // Only enable if the long side of the image is > 2 pixels. Resulting
        // in a display of at least 2 x 1 px image
        self.base.enabled = d_max * 2.0f64.powf(transform_state.get_zoom()) * constants::TILE_SIZE_D > 2.0;
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
        let globe = transform_state.is_globe_rendering();
        // Add additional wrapped tile ids if necessary
        let zoom_range = Range::new(0u8, zoom);
        let ideal_tiles = tile_cover(
            &TileCoverParameters {
                transform_state: transform_state.clone(),
                tile_lod_min_radius: parameters.tile_lod_min_radius,
                tile_lod_scale: parameters.tile_lod_scale,
                tile_lod_pitch_threshold: parameters.tile_lod_pitch_threshold,
                tile_lod_mode: parameters.tile_lod_mode,
                requested_zoom: None,
                round_zoom: false,
            },
            transform_state.get_integer_zoom(),
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

        // C++ swaps the image of its bucket in place; a changed image gets a
        // new bucket here (buckets are shared across threads).
        let same_image =
            self.bucket.as_ref().is_some_and(|bucket| bucket.image.as_ref().is_some_and(|i| Arc::ptr_eq(i, &image)));
        if !same_image {
            self.bucket = Some(Arc::new(RasterBucket::with_shared_image(Some(image))));
        }
        let bucket = self.bucket.as_ref().expect("bucket");

        // The globe bends the quad over the sphere with the tile mesh
        // granularity of its tile, like GL JS.
        let cells = if transform_state.is_globe_rendering() {
            SubdivisionGranularitySetting::globe().tile.get_granularity_for_zoom_level(self.tile_ids[0].canonical.z)
        } else {
            1
        };
        if bucket.geometry().vertices.is_empty() || geom_coords != self.mesh_corners || cells != self.mesh_cells {
            bucket.clear();
            build_image_mesh(bucket, &geom_coords, cells);
            self.mesh_corners = geom_coords;
            self.mesh_cells = cells;
        }
    }

    fn create_render_item(&mut self) -> Box<dyn RenderItem> {
        let render_data = self.render_data.take().expect("RenderImageSource::prepare");
        Box::new(ImageSourceRenderItem(render_data))
    }

    fn prepare(&mut self, _parameters: &SourcePrepareParameters<'_>) {
        self.render_data = Some(Rc::new(ImageSourceRenderData::new(
            self.bucket.clone(),
            if self.is_loaded() { self.tile_ids.clone() } else { Vec::new() },
            self.base.base_impl.id.clone(),
        )));
    }

    fn get_image_render_data(&self) -> Option<Rc<ImageSourceRenderData>> {
        self.render_data.clone()
    }

    fn update_fading_tiles(&mut self) {}

    fn has_fading_tiles(&self) -> bool {
        false
    }

    fn reduce_memory_use(&mut self) {}

    fn dump_debug_logs(&self) {
        Log::info(Event::General, format!("RenderImageSource::id: {}", self.base.base_impl.id));
        Log::info(
            Event::General,
            format!("RenderImageSource::loaded: {}", if self.is_loaded() { "yes" } else { "no" }),
        );
    }
}
