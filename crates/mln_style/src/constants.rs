//! The style spec's constants: the tile size and extent, the bounds of
//! longitude and latitude, and the defaults of sources, transitions and
//! fonts.

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

pub const LATITUDE_MAX: f64 = 85.051_128_779_806_6;
pub const LONGITUDE_MAX: f64 = 180.0;
pub const DEGREES_MAX: f64 = 360.0;

/// The highest zoom level of a source's tiles, unless it says otherwise.
pub const DEFAULT_MAX_ZOOM: u8 = 22;

pub const DEFAULT_PREFETCH_ZOOM_DELTA: u8 = 4;

pub const DEFAULT_TRANSITION_DURATION: Duration = Duration::from_millis(300);

pub const DEFAULT_TRANSITION_EASE: UnitBezier = UnitBezier::new(0.0, 0.0, 0.25, 1.0);

/// The fonts of `text-font`'s default.
pub const LAST_RESORT_ALPHABETIC_FONT: &str = "Open Sans Regular";
pub const LAST_RESORT_PAN_UNICODE_FONT: &str = "Arial Unicode MS Regular";
