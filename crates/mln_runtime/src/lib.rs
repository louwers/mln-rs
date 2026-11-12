//! Thread-safe handles over mln-rs's maps, which mirror
//! maplibre-native-ffi's C API one to one.
//!
//! A [`Runtime`] owns a thread, and the file sources, cache and offline
//! regions of its maps. Each [`Map`] lives on its runtime's thread, and its
//! handle sends commands there: they run in the order sent, across the
//! runtime and all its maps, and report through a [`Completion`]. A map
//! publishes a [`MapSnapshot`] after each command, which any thread reads
//! without waiting, and the events of the runtime and its maps reach one
//! handler in batches. A [`RenderSession`] that [`Map::attach`] creates
//! draws a map's frames offscreen or onto a surface, on a thread of its own,
//! or on the host's graphics thread through its [`DriverWork`].
//!
//! The synchronous, single-thread core that these handles drive is
//! `mln_map`'s; [`Map::with_core`] runs a closure on it.
//!
//! Sessions render with the backends of the crate's features: `webgpu`,
//! `metal`, `vulkan` and `gl`, each where its platform has it. A session's
//! [`Device`] picks its backend.

#[cfg(not(any(feature = "webgpu", feature = "metal", feature = "vulkan", feature = "gl")))]
compile_error!("mln_runtime renders with at least one backend: enable `webgpu`, `metal`, `vulkan` or `gl`");

mod camera;
mod completion;
mod event;
mod feature_state;
mod map;
mod offline;
mod render;
mod resources;
mod runtime;
mod style;

pub use camera::{CameraFitArea, CameraFitOptions, CameraFitUpdate, TileOptions, ViewportOptions};
pub use completion::Completion;
pub use event::{EventHandler, EventMask, EventSource, EventType, RuntimeEvent, RuntimeEventKind};
pub use feature_state::{FeatureState, FeatureStateSelector};
pub use map::{Applied, CameraQuery, CommandGroup, Map, MapId, MapOptions};
pub use mln_map::{LogicalExtent, MapSnapshot};
pub use mln_renderer::query::{RenderedQueryOptions, SourceQueryOptions};
pub use mln_util::{Error, ErrorKind};
pub use offline::Offline;
pub use render::{
    AcquiredFrame, Adapter, AttachOptions, BackendType, BorrowedRing, ConsumerSync, DemandMode, Device, DeviceOptions,
    DmaBufFormat, DmaBufPlane, Driver, DriverWork, ExportKind, ExportKinds, ExportedImage, ExportedRing, FrameDemand,
    FrameDisposition, FrameResult, FrameTexture, FrameTextureKind, FrameView, GpuSync, LocalTarget, RenderSession,
    RenderSessionCapabilities, RenderSessionSnapshot, RenderSessionState, RenderTarget, RenderedQueryGeometry, Surface,
    SurfaceType, SyncKind, SyncKinds, Wake,
};
pub use resources::{CacheDatabaseOperation, ResourceProvider, ResourceRequest, is_managed_http_header};
pub use runtime::{Runtime, RuntimeOptions};
pub use style::{CustomSourceCallbacks, GeoJsonSourceOptions, LayerInfo, SourceInfo, SourceKind, SourceTiles};
