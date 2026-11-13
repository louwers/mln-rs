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
//!
//! In the browser (wasm32-unknown-unknown, without threads), a runtime's
//! thread and its sessions' are the one that made the runtime: their work
//! runs in short tasks on that thread's event loop (`mln_web_host`'s pump),
//! never inside the call that sent it, and nothing blocks. Sessions present
//! to a canvas (`Surface::web_canvas`) with the browser's WebGPU or WebGL2,
//! and put their tiles' workers in Web Workers.

#[cfg(not(any(feature = "webgpu", feature = "metal", feature = "vulkan", feature = "gl")))]
compile_error!("mln_runtime renders with at least one backend: enable `webgpu`, `metal`, `vulkan` or `gl`");

mod camera;
mod completion;
mod event;
mod feature_state;
mod map;
#[cfg(not(target_family = "wasm"))]
mod offline;
#[cfg(target_family = "wasm")]
#[path = "offline_web.rs"]
mod offline;
mod render;
mod resources;
mod runtime;
mod style;
#[cfg(target_family = "wasm")]
mod web;

pub use camera::{CameraFitArea, CameraFitOptions, CameraFitUpdate, TileOptions, ViewportOptions};
pub use completion::Completion;
pub use event::{EventHandler, EventMask, EventSource, EventType, RuntimeEvent, RuntimeEventKind};
pub use feature_state::{FeatureState, FeatureStateSelector};
pub use map::{Applied, CameraQuery, CommandGroup, Map, MapId, MapOptions};
pub use mln_map::{LogicalExtent, MapSnapshot};
pub use mln_renderer::query::{RenderedQueryOptions, SourceQueryOptions};
pub use mln_renderer::{ClipDepthRange, FrameProjection};
pub use mln_util::{Error, ErrorKind};
pub use offline::Offline;
#[cfg(all(feature = "gl", target_family = "wasm"))]
pub use render::WebGlContext;
pub use render::{
    AcquiredFrame, Adapter, AttachOptions, BackendType, BorrowedRing, CanvasFormat, ConsumerSync, DemandMode,
    DepthStencilFormat, DepthStencilFormats, Device, DeviceOptions, DmaBufFormat, DmaBufPlane, Driver, DriverWork,
    ExportKind, ExportKinds, ExportedImage, ExportedRing, FrameDemand, FrameDepth, FrameDisposition, FrameResult,
    FrameTexture, FrameTextureKind, FrameView, GlApi, GpuSync, LocalTarget, NativeTexture, RenderSession,
    RenderSessionCapabilities, RenderSessionSnapshot, RenderSessionState, RenderTarget, RenderedQueryGeometry, Surface,
    SurfaceType, SyncKind, SyncKinds, Wake,
};
pub use resources::{CacheDatabaseOperation, ResourceProvider, ResourceRequest, is_managed_http_header};
pub use runtime::{Runtime, RuntimeOptions};
pub use style::{CustomSourceCallbacks, GeoJsonSourceOptions, LayerInfo, SourceInfo, SourceKind, SourceTiles};
