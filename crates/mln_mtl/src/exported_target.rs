// A ring of IOSurfaces that the target allocates and renders into through
// Metal textures over them, for the host to draw, such as through a
// CVPixelBuffer. The ring target (mln_gfx::ring) draws into the textures as
// into a borrowed ring's.

use mln_gfx::ring::{self, Exporter};
use mln_gfx::{
    Error, ExportKind, ExportKinds, ExportedFrame, ExportedImage, ExportedRing, FrameDepth, NativeTexture, Size,
    SyncKind, SyncKinds,
};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFString};
use objc2_io_surface::{
    IOSurfaceRef, kIOSurfaceBytesPerElement, kIOSurfaceBytesPerRow, kIOSurfaceHeight, kIOSurfacePixelFormat,
    kIOSurfaceWidth,
};
use objc2_metal::{MTLDevice, MTLPixelFormat, MTLStorageMode, MTLTexture, MTLTextureUsage};

use crate::borrowed_target::{BorrowedTextures, MetalRing};
use crate::device::Device;
use crate::gpu::texture_descriptor;
use crate::renderer_backend::RendererBackend;
use crate::{Id, log_target};

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
            log::info!(target: log_target::RENDER, "The Metal device renders into no IOSurface: {}", error.message);
            ExportKinds::NONE
        }
    }
}

/// The kind of `ring` that a session on `device` shares its images in.
///
/// Fails with [`ErrorKind::Unsupported`](mln_gfx::ErrorKind) when the host
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
fn surface_texture(
    device: &ProtocolObject<dyn MTLDevice>,
    size: Size,
) -> Result<(CFRetained<IOSurfaceRef>, Id<dyn MTLTexture>), Error> {
    let surface = create_surface(size)?;
    let descriptor = texture_descriptor(MTLPixelFormat::BGRA8Unorm, size);
    // No shader writes, which would cost lossless compression on Apple GPUs.
    descriptor.setUsage(MTLTextureUsage::ShaderRead | MTLTextureUsage::RenderTarget);
    let texture = device
        .newTextureWithDescriptor_iosurface_plane(&descriptor, &surface, 0)
        .ok_or_else(|| Error::native("Metal can't make a texture of the IOSurface"))?;
    Ok((surface, texture))
}

/// A private texture of `size` that a frame stores its depth and stencil
/// into, for the host to sample or draw into on the device.
fn depth_texture(device: &ProtocolObject<dyn MTLDevice>, size: Size) -> Result<Id<dyn MTLTexture>, Error> {
    let descriptor = texture_descriptor(MTLPixelFormat::Depth32Float_Stencil8, size);
    descriptor.setUsage(MTLTextureUsage::ShaderRead | MTLTextureUsage::RenderTarget);
    descriptor.setStorageMode(MTLStorageMode::Private);
    device.newTextureWithDescriptor(&descriptor).ok_or_else(|| Error::native("Metal can't make a depth texture"))
}

/// Frames drawn into IOSurfaces of the target's own, and queued for the host
/// to acquire.
pub type ExportedTarget = ring::ExportedTarget<MetalExporter>;

/// An [`ExportedTarget`]'s part on Metal: it allocates IOSurfaces and the
/// textures over them, and with the ring's depth, a private depth texture
/// beside each.
pub struct MetalExporter {
    depth: usize,
    keep_depth: bool,
}

impl Exporter for MetalExporter {
    type Ring = MetalRing;
    /// A generation's surfaces, under the textures that frames draw into.
    type Images = Vec<CFRetained<IOSurfaceRef>>;

    /// Fails with [`ErrorKind::Unsupported`](mln_gfx::ErrorKind) for
    /// another kind than IOSurfaces, or another producer kind than
    /// [`sync_kinds`]'s.
    fn new(
        backend: &mut RendererBackend,
        ring: &ExportedRing,
        kind: ExportKind,
        producer: SyncKind,
    ) -> Result<MetalExporter, Error> {
        if !backend.device().export_kinds().contains(kind) {
            return Err(Error::unsupported(format!("the Metal device exports no {kind:?}")));
        }
        if !sync_kinds(backend.device()).0.contains(producer) {
            return Err(Error::unsupported(format!("an exported IOSurface hands no frame over with {producer:?}")));
        }
        Ok(MetalExporter { depth: ring.depth, keep_depth: ring.keep_depth })
    }

    /// Fails with [`ErrorKind::Native`](mln_gfx::ErrorKind) when a surface
    /// or a texture can't be made.
    fn allocate(
        &self,
        backend: &mut RendererBackend,
        size: Size,
    ) -> Result<(Vec<CFRetained<IOSurfaceRef>>, BorrowedTextures), Error> {
        let device = backend.device().device();
        let mut surfaces = Vec::with_capacity(self.depth);
        let mut textures = Vec::with_capacity(self.depth);
        let mut depth_textures = Vec::new();
        for _ in 0..self.depth {
            let (surface, texture) = surface_texture(device, size)?;
            surfaces.push(surface);
            textures.push(texture);
            if self.keep_depth {
                depth_textures.push(depth_texture(device, size)?);
            }
        }
        Ok((surfaces, BorrowedTextures { textures, size, depth_textures }))
    }

    /// The surface of the frame at `slot`, the texture over it, and its
    /// depth texture when the ring keeps depth.
    fn frame(
        &self,
        surfaces: &Vec<CFRetained<IOSurfaceRef>>,
        textures: &BorrowedTextures,
        slot: usize,
    ) -> Option<ExportedFrame> {
        let surface = surfaces.get(slot)?;
        let native = |texture: &Id<dyn MTLTexture>| NativeTexture::Metal {
            texture: Retained::as_ptr(texture) as usize as u64,
            pixel_format: texture.pixelFormat().0 as u64,
        };
        Some(ExportedFrame {
            size: textures.size,
            image: ExportedImage::IoSurface { surface: CFRetained::as_ptr(surface).as_ptr() as usize as u64 },
            texture: native(textures.textures.get(slot)?),
            depth: textures
                .depth_textures
                .get(slot)
                .map(|texture| FrameDepth { texture: native(texture), export: None }),
        })
    }
}
