//! Offline regions: `OfflineDownload`, which downloads an offline region's
//! resources into the `OfflineDatabase`, `Downloads`, which keeps a
//! database's downloads beside it on the database's worker, and the offline
//! region API of that database (`RegionDatabase`).

mod database_file_source;
mod offline_download;

pub use database_file_source::{DatabaseFileSourceExt, RegionDatabase, region_database};

#[cfg(test)]
mod tests;
