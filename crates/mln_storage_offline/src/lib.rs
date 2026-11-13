//! Offline regions: `OfflineDownload`, which downloads an offline region's
//! resources into the `OfflineDatabase`, and the offline region API of
//! `DatabaseFileSource` that drives it. It parses styles and covers regions
//! with tiles, so it sits above `mln_style` and `mln_renderer`.

pub mod database_file_source;
pub mod offline_download;

pub use database_file_source::DatabaseFileSourceExt;
pub use offline_download::OfflineDownload;

#[cfg(test)]
mod tests;
