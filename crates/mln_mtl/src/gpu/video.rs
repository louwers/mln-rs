//! Video frames in IOSurface-backed `CVPixelBuffer`s, as textures over their
//! planes.

use std::ffi::c_void;
use std::sync::Arc;

use mln_gfx::gpu::{GpuError, TextureFormat, TextureId};
use mln_gfx::video::{FrameKeepAlive, ImportedFrame, PlaneLayout, YuvColor, YuvMatrix, YuvRange};
use mln_util::size::Size;
use objc2_core_foundation::CFString;
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetHeight, CVPixelBufferGetHeightOfPlane, CVPixelBufferGetIOSurface,
    CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth, CVPixelBufferGetWidthOfPlane,
    kCVImageBufferYCbCrMatrix_ITU_R_601_4, kCVImageBufferYCbCrMatrix_ITU_R_2020, kCVImageBufferYCbCrMatrixKey,
    kCVPixelFormatType_32BGRA, kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
    kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
};
use objc2_foundation::NSString;
use objc2_metal::{MTLDevice, MTLResource, MTLTextureUsage};

use super::{MetalGpu, Texture, pixel_format, texture_descriptor};

impl MetalGpu {
    /// Textures over the planes of `buffer`, a retained `CVPixelBufferRef`,
    /// which hold `keep_alive` until destroyed and done with: BGRA as one
    /// plane, biplanar 4:2:0 YUV as luma and chroma. Fails with
    /// [`GpuError::Unsupported`] for a buffer without an IOSurface or of
    /// another pixel format.
    pub(super) fn import_pixel_buffer(
        &mut self,
        buffer: *mut c_void,
        color: YuvColor,
        keep_alive: &FrameKeepAlive,
    ) -> Result<ImportedFrame, GpuError> {
        if buffer.is_null() {
            return Err(GpuError::Invalid("a null CVPixelBuffer".to_owned()));
        }
        // SAFETY: a `GpuFrame::CvPixelBuffer` holds a retained, valid
        // `CVPixelBufferRef`, which the frame outlives this call by.
        let buffer = unsafe { &*buffer.cast::<CVPixelBuffer>() };
        let surface = CVPixelBufferGetIOSurface(Some(buffer))
            .ok_or_else(|| GpuError::Unsupported("importing a CVPixelBuffer without an IOSurface".to_owned()))?;
        let format = CVPixelBufferGetPixelFormatType(buffer);
        let (layout, color, planes): (_, _, &[TextureFormat]) = if format == kCVPixelFormatType_32BGRA {
            (PlaneLayout::Rgba, color, &[TextureFormat::Bgra8Unorm])
        } else if format == kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
            || format == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
        {
            let range = if format == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange {
                YuvRange::Full
            } else {
                YuvRange::Limited
            };
            let color = YuvColor { matrix: yuv_matrix(buffer).unwrap_or(color.matrix), range };
            (PlaneLayout::Nv12, color, &[TextureFormat::R8Unorm, TextureFormat::Rg8Unorm])
        } else {
            return Err(GpuError::Unsupported(format!(
                "importing CVPixelBuffers of pixel format {:?}",
                String::from_utf8_lossy(&format.to_be_bytes())
            )));
        };
        let mut ids: Vec<TextureId> = Vec::with_capacity(planes.len());
        for (plane, &texture_format) in planes.iter().enumerate() {
            let size = if planes.len() == 1 {
                Size::new(CVPixelBufferGetWidth(buffer) as u32, CVPixelBufferGetHeight(buffer) as u32)
            } else {
                Size::new(
                    CVPixelBufferGetWidthOfPlane(buffer, plane) as u32,
                    CVPixelBufferGetHeightOfPlane(buffer, plane) as u32,
                )
            };
            if size.is_empty() {
                return Err(GpuError::Invalid("a CVPixelBuffer plane without pixels".to_owned()));
            }
            let descriptor = texture_descriptor(pixel_format(texture_format), size);
            descriptor.setUsage(MTLTextureUsage::ShaderRead);
            let texture = self.device.device().newTextureWithDescriptor_iosurface_plane(&descriptor, &surface, plane);
            let Some(texture) = texture else {
                for id in ids {
                    self.textures.remove(id);
                }
                return Err(GpuError::Invalid(format!("Metal made no texture over plane {plane} of a CVPixelBuffer")));
            };
            texture.setLabel(Some(&NSString::from_str("video frame plane")));
            ids.push(self.textures.insert(Texture {
                texture,
                format: texture_format,
                waits: Vec::new(),
                keep_alive: Some(Arc::clone(keep_alive)),
            }));
        }
        Ok(ImportedFrame { layout, planes: ids, color, external: false })
    }
}

/// The matrix that `buffer`'s attachment names, if it names one the
/// renderer knows.
fn yuv_matrix(buffer: &CVPixelBuffer) -> Option<YuvMatrix> {
    // SAFETY: the keys are CoreVideo's constant strings, and a null mode
    // pointer asks for no mode.
    let (key, bt601, bt2020) = unsafe {
        (kCVImageBufferYCbCrMatrixKey, kCVImageBufferYCbCrMatrix_ITU_R_601_4, kCVImageBufferYCbCrMatrix_ITU_R_2020)
    };
    // SAFETY: as above.
    let value = unsafe { buffer.attachment(key, std::ptr::null_mut()) }?;
    let name = value.downcast_ref::<CFString>()?;
    Some(if name == bt601 {
        YuvMatrix::Bt601
    } else if name == bt2020 {
        YuvMatrix::Bt2020
    } else {
        YuvMatrix::Bt709
    })
}

#[cfg(test)]
mod tests {
    use std::ptr::NonNull;
    use std::sync::Arc;

    use mln_gfx::gpu::{ColorAttachment, Gpu, LoadOp, PassDesc, ResourceId, TextureDesc, TextureUsage};
    use mln_gfx::video::{FrameKeepAlive, GpuFrame};
    use mln_util::Color;
    use objc2_core_foundation::{CFBoolean, CFDictionary, CFType};
    use objc2_core_video::{
        CVPixelBufferCreate, kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey,
        kCVPixelFormatType_422YpCbCr8, kCVReturnSuccess,
    };

    use super::*;
    use crate::Device;

    /// A frame of an IOSurface-backed pixel buffer of `format`.
    fn frame(format: u32) -> GpuFrame {
        let surface = CFDictionary::<CFString, CFType>::empty();
        // SAFETY: CoreVideo's constant keys.
        let keys = unsafe { [kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey] };
        let values: [&CFType; 2] = [surface.as_ref(), CFBoolean::new(true).as_ref()];
        let attributes = CFDictionary::<CFString, CFType>::from_slices(&keys, &values);
        let mut buffer: *mut CVPixelBuffer = std::ptr::null_mut();
        // SAFETY: the attributes map strings to CF objects, and the out
        // pointer is valid.
        let status = unsafe {
            CVPixelBufferCreate(None, 8, 8, format, Some(attributes.as_opaque()), NonNull::from(&mut buffer))
        };
        assert_eq!(kCVReturnSuccess, status);
        // The frame owns the buffer's retain.
        GpuFrame::CvPixelBuffer { buffer: buffer.cast(), color: YuvColor::default() }
    }

    // An imported plane keeps its frame while a command buffer that may read
    // it hasn't finished, though destroyed.
    #[test]
    fn imported_planes_keep_their_frame_until_the_gpu_is_done() {
        let device = Device::new(None).expect("Metal device");
        let mut gpu = MetalGpu::new(&device);
        let keep_alive: FrameKeepAlive = Arc::new(());
        let held = Arc::downgrade(&keep_alive);
        let imported =
            gpu.import_video_frame(&frame(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange), keep_alive).unwrap();
        assert_eq!(PlaneLayout::Nv12, imported.layout);
        assert_eq!(YuvRange::Limited, imported.color.range);
        assert_eq!(2, imported.planes.len());

        // A frame's command buffer, not yet committed, is open while the
        // planes go.
        let target = gpu
            .create_texture(&TextureDesc {
                label: "target",
                size: Size::new(4, 4),
                format: TextureFormat::Rgba8Unorm,
                usage: TextureUsage { render_target: true, ..Default::default() },
            })
            .unwrap();
        gpu.begin_pass(&PassDesc {
            label: "clear",
            color: Some(ColorAttachment { texture: target, load: LoadOp::Clear(Color::black()) }),
            depth_stencil: None,
        })
        .unwrap();
        gpu.end_pass().unwrap();
        for plane in imported.planes {
            gpu.destroy(ResourceId::Texture(plane));
        }
        gpu.retire();
        assert!(held.upgrade().is_some(), "the uncommitted frame may read the planes");
        gpu.flush(true).unwrap();
        assert!(held.upgrade().is_none(), "the GPU is done with the planes");
    }

    #[test]
    fn other_pixel_formats_are_unsupported() {
        let device = Device::new(None).expect("Metal device");
        let mut gpu = MetalGpu::new(&device);
        let result = gpu.import_video_frame(&frame(kCVPixelFormatType_422YpCbCr8), Arc::new(()));
        assert!(matches!(result, Err(GpuError::Unsupported(_))), "{result:?}");
    }
}
