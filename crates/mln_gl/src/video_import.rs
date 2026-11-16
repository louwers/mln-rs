// Video frames in dmabufs as GL textures: each plane becomes an EGLImage of
// a one- or two-channel DRM format (EGL_EXT_image_dma_buf_import, with
// _modifiers for tiled memory), which a GL_TEXTURE_2D texture samples, as
// mpv and GStreamer import VA-API's frames. The driver keeps the dmabuf's
// implicit fences, so the decoder doesn't write a frame the GPU still reads.

use std::ffi::c_void;
use std::os::fd::AsRawFd as _;

use khronos_egl as egl;
use mln_gfx::gpu::GpuError;
use mln_gfx::video::{DmaBufPlane, PlaneLayout};

use crate::egl::Egl;

type CreateImage =
    unsafe extern "system" fn(*mut c_void, *mut c_void, egl::Enum, *mut c_void, *const egl::Int) -> *mut c_void;
type DestroyImage = unsafe extern "system" fn(*mut c_void, *mut c_void) -> egl::Boolean;
type ImageTargetTexture = unsafe extern "system" fn(u32, *mut c_void);

// EGL_EXT_image_dma_buf_import(_modifiers), spelled out as in
// exported_target/dma_buf.rs.
const LINUX_DMA_BUF: egl::Enum = 0x3270;
const LINUX_DRM_FOURCC: egl::Int = 0x3271;
const PLANE0_FD: egl::Int = 0x3272;
const PLANE0_OFFSET: egl::Int = 0x3273;
const PLANE0_PITCH: egl::Int = 0x3274;
const PLANE0_MODIFIER_LO: egl::Int = 0x3443;
const PLANE0_MODIFIER_HI: egl::Int = 0x3444;

const fn fourcc(code: [u8; 4]) -> u32 {
    u32::from_le_bytes(code)
}

/// `DRM_FORMAT_NV12`: a luma plane and an interleaved `CbCr` plane.
pub(crate) const DRM_FORMAT_NV12: u32 = fourcc(*b"NV12");
const DRM_FORMAT_R8: u32 = fourcc(*b"R8  ");
const DRM_FORMAT_GR88: u32 = fourcc(*b"GR88");
const DRM_FORMAT_ARGB8888: u32 = fourcc(*b"AR24");
const DRM_FORMAT_XRGB8888: u32 = fourcc(*b"XR24");
const DRM_FORMAT_ABGR8888: u32 = fourcc(*b"AB24");
const DRM_FORMAT_XBGR8888: u32 = fourcc(*b"XB24");
/// `DRM_FORMAT_MOD_INVALID`: the memory's layout is the driver's to know.
const DRM_FORMAT_MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;

/// How a frame of `fourcc` lays out its planes, and each plane's one- or
/// two-channel DRM format with the divisor of its size.
pub(crate) fn plane_formats(fourcc: u32) -> Option<(PlaneLayout, Vec<(u32, u32)>)> {
    match fourcc {
        DRM_FORMAT_NV12 => Some((PlaneLayout::Nv12, vec![(DRM_FORMAT_R8, 1), (DRM_FORMAT_GR88, 2)])),
        DRM_FORMAT_ARGB8888 | DRM_FORMAT_XRGB8888 | DRM_FORMAT_ABGR8888 | DRM_FORMAT_XBGR8888 => {
            Some((PlaneLayout::Rgba, vec![(fourcc, 1)]))
        }
        _ => None,
    }
}

/// The current display's `EGLImage`s of dmabufs, and the GL function that
/// makes a texture of one.
pub(crate) struct DmaBufImages {
    display: *mut c_void,
    create_image: CreateImage,
    destroy_image: DestroyImage,
    image_target_texture: ImageTargetTexture,
    /// `EGL_EXT_image_dma_buf_import_modifiers`, without which only memory
    /// of an implicit layout imports.
    modifiers: bool,
}

impl DmaBufImages {
    /// The functions of the display of the current context, or `None` when
    /// it doesn't import dmabufs.
    pub(crate) fn load() -> Option<DmaBufImages> {
        let egl: &'static Egl = crate::egl::library().ok()?;
        let display = egl.get_current_display()?;
        let extensions = egl.query_string(Some(display), egl::EXTENSIONS).ok()?.to_string_lossy().into_owned();
        let has = |name: &str| extensions.split(' ').any(|extension| extension == name);
        if !has("EGL_KHR_image_base") || !has("EGL_EXT_image_dma_buf_import") {
            return None;
        }
        let lookup = |name: &str| egl.get_proc_address(name).map(|function| function as *const c_void);
        // SAFETY: each function has its extension's type.
        unsafe {
            Some(DmaBufImages {
                display: display.as_ptr(),
                create_image: std::mem::transmute::<*const c_void, CreateImage>(lookup("eglCreateImageKHR")?),
                destroy_image: std::mem::transmute::<*const c_void, DestroyImage>(lookup("eglDestroyImageKHR")?),
                image_target_texture: std::mem::transmute::<*const c_void, ImageTargetTexture>(lookup(
                    "glEGLImageTargetTexture2DOES",
                )?),
                modifiers: has("EGL_EXT_image_dma_buf_import_modifiers"),
            })
        }
    }

    /// An image of `plane`, of `width` by `height` texels of `format`, laid
    /// out by `modifier`.
    pub(crate) fn create(
        &self,
        plane: &DmaBufPlane,
        format: u32,
        modifier: u64,
        width: u32,
        height: u32,
    ) -> Result<*mut c_void, GpuError> {
        let mut attributes = vec![
            egl::WIDTH,
            width as egl::Int,
            egl::HEIGHT,
            height as egl::Int,
            LINUX_DRM_FOURCC,
            format as egl::Int,
            PLANE0_FD,
            plane.fd.as_raw_fd(),
            PLANE0_OFFSET,
            plane.offset as egl::Int,
            PLANE0_PITCH,
            plane.stride as egl::Int,
        ];
        if modifier != DRM_FORMAT_MOD_INVALID {
            if !self.modifiers {
                return Err(GpuError::Unsupported(format!("dmabufs with the modifier {modifier:#x}")));
            }
            attributes.extend([
                PLANE0_MODIFIER_LO,
                (modifier & 0xffff_ffff) as egl::Int,
                PLANE0_MODIFIER_HI,
                (modifier >> 32) as egl::Int,
            ]);
        }
        attributes.push(egl::NONE);
        // SAFETY: the attributes end with EGL_NONE and name an open dmabuf,
        // which EGL dups.
        let image = unsafe {
            (self.create_image)(
                self.display,
                std::ptr::null_mut(),
                LINUX_DMA_BUF,
                std::ptr::null_mut(),
                attributes.as_ptr(),
            )
        };
        if image.is_null() {
            return Err(GpuError::Unsupported(format!(
                "importing a {width}x{height} dmabuf plane of {format:#x} with the modifier {modifier:#x}"
            )));
        }
        Ok(image)
    }

    /// Makes the texture bound to `GL_TEXTURE_2D` sample `image`.
    ///
    /// # Safety
    ///
    /// The display's context is current with a texture bound.
    pub(crate) unsafe fn bind(&self, image: *mut c_void) {
        // SAFETY: the caller's contract.
        unsafe { (self.image_target_texture)(glow::TEXTURE_2D, image) };
    }

    /// Destroys `image`, which the textures over it outlive.
    pub(crate) fn destroy(&self, image: *mut c_void) {
        // SAFETY: the image is the display's, and nothing else destroys it.
        unsafe { (self.destroy_image)(self.display, image) };
    }
}
