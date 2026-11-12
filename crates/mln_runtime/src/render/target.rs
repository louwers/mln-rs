//! What a render session renders into, and how it attaches.

use std::sync::Arc;

use mln_gfx::Window;
pub use mln_gfx::backend::Type as BackendType;
pub use mln_gfx::{
    ConsumerSync, DmaBufFormat, DmaBufPlane, ExportKind, ExportKinds, ExportedImage, ExportedRing, GpuSync, SyncKind,
    SyncKinds,
};
use mln_util::error::Error;
use mln_util::size::Size;

/// A callback that tells the host about something it should act on, such as
/// frame results to drain. It runs on a library thread and must not block.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// The thread that runs a render session's graphics work.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Driver {
    /// A thread of the session's own.
    #[default]
    CoreWorker,
    /// The host's graphics thread, which runs the work through the session's
    /// [`DriverWork`](crate::DriverWork).
    CallerThread,
}

/// How a session attaches to its map.
#[derive(Clone, Default)]
pub struct AttachOptions {
    pub driver: Driver,
    /// Runs when frame results wait for
    /// [`drain_frame_results`](crate::RenderSession::drain_frame_results),
    /// and not again until a drain takes them.
    pub frame_wake: Option<Wake>,
    /// Runs when the map has something new to draw, and not again until the
    /// host demands a frame, or while the session has no target.
    pub redraw_wake: Option<Wake>,
    /// Runs when a [`Driver::CallerThread`] session has work for
    /// [`DriverWork::service`](crate::DriverWork::service), and not again
    /// until a service starts.
    pub driver_work_wake: Option<Wake>,
    /// The kinds of sync that the host can wait for before it reads a
    /// borrowed ring's frame. The session picks the one of them that its
    /// target supports and that waits least: see [`SyncKinds::pick`].
    pub producer_sync_kinds: SyncKinds,
}

impl std::fmt::Debug for AttachOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttachOptions")
            .field("driver", &self.driver)
            .field("frame_wake", &self.frame_wake.is_some())
            .field("redraw_wake", &self.redraw_wake.is_some())
            .field("driver_work_wake", &self.driver_work_wake.is_some())
            .field("producer_sync_kinds", &self.producer_sync_kinds)
            .finish()
    }
}

/// What a render session renders into. The session builds it into the
/// backend's objects on the thread that renders, on `device`, or on a device
/// that [`Device::new`] makes when that is `None`. The device's backend is
/// the session's.
#[derive(Debug)]
pub enum RenderTarget {
    /// An image of `size` logical pixels, rendered at the map's pixel ratio,
    /// whose pixels [`read_texture`](crate::RenderSession::read_texture)
    /// reads.
    Offscreen { size: Size, device: Option<Device> },
    /// A surface of `size` logical pixels, rendered at the map's pixel ratio,
    /// that shows each frame that the session renders. Without a device, a
    /// window renders on the platform's native backend where that takes
    /// windows, and on WebGPU otherwise (on Apple's platforms, whose Metal
    /// takes a layer); a Metal layer renders on Metal.
    ///
    /// [`release_target`](crate::RenderSession::release_target) lets go of
    /// the surface, and [`set_surface`](crate::RenderSession::set_surface)
    /// gives the session another one.
    Surface { size: Size, surface: Surface, device: Option<Device> },
    /// A ring of the host's textures of `size` logical pixels, on `device`,
    /// whose backend is the ring's. Each frame goes to the host, which
    /// takes it with [`acquire_frame`](crate::RenderSession::acquire_frame)
    /// and gives it back once it has read it. The ring's textures set the
    /// size in pixels, and [`set_ring`](crate::RenderSession::set_ring)
    /// gives the session new ones.
    Borrowed { size: Size, ring: BorrowedRing, device: Device },
    /// A ring of images of `size` logical pixels, rendered at the map's
    /// pixel ratio, that the session allocates on `device` and shares with
    /// the host in a kind of `ring.kinds`: the first, by
    /// [`ExportKind::BY_PREFERENCE`], that the device exports. Without a
    /// device, the session renders on the first of [`Device::new`]'s
    /// backends whose device exports one. Each frame goes to the host, which
    /// takes it with [`acquire_frame`](crate::RenderSession::acquire_frame)
    /// and gives it back once it has read it.
    /// [`resize`](crate::RenderSession::resize) gives the ring images of a
    /// new size, and the frames that the host holds keep theirs.
    Exported { size: Size, ring: ExportedRing, device: Option<Device> },
}

/// A ring of the host's textures, of one backend.
#[derive(Clone, Debug, PartialEq)]
pub struct BorrowedRing {
    pub(crate) kind: RingKind,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RingKind {
    #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
    Vulkan(mln_vulkan::BorrowedImages),
    #[cfg(all(feature = "metal", target_vendor = "apple"))]
    Metal(mln_mtl::BorrowedTextures),
    #[cfg(all(feature = "gl", not(target_family = "wasm")))]
    Gl(mln_gl::BorrowedTextures),
    #[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
    WebGpu(mln_webgpu::BorrowedTextures),
}

/// The host's texture that an acquired frame was rendered into, of `size`
/// pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameTexture {
    pub size: Size,
    pub kind: FrameTextureKind,
}

/// A [`FrameTexture`]'s backend objects.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum FrameTextureKind {
    /// A `VkImage` and its `VkImageView` of a `VkDevice`, in a `VkFormat`,
    /// left in a `VkImageLayout`.
    Vulkan { image: u64, view: u64, device: u64, format: i32, layout: i32 },
    /// An `id<MTLTexture>` of an `id<MTLDevice>`, in an `MTLPixelFormat`.
    Metal { texture: u64, device: u64, pixel_format: u64 },
    /// An OpenGL texture name, of a texture target.
    OpenGl { texture: u32, target: u32 },
    /// A WebGPU texture: the host's at the frame's slot of its ring.
    WebGpu,
    /// An image of an exported ring, and its shared memory.
    Exported(ExportedImage),
}

impl BorrowedRing {
    /// A ring of the host's Vulkan images.
    #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
    #[must_use]
    pub fn vulkan(images: mln_vulkan::BorrowedImages) -> BorrowedRing {
        BorrowedRing { kind: RingKind::Vulkan(images) }
    }

    /// A ring of the host's Metal textures.
    #[cfg(all(feature = "metal", target_vendor = "apple"))]
    pub fn metal(textures: mln_mtl::BorrowedTextures) -> BorrowedRing {
        BorrowedRing { kind: RingKind::Metal(textures) }
    }

    /// A ring of the host's OpenGL textures, in the share group of a device
    /// that wraps the host's context.
    #[cfg(all(feature = "gl", not(target_family = "wasm")))]
    #[must_use]
    pub fn gl(textures: mln_gl::BorrowedTextures) -> BorrowedRing {
        BorrowedRing { kind: RingKind::Gl(textures) }
    }

    /// A ring of the host's WebGPU textures, of the WebGPU device that
    /// renders into them.
    #[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
    #[must_use]
    pub fn webgpu(textures: mln_webgpu::BorrowedTextures) -> BorrowedRing {
        BorrowedRing { kind: RingKind::WebGpu(textures) }
    }

    /// The backend whose device renders into the ring.
    #[must_use]
    pub fn backend(&self) -> BackendType {
        match self.kind {
            #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
            RingKind::Vulkan(_) => BackendType::Vulkan,
            #[cfg(all(feature = "metal", target_vendor = "apple"))]
            RingKind::Metal(_) => BackendType::Metal,
            #[cfg(all(feature = "gl", not(target_family = "wasm")))]
            RingKind::Gl(_) => BackendType::OpenGL,
            #[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
            RingKind::WebGpu(_) => BackendType::WebGPU,
            // The browser build renders into no ring.
            #[allow(unreachable_patterns)]
            _ => BackendType::WebGPU,
        }
    }

    /// How many textures the ring has.
    #[must_use]
    pub fn depth(&self) -> usize {
        match &self.kind {
            #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
            RingKind::Vulkan(images) => images.images.len(),
            #[cfg(all(feature = "metal", target_vendor = "apple"))]
            RingKind::Metal(textures) => textures.textures.len(),
            #[cfg(all(feature = "gl", not(target_family = "wasm")))]
            RingKind::Gl(textures) => textures.textures.len(),
            #[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
            RingKind::WebGpu(textures) => textures.textures.len(),
            #[allow(unreachable_patterns)]
            _ => 0,
        }
    }

    /// The textures' size in pixels.
    #[must_use]
    pub fn physical_size(&self) -> Size {
        match &self.kind {
            #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
            RingKind::Vulkan(images) => images.size,
            #[cfg(all(feature = "metal", target_vendor = "apple"))]
            RingKind::Metal(textures) => textures.size,
            #[cfg(all(feature = "gl", not(target_family = "wasm")))]
            RingKind::Gl(textures) => textures.size,
            #[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
            RingKind::WebGpu(textures) => textures.size,
            #[allow(unreachable_patterns)]
            _ => Size::default(),
        }
    }

    /// Whether the texture at `slot` is the one at `other`'s `other_slot`.
    #[cfg_attr(target_family = "wasm", allow(unused_variables))]
    pub(crate) fn same_texture(&self, slot: usize, other: &BorrowedRing, other_slot: usize) -> bool {
        #[allow(unreachable_patterns)]
        match (&self.kind, &other.kind) {
            #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
            (RingKind::Vulkan(images), RingKind::Vulkan(others)) => images
                .images
                .get(slot)
                .zip(others.images.get(other_slot))
                .is_some_and(|(image, other)| image.image == other.image),
            #[cfg(all(feature = "metal", target_vendor = "apple"))]
            (RingKind::Metal(textures), RingKind::Metal(others)) => {
                textures.textures.get(slot).zip(others.textures.get(other_slot)).is_some_and(|(texture, other)| {
                    std::ptr::eq(
                        mln_mtl::objc2::rc::Retained::as_ptr(texture),
                        mln_mtl::objc2::rc::Retained::as_ptr(other),
                    )
                })
            }
            #[cfg(all(feature = "gl", not(target_family = "wasm")))]
            (RingKind::Gl(textures), RingKind::Gl(others)) => {
                textures.textures.get(slot).is_some_and(|texture| others.textures.get(other_slot) == Some(texture))
            }
            #[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
            (RingKind::WebGpu(textures), RingKind::WebGpu(others)) => {
                textures.textures.get(slot).is_some_and(|texture| others.textures.get(other_slot) == Some(texture))
            }
            _ => false,
        }
    }

    /// The texture at `slot`, which sessions on `device` render into.
    #[cfg_attr(target_family = "wasm", allow(unused_variables))]
    #[must_use]
    pub fn texture(&self, slot: usize, device: &Device) -> Option<FrameTexture> {
        #[allow(unreachable_patterns)]
        match (&self.kind, device) {
            #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
            (RingKind::Vulkan(images), Device::Vulkan(device)) => {
                use mln_vulkan::ash::vk::Handle as _;
                images.images.get(slot).map(|image| FrameTexture {
                    size: images.size,
                    kind: FrameTextureKind::Vulkan {
                        image: image.image.as_raw(),
                        view: image.view.as_raw(),
                        device: device.raw().device.as_raw(),
                        format: images.format.as_raw(),
                        layout: images.final_layout.as_raw(),
                    },
                })
            }
            #[cfg(all(feature = "metal", target_vendor = "apple"))]
            (RingKind::Metal(textures), Device::Metal(device)) => {
                use mln_mtl::objc2::rc::Retained;
                textures.textures.get(slot).map(|texture| FrameTexture {
                    size: textures.size,
                    kind: FrameTextureKind::Metal {
                        texture: Retained::as_ptr(texture) as usize as u64,
                        device: Retained::as_ptr(device.device()) as usize as u64,
                        pixel_format: textures.pixel_format().0 as u64,
                    },
                })
            }
            #[cfg(all(feature = "gl", not(target_family = "wasm")))]
            (RingKind::Gl(textures), Device::Gl(_)) => textures.textures.get(slot).map(|&texture| FrameTexture {
                size: textures.size,
                // GL_TEXTURE_2D.
                kind: FrameTextureKind::OpenGl { texture, target: 0x0DE1 },
            }),
            #[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
            (RingKind::WebGpu(textures), Device::WebGpu(_)) => textures
                .textures
                .get(slot)
                .map(|_| FrameTexture { size: textures.size, kind: FrameTextureKind::WebGpu }),
            _ => None,
        }
    }
}

/// What a surface session presents to.
pub struct Surface {
    pub(crate) kind: SurfaceKind,
}

pub(crate) enum SurfaceKind {
    Window(Arc<dyn Window>),
    #[cfg(all(feature = "metal", target_vendor = "apple"))]
    MetalLayer(mln_mtl::Layer),
    #[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
    Wgpu(wgpu::Surface<'static>),
    /// The host's `VkSurfaceKHR`, of the instance of the session's device.
    #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
    Vulkan(mln_vulkan::HostSurface),
    /// The host's `EGLSurface`, of the display of the session's device.
    #[cfg(all(feature = "gl", not(target_family = "wasm")))]
    Gl(mln_gl::HostSurface),
}

impl Surface {
    /// A window, which the session makes its surface on. The window must
    /// outlive the session's use of it: until a
    /// [`set_surface`](crate::RenderSession::set_surface) or
    /// [`release_target`](crate::RenderSession::release_target) replaces it,
    /// or the session detaches or is abandoned.
    pub fn window(window: impl Window + 'static) -> Surface {
        Surface { kind: SurfaceKind::Window(Arc::new(window)) }
    }

    /// A `CAMetalLayer`, which a session on Metal presents to. It must
    /// outlive the session's use of it, as a window must.
    #[cfg(all(feature = "metal", target_vendor = "apple"))]
    #[must_use]
    pub fn metal_layer(layer: mln_mtl::Layer) -> Surface {
        Surface { kind: SurfaceKind::MetalLayer(layer) }
    }

    /// A surface that the host made with the instance of the WebGPU
    /// [`Device`] that the session renders on.
    #[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
    pub fn wgpu(surface: wgpu::Surface<'static>) -> Surface {
        Surface { kind: SurfaceKind::Wgpu(surface) }
    }

    /// The host's `VkSurfaceKHR`, which a session on the Vulkan device
    /// whose instance made it presents to. The surface stays the host's.
    #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
    #[must_use]
    pub fn vulkan(surface: mln_vulkan::HostSurface) -> Surface {
        Surface { kind: SurfaceKind::Vulkan(surface) }
    }

    /// The host's `EGLSurface`, which a session on the OpenGL device that
    /// wraps the host's context presents to. The surface stays the host's.
    #[cfg(all(feature = "gl", not(target_family = "wasm")))]
    #[must_use]
    pub fn gl(surface: mln_gl::HostSurface) -> Surface {
        Surface { kind: SurfaceKind::Gl(surface) }
    }

    /// The backend of the device whose instance or share group a host's
    /// surface is of, or `None` for a window or layer.
    #[must_use]
    pub fn host_backend(&self) -> Option<BackendType> {
        match self.kind {
            #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
            SurfaceKind::Vulkan(_) => Some(BackendType::Vulkan),
            #[cfg(all(feature = "gl", not(target_family = "wasm")))]
            SurfaceKind::Gl(_) => Some(BackendType::OpenGL),
            #[allow(unreachable_patterns)]
            _ => None,
        }
    }

    /// The surface again, if it is a window, or a surface that a new
    /// backend presents to.
    pub(crate) fn clone_window(&self) -> Option<Surface> {
        match &self.kind {
            SurfaceKind::Window(window) => Some(Surface { kind: SurfaceKind::Window(window.clone()) }),
            #[cfg(all(feature = "gl", not(target_family = "wasm")))]
            SurfaceKind::Gl(surface) => Some(Surface { kind: SurfaceKind::Gl(*surface) }),
            #[allow(unreachable_patterns)]
            _ => None,
        }
    }

    /// Whether the session can make the surface without the host's device.
    pub(crate) fn needs_no_device(&self) -> bool {
        match self.kind {
            SurfaceKind::Window(_) => true,
            #[cfg(all(feature = "metal", target_vendor = "apple"))]
            SurfaceKind::MetalLayer(_) => true,
            #[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
            SurfaceKind::Wgpu(_) => false,
            #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
            SurfaceKind::Vulkan(_) => false,
            #[cfg(all(feature = "gl", not(target_family = "wasm")))]
            SurfaceKind::Gl(_) => false,
        }
    }
}

impl std::fmt::Debug for Surface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Surface").finish_non_exhaustive()
    }
}

/// A GPU that sessions on many maps and threads share: one backend's
/// device. Sessions on it keep working after the last handle drops, until
/// they detach.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Device {
    /// A WebGPU device: the library's, through wgpu, or the host's
    /// ([`Device::webgpu`]).
    #[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
    WebGpu(mln_webgpu::Device),
    /// A Metal device and command queue.
    #[cfg(all(feature = "metal", target_vendor = "apple"))]
    Metal(mln_mtl::Device),
    /// A Vulkan GPU.
    #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
    Vulkan(mln_vulkan::Device),
    /// An OpenGL ES share group.
    #[cfg(all(feature = "gl", not(target_family = "wasm")))]
    Gl(mln_gl::Device),
}

/// How to create a [`Device`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct DeviceOptions {
    /// The GPU to use. `None` picks the default one.
    pub adapter: Option<Adapter>,
}

/// A GPU, by what names it to a backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Adapter {
    /// An index into the backend's list of GPUs.
    Index(usize),
    /// The GPU's 16-byte UUID, as Vulkan's
    /// `VkPhysicalDeviceIDProperties.deviceUUID` reports it.
    Uuid([u8; 16]),
    /// The GPU's 8-byte LUID, as Vulkan's
    /// `VkPhysicalDeviceIDProperties.deviceLUID` reports it: on Windows, its
    /// DXGI adapter's.
    Luid([u8; 8]),
}

/// A kind of window or layer that a session presents to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SurfaceType {
    /// A `CAMetalLayer`.
    MetalLayer,
    /// A Win32 window.
    Win32,
    /// An X11 window, through Xlib.
    Xlib,
    /// An X11 window, through XCB.
    Xcb,
    /// A Wayland surface.
    Wayland,
    /// An Android `ANativeWindow`.
    AndroidNdk,
}

impl SurfaceType {
    /// Whether a device of `backend` presents to the surfaces of this type
    /// on this platform, in this build.
    #[must_use]
    pub fn presents_with(self, backend: BackendType) -> bool {
        let windowing = match self {
            SurfaceType::MetalLayer => false,
            SurfaceType::Win32 => cfg!(windows),
            SurfaceType::Xlib | SurfaceType::Xcb | SurfaceType::Wayland => {
                cfg!(all(unix, not(target_vendor = "apple"), not(target_os = "android")))
            }
            SurfaceType::AndroidNdk => cfg!(target_os = "android"),
        };
        match backend {
            BackendType::Metal => {
                cfg!(all(feature = "metal", target_vendor = "apple")) && self == SurfaceType::MetalLayer
            }
            BackendType::Vulkan => {
                cfg!(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm")))) && windowing
            }
            BackendType::OpenGL => {
                cfg!(all(feature = "gl", unix, not(target_vendor = "apple"), not(target_os = "android")))
                    && matches!(self, SurfaceType::Xlib | SurfaceType::Wayland)
            }
            BackendType::WebGPU => cfg!(all(feature = "webgpu", not(target_family = "wasm"))) && windowing,
            #[allow(unreachable_patterns)]
            _ => false,
        }
    }

    /// Whether a device of any backend of this build presents to the
    /// surfaces of this type.
    #[must_use]
    pub fn presents_with_any(self) -> bool {
        [BackendType::Metal, BackendType::Vulkan, BackendType::OpenGL, BackendType::WebGPU]
            .into_iter()
            .any(|backend| self.presents_with(backend))
    }

    /// Whether a session of this build presents to the surfaces of this
    /// type without the host's device: on the device that a surface session
    /// makes when it gets none.
    #[must_use]
    pub fn presents_without_device(self) -> bool {
        match self {
            SurfaceType::MetalLayer => self.presents_with(BackendType::Metal),
            _ if cfg!(target_vendor = "apple") => self.presents_with(BackendType::WebGPU),
            _ => [BackendType::Vulkan, BackendType::OpenGL, BackendType::WebGPU]
                .into_iter()
                .any(|backend| self.presents_with(backend)),
        }
    }
}

/// Whether a backend that failed with `error` is missing, so the next one
/// may take its place.
#[cfg(not(target_family = "wasm"))]
fn missing(error: &Error) -> bool {
    error.kind == mln_util::ErrorKind::Unsupported
}

impl Device {
    /// The library's device, on the platform's native backend: Metal on
    /// Apple's platforms, and Vulkan elsewhere, or OpenGL ES where there is
    /// no Vulkan. WebGPU (wgpu) takes the place of a native backend that
    /// isn't there. A backend that can't name the GPU that `options` names
    /// is skipped: OpenGL ES names none, and only Vulkan names one by UUID or
    /// LUID.
    ///
    /// Fails with [`ErrorKind::NotFound`](mln_util::ErrorKind) when
    /// the options name no GPU, with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) when no backend has a
    /// GPU or in the browser, where a session renders on the page's device,
    /// and with [`ErrorKind::Native`](mln_util::ErrorKind) when the GPU
    /// can't create the device.
    pub fn new(options: DeviceOptions) -> Result<Device, Error> {
        #[cfg(target_family = "wasm")]
        {
            let _ = options;
            Err(Error::unsupported("the browser has no library devices"))
        }
        #[cfg(not(target_family = "wasm"))]
        {
            let mut error = Error::unsupported("this build has no backend");
            for backend in Self::native_order() {
                match Self::with_backend(*backend, options) {
                    Ok(device) => return Ok(device),
                    Err(e) if missing(&e) => error = e,
                    Err(e) => return Err(e),
                }
            }
            Err(error)
        }
    }

    /// The library's device on `backend`.
    ///
    /// Fails as [`new`](Self::new) does, and with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) for a backend that
    /// isn't in this build or on this platform.
    pub fn with_backend(backend: BackendType, options: DeviceOptions) -> Result<Device, Error> {
        let DeviceOptions { adapter } = options;
        // Only Vulkan names its GPUs by ID here.
        let index = |adapter: Option<Adapter>| match adapter {
            None => Ok(None),
            Some(Adapter::Index(index)) => Ok(Some(index)),
            Some(_) => Err(Error::unsupported(format!("{backend:?} names no GPU by UUID or LUID"))),
        };
        match backend {
            #[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
            BackendType::WebGPU => mln_webgpu::Device::new(index(adapter)?).map(Device::WebGpu),
            #[cfg(all(feature = "metal", target_vendor = "apple"))]
            BackendType::Metal => mln_mtl::Device::new(index(adapter)?).map(Device::Metal),
            #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
            BackendType::Vulkan => match adapter {
                Some(Adapter::Uuid(uuid)) => mln_vulkan::Device::with_adapter_id(mln_vulkan::AdapterId::Uuid(uuid)),
                Some(Adapter::Luid(luid)) => mln_vulkan::Device::with_adapter_id(mln_vulkan::AdapterId::Luid(luid)),
                _ => mln_vulkan::Device::new(index(adapter)?),
            }
            .map(Device::Vulkan),
            #[cfg(all(feature = "gl", not(target_family = "wasm")))]
            BackendType::OpenGL => match adapter {
                Some(_) => Err(Error::unsupported("OpenGL ES picks no GPU")),
                None => mln_gl::Device::new().map(Device::Gl),
            },
            #[allow(unreachable_patterns)]
            _ => {
                let _ = (index, adapter);
                Err(Error::unsupported(format!("this build has no {backend:?} backend")))
            }
        }
    }

    /// The kinds of sync with which the host can wait for a frame of `ring`
    /// on the device (the producer's), and with which a session waits for
    /// the host's reads (the consumer's).
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) when the
    /// device can't render into the ring: one of another backend, or one the
    /// device lacks a feature for.
    pub fn borrowed_sync_kinds(&self, ring: &BorrowedRing) -> Result<(SyncKinds, SyncKinds), Error> {
        #[allow(unreachable_patterns)]
        match (self, &ring.kind) {
            #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
            (Device::Vulkan(device), RingKind::Vulkan(images)) => {
                images.check(device)?;
                Ok(device.sync_kinds())
            }
            #[cfg(all(feature = "metal", target_vendor = "apple"))]
            (Device::Metal(device), RingKind::Metal(textures)) => {
                textures.check(device)?;
                Ok(mln_mtl::BorrowedTextures::sync_kinds(device))
            }
            #[cfg(all(feature = "gl", not(target_family = "wasm")))]
            (Device::Gl(device), RingKind::Gl(_)) => {
                if !device.is_host() {
                    return Err(Error::unsupported(
                        "an OpenGL ring renders on a device in the share group of the host's context",
                    ));
                }
                Ok(mln_gl::BorrowedTextures::sync_kinds())
            }
            #[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
            (Device::WebGpu(_), RingKind::WebGpu(textures)) => {
                textures.check()?;
                Ok(mln_webgpu::BorrowedTextures::sync_kinds())
            }
            _ => Err(Error::unsupported(format!(
                "a {:?} device renders into no ring of {:?} textures",
                self.backend(),
                ring.backend()
            ))),
        }
    }

    /// The kinds of exported ring that sessions on the device share their
    /// images in: those that the device has exported an image of.
    #[must_use]
    pub fn export_kinds(&self) -> ExportKinds {
        match self {
            #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
            Device::Vulkan(device) => device.export_kinds(),
            #[cfg(all(feature = "metal", target_vendor = "apple"))]
            Device::Metal(device) => mln_mtl::exported_target::export_kinds(device),
            #[cfg(all(feature = "gl", unix, not(target_vendor = "apple"), not(target_os = "android")))]
            Device::Gl(device) => mln_gl::exported_target::export_kinds(device),
            #[allow(unreachable_patterns)]
            _ => ExportKinds::NONE,
        }
    }

    /// The kind that a session on the device shares `ring`'s images in, and
    /// the kinds of sync with which the host can wait for a frame (the
    /// producer's) and a session for the host's reads (the consumer's).
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) when the
    /// device exports none of the ring's kinds.
    pub fn export_plan(&self, ring: &ExportedRing) -> Result<(ExportKind, SyncKinds, SyncKinds), Error> {
        #[allow(unreachable_patterns)]
        match self {
            #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
            Device::Vulkan(device) => {
                let kind = mln_vulkan::exported_target::plan(device, ring)?;
                let (producer, consumer) = mln_vulkan::exported_target::sync_kinds(device);
                Ok((kind, producer, consumer))
            }
            #[cfg(all(feature = "metal", target_vendor = "apple"))]
            Device::Metal(device) => {
                let kind = mln_mtl::exported_target::plan(device, ring)?;
                let (producer, consumer) = mln_mtl::exported_target::sync_kinds(device);
                Ok((kind, producer, consumer))
            }
            #[cfg(all(feature = "gl", unix, not(target_vendor = "apple"), not(target_os = "android")))]
            Device::Gl(device) => {
                let kind = mln_gl::exported_target::plan(device, ring)?;
                let (producer, consumer) = mln_gl::exported_target::sync_kinds();
                Ok((kind, producer, consumer))
            }
            _ => {
                let _ = ring;
                Err(Error::unsupported(format!("a {:?} device exports no ring", self.backend())))
            }
        }
    }

    /// The kinds of sync with which the host can wait for an exported
    /// ring's frame (the producer's) and a session for the host's reads (the
    /// consumer's), on a device that exports a ring.
    #[must_use]
    pub fn export_sync_kinds(&self) -> (SyncKinds, SyncKinds) {
        if self.export_kinds().is_empty() {
            return (SyncKinds::NONE, SyncKinds::NONE);
        }
        #[allow(unreachable_patterns)]
        match self {
            #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
            Device::Vulkan(device) => mln_vulkan::exported_target::sync_kinds(device),
            #[cfg(all(feature = "metal", target_vendor = "apple"))]
            Device::Metal(device) => mln_mtl::exported_target::sync_kinds(device),
            #[cfg(all(feature = "gl", unix, not(target_vendor = "apple"), not(target_os = "android")))]
            Device::Gl(_) => mln_gl::exported_target::sync_kinds(),
            _ => (SyncKinds::NONE, SyncKinds::NONE),
        }
    }

    /// The library's device for `ring`: on the first of [`new`](Self::new)'s
    /// backends whose device exports one of the ring's kinds.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) when no
    /// backend's device does, and as [`new`](Self::new) does.
    pub fn for_exported_ring(ring: &ExportedRing) -> Result<Device, Error> {
        #[cfg(target_family = "wasm")]
        {
            let _ = ring;
            Err(Error::unsupported("the browser has no library devices"))
        }
        #[cfg(not(target_family = "wasm"))]
        {
            let mut error = Error::unsupported("no backend of this build exports the ring's kinds of image");
            for backend in Self::native_order() {
                match Self::with_backend(*backend, DeviceOptions::default()) {
                    Ok(device) => match device.export_plan(ring) {
                        Ok(_) => return Ok(device),
                        Err(e) => error = e,
                    },
                    Err(e) if missing(&e) => {}
                    Err(e) => return Err(e),
                }
            }
            Err(error)
        }
    }

    /// Whether sessions on the device present to the host's own surfaces:
    /// a Vulkan device's `VkSurfaceKHR`s, or the `EGLSurface`s of an OpenGL
    /// device that wraps the host's context.
    #[must_use]
    pub fn presents_to_host_surfaces(&self) -> bool {
        match self {
            #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
            Device::Vulkan(device) => device.presents(),
            #[cfg(all(feature = "gl", not(target_family = "wasm")))]
            Device::Gl(device) => device.is_host(),
            #[allow(unreachable_patterns)]
            _ => false,
        }
    }

    /// Whether sessions on the device present to the surfaces of
    /// `surface`'s type.
    #[must_use]
    pub fn presents_to(&self, surface: SurfaceType) -> bool {
        surface.presents_with(self.backend())
    }

    /// The backends [`new`](Self::new) tries, in order.
    #[cfg(not(target_family = "wasm"))]
    fn native_order() -> &'static [BackendType] {
        if cfg!(target_vendor = "apple") {
            &[BackendType::Metal, BackendType::WebGPU]
        } else {
            &[BackendType::Vulkan, BackendType::OpenGL, BackendType::WebGPU]
        }
    }

    /// The host's WebGPU device and queue, which `instance` made. The
    /// session configures its surfaces with `format`, which each surface
    /// must support.
    #[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
    #[must_use]
    pub fn webgpu(
        instance: wgpu::Instance,
        device: wgpu::Device,
        queue: wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Device {
        Device::WebGpu(mln_webgpu::Device::from_wgpu(Some(instance), device, queue, Some(format)))
    }

    /// The backend the device is of.
    #[must_use]
    pub fn backend(&self) -> BackendType {
        match *self {
            #[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
            Device::WebGpu(_) => BackendType::WebGPU,
            #[cfg(all(feature = "metal", target_vendor = "apple"))]
            Device::Metal(_) => BackendType::Metal,
            #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
            Device::Vulkan(_) => BackendType::Vulkan,
            #[cfg(all(feature = "gl", not(target_family = "wasm")))]
            Device::Gl(_) => BackendType::OpenGL,
        }
    }
}
