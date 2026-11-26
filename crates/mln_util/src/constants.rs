//! The engine's constants: the camera's limits, the caches' sizes, the
//! URL protocols and the layout's em.

use std::time::Duration;

pub const M2PI: f64 = std::f64::consts::PI * 2.0;
pub const EARTH_RADIUS_M: f64 = 6_378_137.0;
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

/// `ONE_EM` constant used to go between "em" units used in style spec and
/// "points" used internally for layout.
pub const ONE_EM: f32 = 24.0;

pub const DEFAULT_MAX_CACHE_SIZE: u64 = 50 * 1024 * 1024;

/// Default `ImageManager`'s cache size for images added via
/// onStyleImageMissing API. Average sprite size with 1.0 pixel ratio is
/// ~2kB, 8kB for pixel ratio of 2.0.
pub const DEFAULT_ON_DEMAND_IMAGES_CACHE_SIZE: usize = 100 * 8192;

pub const DEFAULT_RATE_LIMIT_TIMEOUT: Duration = Duration::from_secs(5);

pub const ASSET_PROTOCOL: &str = "asset://";
pub const FILE_PROTOCOL: &str = "file://";
pub const MBTILES_PROTOCOL: &str = "mbtiles://";
pub const PMTILES_PROTOCOL: &str = "pmtiles://";
pub const DEFAULT_MAXIMUM_CONCURRENT_REQUESTS: u32 = 20;

pub const TERRAIN_RGB_MAXZOOM: u8 = 15;

/// Debug warning switches; the ones that are on are on in debug builds only.
pub(crate) mod debug {}
