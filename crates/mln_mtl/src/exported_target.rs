// A ring of IOSurfaces that the target allocates and renders into through
// Metal textures over them, for the host to draw, such as through a
// CVPixelBuffer. A resize makes a new generation of surfaces; the frames that
// the host holds keep the old generation's surfaces until they are given
// back.

use std::cell::RefCell;
use std::rc::Rc;

use mln_gfx::command_encoder::CommandEncoder;
use mln_gfx::export::Generations;
use mln_gfx::frame::{self, FrameTarget, Handoff};
use mln_gfx::{
    ConsumerSync, ExportKind, ExportKinds, ExportedFrame, ExportedImage, ExportedRing, FrameDepth, NativeTexture,
    SyncKind, SyncKinds,
};
use mln_util::error::Error;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;
use objc2::rc::Retained;
use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFString};
use objc2_io_surface::{
    IOSurfaceRef, kIOSurfaceBytesPerElement, kIOSurfaceBytesPerRow, kIOSurfaceHeight, kIOSurfacePixelFormat,
    kIOSurfaceWidth,
};
use objc2_metal::{MTLDevice, MTLPixelFormat, MTLStorageMode, MTLTexture, MTLTextureDescriptor, MTLTextureUsage};

use crate::borrowed_target::{BorrowedTarget, BorrowedTextures};
use crate::device::Device;
use crate::mtl_fwd::{MTLDevicePtr, MTLTexturePtr};
use crate::renderer_backend::RendererBackend;

/// `kCVPixelFormatType_32BGRA`, the four characters 'BGRA', which Metal
/// renders as `MTLPixelFormatBGRA8Unorm`.
const PIXEL_FORMAT: i64 = 0x4247_5241;
const BYTES_PER_ELEMENT: i64 = 4;

/// The kinds of exported ring that sessions on `device` share their images
/// in: IOSurfaces, once the device renders into one.
#[must_use]
pub fn export_kinds(device: &Device) -> ExportKinds {
    device.export_kinds()
}

/// The kinds that `device` exports, found by making a texture over an
/// IOSurface.
pub(crate) fn probe(device: &Device) -> ExportKinds {
    match surface_texture(device.device(), Size::new(1, 1)) {
        Ok(_) => ExportKinds::of(&[ExportKind::IoSurface]),
        Err(error) => {
            Log::info(Event::Render, format!("The Metal device renders into no IOSurface: {}", error.message));
            ExportKinds::NONE
        }
    }
}

/// The kind of `ring` that a session on `device` shares its images in.
///
/// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) when the host
/// takes no IOSurface.
pub fn plan(device: &Device, ring: &ExportedRing) -> Result<ExportKind, Error> {
    export_kinds(device)
        .intersection(ring.kinds)
        .iter()
        .next()
        .ok_or_else(|| Error::unsupported("the Metal device exports none of the ring's kinds of image"))
}

/// The kinds of sync with which the host waits for a frame (the
/// producer's), and the target for the host's reads (the consumer's): a
/// shared event, which other APIs import, or a finished frame.
#[must_use]
pub fn sync_kinds(_device: &Device) -> (SyncKinds, SyncKinds) {
    let kinds = SyncKinds::of(&[SyncKind::CpuComplete, SyncKind::MetalSharedEvent]);
    (kinds, kinds)
}

/// A new IOSurface of `size` pixels in the ring's pixel format.
fn create_surface(size: Size) -> Result<CFRetained<IOSurfaceRef>, Error> {
    // SAFETY: the keys are IOSurface's own.
    let keys: [&CFString; 5] = unsafe {
        [kIOSurfaceWidth, kIOSurfaceHeight, kIOSurfaceBytesPerElement, kIOSurfaceBytesPerRow, kIOSurfacePixelFormat]
    };
    // SAFETY: as above.
    let bytes_per_row = IOSurfaceRef::align_property(
        unsafe { kIOSurfaceBytesPerRow },
        size.width as usize * BYTES_PER_ELEMENT as usize,
    );
    let values = [
        CFNumber::new_i64(i64::from(size.width)),
        CFNumber::new_i64(i64::from(size.height)),
        CFNumber::new_i64(BYTES_PER_ELEMENT),
        CFNumber::new_i64(bytes_per_row as i64),
        CFNumber::new_i64(PIXEL_FORMAT),
    ];
    let values: Vec<&CFNumber> = values.iter().map(|value| &**value).collect();
    let properties = CFDictionary::<CFString, CFNumber>::from_slices(&keys, &values);
    // SAFETY: the properties map IOSurface's keys to numbers.
    unsafe { IOSurfaceRef::new(properties.as_opaque()) }.ok_or_else(|| Error::native("IOSurface can't make a surface"))
}

/// A new IOSurface of `size` pixels, and a texture of `device` over it that
/// frames render into.
fn surface_texture(device: &MTLDevicePtr, size: Size) -> Result<(CFRetained<IOSurfaceRef>, MTLTexturePtr), Error> {
    let surface = create_surface(size)?;
    // SAFETY: a plain descriptor of the surface's size and format.
    let descriptor = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            MTLPixelFormat::BGRA8Unorm,
            size.width as usize,
            size.height as usize,
            false,
        )
    };
    // No shader writes, which would cost lossless compression on Apple GPUs.
    descriptor.setUsage(MTLTextureUsage::ShaderRead | MTLTextureUsage::RenderTarget);
    let texture = device
        .newTextureWithDescriptor_iosurface_plane(&descriptor, &surface, 0)
        .ok_or_else(|| Error::native("Metal can't make a texture of the IOSurface"))?;
    Ok((surface, texture))
}

/// One generation of a ring's surfaces, of one size, and the borrowed ring
/// target that draws into the textures over them.
struct Generation {
    target: BorrowedTarget,
    surfaces: Vec<CFRetained<IOSurfaceRef>>,
    /// The textures over `surfaces`, and each slot's depth texture when the
    /// ring keeps depth.
    textures: BorrowedTextures,
    size: Size,
}

impl Generation {
    fn new(
        backend: &Rc<RendererBackend>,
        depth: usize,
        size: Size,
        producer: SyncKind,
        keep_depth: bool,
    ) -> Result<Generation, Error> {
        let device = backend.device().device().clone();
        let mut surfaces = Vec::with_capacity(depth);
        let mut textures = Vec::with_capacity(depth);
        let mut depth_textures = Vec::new();
        for _ in 0..depth {
            let (surface, texture) = surface_texture(&device, size)?;
            surfaces.push(surface);
            textures.push(texture);
            if keep_depth {
                depth_textures.push(depth_texture(&device, size)?);
            }
        }
        let textures = BorrowedTextures { textures, size, depth_textures };
        let target = BorrowedTarget::new(Rc::clone(backend), textures.clone(), producer)?;
        Ok(Generation { target, surfaces, textures, size })
    }
}

/// A private texture of `size` that a frame stores its depth and stencil
/// into, for the host to sample or draw into on the device.
fn depth_texture(device: &MTLDevicePtr, size: Size) -> Result<MTLTexturePtr, Error> {
    // SAFETY: a plain descriptor of a non-zero size and a 2D format.
    let descriptor = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            MTLPixelFormat::Depth32Float_Stencil8,
            size.width as usize,
            size.height as usize,
            false,
        )
    };
    descriptor.setUsage(MTLTextureUsage::ShaderRead | MTLTextureUsage::RenderTarget);
    descriptor.setStorageMode(MTLStorageMode::Private);
    device.newTextureWithDescriptor(&descriptor).ok_or_else(|| Error::native("Metal can't make a depth texture"))
}

/// Frames drawn into IOSurfaces of the target's own, and queued for the host
/// to acquire.
pub struct ExportedTarget {
    backend: Rc<RendererBackend>,
    depth: usize,
    producer: SyncKind,
    keep_depth: bool,
    generations: RefCell<Generations<Generation>>,
}

impl ExportedTarget {
    /// A target on `backend` that draws into `ring.depth` IOSurfaces of
    /// `size` pixels, and hands each frame over with sync of the kind
    /// `producer`, one of [`sync_kinds`]'s.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for
    /// another kind than IOSurfaces or `producer`, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) when a surface can't be
    /// made.
    pub fn new(
        backend: Rc<RendererBackend>,
        ring: &ExportedRing,
        kind: ExportKind,
        size: Size,
        producer: SyncKind,
    ) -> Result<ExportedTarget, Error> {
        if !backend.device().export_kinds().contains(kind) {
            return Err(Error::unsupported(format!("the Metal device exports no {kind:?}")));
        }
        if !sync_kinds(backend.device()).0.contains(producer) {
            return Err(Error::unsupported(format!("an exported IOSurface hands no frame over with {producer:?}")));
        }
        let current = Generation::new(&backend, ring.depth, size, producer, ring.keep_depth)?;
        Ok(ExportedTarget {
            backend,
            depth: ring.depth,
            producer,
            keep_depth: ring.keep_depth,
            generations: RefCell::new(Generations::new(current)),
        })
    }

    /// Has the next frame draw into the surface at `slot`.
    pub fn select(&self, slot: usize) {
        self.generations.borrow().current().target.select(slot);
    }

    /// The host gave the frame at `slot` back, once its reads reach
    /// `consumer`.
    ///
    /// Fails as [`BorrowedTarget::release`] does.
    pub fn release(&self, slot: usize, consumer: ConsumerSync) -> Result<(), Error> {
        let mut generations = self.generations.borrow_mut();
        if generations.give_back(slot) {
            return Ok(());
        }
        generations.current().target.release(slot, consumer)
    }

    /// The host gave the frame at `slot` up.
    pub fn discard(&self, slot: usize) {
        let mut generations = self.generations.borrow_mut();
        if !generations.give_back(slot) {
            generations.current().target.discard(slot);
        }
    }

    /// Draws into surfaces of `size` pixels from now on. The frames that the
    /// host holds, at `held` slots, keep their surfaces. Returns whether the
    /// ring has new surfaces, which it doesn't when they have `size` already.
    ///
    /// Fails with [`ErrorKind::Native`](mln_util::ErrorKind) when a surface
    /// can't be made, and the target then keeps its own.
    pub fn resize(&self, size: Size, held: &[usize]) -> Result<bool, Error> {
        if self.size() == size {
            return Ok(false);
        }
        let next = Generation::new(&self.backend, self.depth, size, self.producer, self.keep_depth)?;
        self.generations.borrow_mut().replace(next, held);
        Ok(true)
    }

    /// The size of the surfaces that frames draw into.
    pub fn size(&self) -> Size {
        self.generations.borrow().current().size
    }

    /// The surface of the frame at `slot`, the texture over it, and its
    /// depth texture when the ring keeps depth.
    pub fn image(&self, slot: usize) -> Option<ExportedFrame> {
        let generations = self.generations.borrow();
        let generation = generations.retired(slot).unwrap_or(generations.current());
        let surface = generation.surfaces.get(slot)?;
        let native = |texture: &MTLTexturePtr| NativeTexture::Metal {
            texture: Retained::as_ptr(texture) as usize as u64,
            pixel_format: texture.pixelFormat().0 as u64,
        };
        Some(ExportedFrame {
            size: generation.size,
            image: ExportedImage::IoSurface { surface: CFRetained::as_ptr(surface).as_ptr() as usize as u64 },
            texture: native(generation.textures.textures.get(slot)?),
            depth: generation
                .textures
                .depth_textures
                .get(slot)
                .map(|texture| FrameDepth { texture: native(texture), export: None }),
        })
    }
}

impl FrameTarget for ExportedTarget {
    fn begin_frame(&self) -> Option<Rc<dyn frame::DrawTarget>> {
        self.generations.borrow().current().target.begin_frame()
    }

    fn end_frame(&self, encoder: &dyn CommandEncoder) -> Handoff {
        match self.generations.borrow().current().target.end_frame(encoder) {
            Handoff::GivenBack { slot, sync } => Handoff::Queued { slot, sync },
            other => other,
        }
    }
}

impl Drop for ExportedTarget {
    fn drop(&mut self) {
        self.generations.get_mut().clear_retired();
    }
}
