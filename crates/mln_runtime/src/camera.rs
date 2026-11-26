//! The options of the camera commands that the core map takes as separate
//! calls: fitting an area, the viewport and the tile settings.

use geo_types::Geometry;
use mln_map::{
    AnimationOptions, CameraOptions, CameraUpdateMode, ConstrainMode, NorthOrientation, TileLodMode, ViewportMode,
};
use mln_style::geometry::for_each_point;
use mln_util::error::Error;
use mln_util::geo::{EdgeInsets, LatLng, LatLngBounds};

/// An area for a camera to fit in the map's view.
#[derive(Clone, Debug, PartialEq)]
pub enum CameraFitArea {
    Bounds(LatLngBounds),
    /// At least one coordinate.
    Coordinates(Vec<LatLng>),
    /// A geometry with at least one coordinate, whose points are longitude
    /// `x` and latitude `y`.
    Geometry(Geometry<f64>),
}

/// How a camera fits an area.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CameraFitOptions {
    /// The space to leave between the area and each edge of the map. The
    /// left and right insets add up to less than the map's width, and the top
    /// and bottom to less than its height.
    pub padding: EdgeInsets,
    /// The rotation from north, in degrees. `None` keeps the map's.
    pub bearing: Option<f64>,
    /// The tilt toward the horizon, in degrees. `None` keeps the map's.
    pub pitch: Option<f64>,
}

/// Moves the camera to fit an area: [`CameraUpdate`](mln_map::CameraUpdate)'s
/// counterpart for [`Map::fit_camera`](crate::Map::fit_camera).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CameraFitUpdate {
    pub mode: CameraUpdateMode,
    pub fit: CameraFitOptions,
    pub animation: AnimationOptions,
}

/// How the map orients and constrains its view. A setting that is `None`
/// keeps its value.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ViewportOptions {
    pub north_orientation: Option<NorthOrientation>,
    pub constrain_mode: Option<ConstrainMode>,
    pub viewport_mode: Option<ViewportMode>,
    pub frustum_offset: Option<EdgeInsets>,
    /// As `mln_map::Viewport::center_clamped_to_ground`.
    pub center_clamped_to_ground: Option<bool>,
    /// As `mln_map::Viewport::globe_free_bearing`.
    pub globe_free_bearing: Option<bool>,
}

/// How the map picks the tiles to load, as its snapshot's
/// `TileSettings` report it. A setting that is
/// `None` keeps its value.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TileOptions {
    pub prefetch_zoom_delta: Option<u8>,
    pub lod_min_radius: Option<f64>,
    pub lod_scale: Option<f64>,
    /// In degrees.
    pub lod_pitch_threshold: Option<f64>,
    pub lod_zoom_shift: Option<f64>,
    pub lod_mode: Option<TileLodMode>,
}

/// The camera that fits `area` in `map`'s view. Fails with
/// [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for an empty area,
/// and for padding that leaves no room for it, for which the core would keep
/// the zoom.
pub(crate) fn camera_for(
    map: &mln_map::Map,
    area: &CameraFitArea,
    options: &CameraFitOptions,
) -> Result<CameraOptions, Error> {
    let size = map.transform_state().view().size();
    let padding = options.padding;
    if padding.left + padding.right >= f64::from(size.width) || padding.top + padding.bottom >= f64::from(size.height) {
        return Err(Error::invalid_argument("camera fit padding must be less than the map's width and height"));
    }
    let CameraFitOptions { padding, bearing, pitch } = *options;
    match area {
        CameraFitArea::Bounds(bounds) => map.camera_for_lat_lng_bounds(bounds, &padding, bearing, pitch),
        CameraFitArea::Coordinates(coordinates) if coordinates.is_empty() => {
            Err(Error::invalid_argument("coordinates must not be empty"))
        }
        CameraFitArea::Coordinates(coordinates) => map.camera_for_lat_lngs(coordinates, &padding, bearing, pitch),
        CameraFitArea::Geometry(geometry) => {
            let mut empty = true;
            for_each_point(geometry, &mut |_| empty = false);
            if empty {
                return Err(Error::invalid_argument("geometry must contain at least one coordinate"));
            }
            map.camera_for_geometry(geometry, &padding, bearing, pitch)
        }
    }
}

pub(crate) fn set_viewport(map: &mut mln_map::Map, options: &ViewportOptions) {
    let mut viewport = map.snapshot().view.viewport;
    viewport.north_orientation = options.north_orientation.unwrap_or(viewport.north_orientation);
    viewport.constrain_mode = options.constrain_mode.unwrap_or(viewport.constrain_mode);
    viewport.viewport_mode = options.viewport_mode.unwrap_or(viewport.viewport_mode);
    viewport.frustum_offset = options.frustum_offset.unwrap_or(viewport.frustum_offset);
    viewport.center_clamped_to_ground = options.center_clamped_to_ground.unwrap_or(viewport.center_clamped_to_ground);
    viewport.globe_free_bearing = options.globe_free_bearing.unwrap_or(viewport.globe_free_bearing);
    map.set_viewport(&viewport);
}

pub(crate) fn set_tile_options(map: &mut mln_map::Map, options: &TileOptions) {
    let mut tile = map.snapshot().tile;
    tile.prefetch_zoom_delta = options.prefetch_zoom_delta.unwrap_or(tile.prefetch_zoom_delta);
    tile.lod_min_radius = options.lod_min_radius.unwrap_or(tile.lod_min_radius);
    tile.lod_scale = options.lod_scale.unwrap_or(tile.lod_scale);
    tile.lod_pitch_threshold = options.lod_pitch_threshold.unwrap_or(tile.lod_pitch_threshold);
    tile.lod_zoom_shift = options.lod_zoom_shift.unwrap_or(tile.lod_zoom_shift);
    tile.lod_mode = options.lod_mode.unwrap_or(tile.lod_mode);
    map.set_tile_settings(&tile);
}
