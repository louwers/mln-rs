use std::time::Duration;

use crate::unitbezier::UnitBezier;

pub const TILE_SIZE_D: f64 = 512.0;
pub const TILE_SIZE_I: u16 = 512;
/// The seed's name for `tileSize_D`.
pub const TILE_SIZE: f64 = TILE_SIZE_D;

/// The maximum extent of a feature that can be safely stored in the buffer.
/// In practice, all features are converted to this extent before being added.
///
/// Positions are stored as signed 16bit integers.
/// One bit is lost for signedness to support features extending past the left
/// edge of the tile. One bit is lost because the line vertex buffer used to pack
/// 1 bit of other data into the int. This is no longer the case but we're
/// reserving this bit anyway. One bit is lost to support features extending past
/// the extent on the right edge of the tile. This leaves us with 2^13 = 8192
pub const EXTENT: i32 = 8192;

pub const M2PI: f64 = std::f64::consts::PI * 2.0;
pub const EARTH_RADIUS_M: f64 = 6_378_137.0;
pub const LATITUDE_MAX: f64 = 85.051_128_779_806_6;
pub const LONGITUDE_MAX: f64 = 180.0;
pub const DEGREES_MAX: f64 = 360.0;
pub const PITCH_MIN: f64 = 0.0;
pub const DEFAULT_PITCH_MAX: f64 = std::f64::consts::PI / 3.0;
pub const PITCH_MAX: f64 = std::f64::consts::PI;
/// This fov value is somewhat arbitrary. The altitude of the camera used
/// to be defined as 1.5 screen heights above the ground, which was an
/// arbitrary choice. This is the fov equivalent to that value calculated
/// with: `fov = 2 * arctan((height / 2) / (height * 1.5))`
pub const DEFAULT_FOV: f64 = 0.643_501_108_793_284_4;
pub const MIN_ZOOM: f64 = 0.0;
pub const MAX_ZOOM: f64 = 25.5;
pub const MIN_ZOOM_F: f32 = MIN_ZOOM as f32;
pub const MAX_ZOOM_F: f32 = MAX_ZOOM as f32;
pub const DEFAULT_MAX_ZOOM: u8 = 22;

/// `ONE_EM` constant used to go between "em" units used in style spec and
/// "points" used internally for layout.
pub const ONE_EM: f32 = 24.0;

pub const DEFAULT_PREFETCH_ZOOM_DELTA: u8 = 4;

pub const DEFAULT_MAX_CACHE_SIZE: u64 = 50 * 1024 * 1024;

/// Default `ImageManager`'s cache size for images added via
/// onStyleImageMissing API. Average sprite size with 1.0 pixel ratio is
/// ~2kB, 8kB for pixel ratio of 2.0.
pub const DEFAULT_ON_DEMAND_IMAGES_CACHE_SIZE: usize = 100 * 8192;

pub const DEFAULT_TRANSITION_DURATION: Duration = Duration::from_millis(300);
pub const CLOCK_SKEW_RETRY_TIMEOUT: Duration = Duration::from_secs(30);

pub const DEFAULT_TRANSITION_EASE: UnitBezier = UnitBezier::new(0.0, 0.0, 0.25, 1.0);

pub const DEFAULT_RATE_LIMIT_TIMEOUT: Duration = Duration::from_secs(5);

pub const ASSET_PROTOCOL: &str = "asset://";
pub const FILE_PROTOCOL: &str = "file://";
pub const MBTILES_PROTOCOL: &str = "mbtiles://";
pub const PMTILES_PROTOCOL: &str = "pmtiles://";
pub const DEFAULT_MAXIMUM_CONCURRENT_REQUESTS: u32 = 20;

pub const TERRAIN_RGB_MAXZOOM: u8 = 15;

pub const LAST_RESORT_ALPHABETIC_FONT: &str = "Open Sans Regular";
pub const LAST_RESORT_PAN_UNICODE_FONT: &str = "Arial Unicode MS Regular";

/// Debug warning switches; the ones that are on are on in debug builds only.
pub mod debug {
    pub const TILE_PARSE_WARNINGS: bool = false;
    pub const STYLE_PARSE_WARNINGS: bool = false;
    pub const SPRITE_WARNINGS: bool = false;
    pub const RENDER_WARNINGS: bool = false;
    pub const LABEL_TEXT_MISSING_WARNING: bool = cfg!(debug_assertions);
    pub const MISSING_FONT_STACK_WARNING: bool = cfg!(debug_assertions);
    pub const MISSING_FONT_FACE_WARNING: bool = cfg!(debug_assertions);
    pub const GLYPH_WARNING: bool = cfg!(debug_assertions);
    pub const SHAPING_WARNING: bool = cfg!(debug_assertions);
}
