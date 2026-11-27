//! Offline regions: `OfflineDownload`, which downloads an offline region's
//! resources into the `OfflineDatabase`, `Downloads`, which keeps a
//! database's downloads beside it on the database's worker, and the offline
//! region API of that database (`RegionDatabase`).

mod database_file_source;
mod offline_download;

pub use database_file_source::{DatabaseFileSourceExt, RegionDatabase, region_database};
// Public for the crate's test of a download that a map renders from
// (tests/offline_download_map.rs). Not API.
#[doc(hidden)]
pub use offline_download::{DownloadTask, Downloads, OfflineDownload, Post};

#[cfg(test)]
mod tests;
