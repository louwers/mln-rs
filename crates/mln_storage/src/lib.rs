//! Resource loading: `Resource`, `Response`, `FileSource`,
//! `FileSourceRequest`, `ResourceOptions`, `ResourceTransform`,
//! `FileSourceManager`, `MainResourceLoader`, the database, online/HTTP,
//! asset, local file, MBTiles and PMTiles file sources, `OfflineDatabase`
//! and the offline region definitions. `OfflineDownload` needs the style
//! parser and is in `mln_storage_offline`.
//!
//! The `sqlite` feature (on by default) holds everything that needs SQLite:
//! `OfflineDatabase`, the database file source and the MBTiles file source.
//! Without it the `ResourceLoader` has no ambient cache (examples/web relies
//! on the browser's HTTP cache instead).

#[cfg(target_os = "android")]
pub mod android;
mod asset_file_source;
mod async_request;
#[cfg(feature = "sqlite")]
pub mod database_file_source;
#[cfg(target_family = "wasm")]
mod fetch_transport;
pub mod file_source;
mod file_source_manager;
mod file_source_request;
mod http_file_source;
pub mod http_header_transform;
mod http_timeout;
mod json_document;
mod local_file_request;
mod local_file_source;
mod main_resource_loader;
#[cfg(feature = "sqlite")]
mod mbtiles_file_source;
#[cfg(feature = "sqlite")]
mod merge_sideloaded;
pub mod network_status;
#[cfg(target_vendor = "apple")]
mod nsurlsession_transport;
pub mod offline;
#[cfg(feature = "sqlite")]
pub mod offline_database;
#[cfg(feature = "sqlite")]
mod offline_schema;
pub mod online_file_source;
mod pmtiles_file_source;
pub mod resource;
mod resource_options;
pub mod resource_transform;
pub mod response;
#[cfg(feature = "sqlite")]
pub mod sqlite3;
#[cfg(any(test, feature = "stub-file-source"))]
pub mod stub_file_source;
#[cfg(not(any(target_family = "wasm", target_vendor = "apple")))]
mod ureq_transport;

pub use async_request::AsyncRequest;
#[cfg(feature = "sqlite")]
pub use database_file_source::DatabaseFileSource;
#[cfg(target_family = "wasm")]
pub use fetch_transport::set_fetch_preloads;
pub use file_source::{Callback, FileSource, FileSourceType, READ_ONLY_MODE_KEY};
pub use file_source_manager::FileSourceManager;
pub use main_resource_loader::MainResourceLoader;
#[cfg(feature = "sqlite")]
pub use offline_database::OfflineDatabase;
pub use online_file_source::OnlineFileSource;
pub use resource::{Kind, LoadingMethod, RequestOrder, Resource, StoragePolicy};
pub use resource_options::ResourceOptions;
pub use response::{Data, ResourceError, Response};

#[cfg(test)]
mod tests;
