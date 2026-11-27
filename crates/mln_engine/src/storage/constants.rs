//! The storage's defaults and URL protocols.

use std::time::Duration;

pub const DEFAULT_MAX_CACHE_SIZE: u64 = 50 * 1024 * 1024;

pub(crate) const DEFAULT_RATE_LIMIT_TIMEOUT: Duration = Duration::from_secs(5);

pub const ASSET_PROTOCOL: &str = "asset://";
pub(crate) const FILE_PROTOCOL: &str = "file://";
#[cfg(feature = "sqlite")]
pub(crate) const MBTILES_PROTOCOL: &str = "mbtiles://";
pub(crate) const PMTILES_PROTOCOL: &str = "pmtiles://";
pub(crate) const DEFAULT_MAXIMUM_CONCURRENT_REQUESTS: u32 = 20;
