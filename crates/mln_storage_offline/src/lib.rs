//! Ports platform/default/src/mln/storage/offline_download.cpp
//! (`OfflineDownload`, which downloads an offline region's resources into
//! the OfflineDatabase). It parses styles and covers regions with tiles, so
//! it sits above mln_style and mln_renderer.

pub mod mapbox;
pub mod offline_download;

pub use offline_download::OfflineDownload;

#[cfg(test)]
mod tests;
