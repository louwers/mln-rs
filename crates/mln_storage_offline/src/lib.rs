//! Ports `platform/default/src/mln/storage/offline_download.cpp`
//! (`OfflineDownload`, which downloads an offline region's resources into
//! the `OfflineDatabase`), the offline region API of `DatabaseFileSource` that
//! drives it, and the canonicalize* half of src/mln/util/mapbox.cpp it uses.
//! It parses styles and covers regions with tiles, so it sits above `mln_style`
//! and `mln_renderer`.

pub mod database_file_source;
pub mod mapbox;
pub mod offline_download;

pub use database_file_source::DatabaseFileSourceExt;
pub use offline_download::OfflineDownload;

#[cfg(test)]
mod tests;
