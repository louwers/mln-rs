//! Resource loading: `Resource`, `Response`, the `FileSource` trait, the
//! [`Loader`] over the built-in file sources (assets, local files, MBTiles,
//! PMTiles, the cache database and the network), `OfflineDatabase` and the
//! offline region definitions. `OfflineDownload` is in `offline`.
//!
//! The crate's `sqlite` feature (on by default) holds everything that needs SQLite:
//! `OfflineDatabase`, the database file source and the MBTiles file source.
//! Without it a `Loader` has no cache (examples/web relies on the browser's
//! HTTP cache instead).

#[cfg(target_os = "android")]
pub mod android;
mod archive_tilejson;
mod asset_file_source;
pub mod chrono;
pub mod compression;
pub mod constants;
#[cfg(feature = "sqlite")]
pub mod database_file_source;
#[cfg(target_family = "wasm")]
mod fetch_transport;
pub mod file_source;
mod http_file_source;
pub mod http_header;
pub mod http_header_transform;
mod http_timeout;
#[cfg(target_os = "android")]
mod http_url_connection_transport;
mod loader;
mod local_file_request;
mod local_file_source;
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
#[cfg(not(any(target_family = "wasm", target_vendor = "apple")))]
mod redirect;
pub mod request;
pub mod resource;
mod resource_options;
pub mod resource_transform;
pub mod response;
#[cfg(all(any(test, feature = "bench"), not(target_family = "wasm")))]
#[doc(hidden)]
pub mod simulated_network;
#[cfg(any(test, feature = "stub-file-source"))]
pub mod stub_file_source;
pub mod token;
#[cfg(not(any(target_family = "wasm", target_vendor = "apple", target_os = "android")))]
mod ureq_transport;

#[cfg(feature = "sqlite")]
pub use database_file_source::{Cache, DatabaseFileSource};
pub use file_source::{FileSource, FileSourceExt};
pub use loader::{Loader, LoaderBuilder};
#[cfg(feature = "sqlite")]
pub use offline_database::OfflineDatabase;
pub use online_file_source::OnlineFileSource;
pub use request::{Callback, Request, Responder};
pub use resource::{Kind, LoadingMethod, RequestOrder, Resource, StoragePolicy, TileFormat};
pub use resource_options::ResourceOptions;
pub use response::{Data, ResourceError, Response};

#[cfg(test)]
mod tests;
