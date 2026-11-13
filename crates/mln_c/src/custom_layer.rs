//! Custom layers: the host's `mln_custom_layer_host` as the style's
//! `CustomLayerHost`, which hands the callbacks the backend's commands as the
//! header's variants.

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mln_style::layers::{
    CustomLayerFrame, CustomLayerHost, CustomLayerPrerenderParameters, CustomLayerRenderParameters,
};

use crate::callback::Registration;
use crate::generated::types::{
    mln_buffer_view, mln_completion, mln_custom_layer_encoder_data, mln_custom_layer_frame, mln_custom_layer_host,
    mln_custom_layer_pass_data, mln_custom_layer_prerender_callback, mln_custom_layer_prerender_parameters,
    mln_custom_layer_render_callback, mln_custom_layer_render_parameters, mln_custom_layer_session_end_callback,
    mln_diagnostic, mln_map, mln_render_backend, mln_status,
};
use crate::{abi, map, projection, render_session};

abi::versioned!(mln_custom_layer_host);

/// A host's callbacks, which a session runs on its graphics thread.
struct Host {
    registration: Registration,
    prerender: mln_custom_layer_prerender_callback,
    render: mln_custom_layer_render_callback,
    deinitialize: mln_custom_layer_session_end_callback,
    context_lost: mln_custom_layer_session_end_callback,
    /// The scale factor that the caller created the map with, which views
    /// report rather than the runtime's single-precision one.
    scale_factor: f64,
    /// Whether a prerender or render reached the host since the session that
    /// drew the layer last ended, so that the host hears of its end.
    drew: AtomicBool,
}

impl Host {
    fn frame(&self, frame: &CustomLayerFrame) -> mln_custom_layer_frame {
        let mut view = projection::from_map_view(&frame.view);
        view.extent.scale_factor = map::exact_scale_factor(self.scale_factor, view.extent.scale_factor);
        mln_custom_layer_frame {
            view,
            frame_projection: render_session::frame_projection(&frame.projection),
            width: frame.width,
            height: frame.height,
        }
    }

    fn end(&self, callback: mln_custom_layer_session_end_callback) {
        if !self.drew.swap(false, Ordering::AcqRel) {
            return;
        }
        if let Some(callback) = callback
            && !crate::process_exit::exiting()
        {
            // SAFETY: the host's callback, with its user data, as the header
            // promises; it runs on the session's graphics thread.
            unsafe { callback(self.registration.user_data().get()) };
        }
    }
}

impl CustomLayerHost for Host {
    fn prerender(&self, parameters: &mut CustomLayerPrerenderParameters<'_>) {
        let Some((backend, encoder)) = native::encoder(parameters.encoder) else {
            return;
        };
        self.drew.store(true, Ordering::Release);
        let Some(callback) = self.prerender.filter(|_| !crate::process_exit::exiting()) else { return };
        let parameters = mln_custom_layer_prerender_parameters {
            size: size_of::<mln_custom_layer_prerender_parameters>() as u32,
            backend,
            encoder,
            frame: self.frame(parameters.frame),
        };
        // SAFETY: the host's callback, with its user data and parameters that
        // stay valid until it returns.
        unsafe { callback(self.registration.user_data().get(), &raw const parameters) };
    }

    fn render(&self, parameters: &mut CustomLayerRenderParameters<'_>) {
        let Some((backend, pass)) = native::pass(parameters.pass) else {
            return;
        };
        self.drew.store(true, Ordering::Release);
        let Some(callback) = self.render.filter(|_| !crate::process_exit::exiting()) else { return };
        let parameters = mln_custom_layer_render_parameters {
            size: size_of::<mln_custom_layer_render_parameters>() as u32,
            backend,
            pass,
            frame: self.frame(parameters.frame),
            layer_depth: parameters.layer_depth,
        };
        // SAFETY: as for `prerender`.
        unsafe { callback(self.registration.user_data().get(), &raw const parameters) };
    }

    fn deinitialize(&self) {
        self.end(self.deinitialize);
    }

    fn context_lost(&self) {
        self.end(self.context_lost);
    }
}

/// The header's variants of the backends' commands. A backend without one,
/// such as WebGPU outside a browser, gives the host nothing to draw with.
mod native {
    #[cfg(not(any(target_vendor = "apple", target_family = "wasm")))]
    use core::ffi::c_void;

    use super::{Any, mln_custom_layer_encoder_data, mln_custom_layer_pass_data, mln_render_backend};

    #[allow(unused_variables, unused_mut, clippy::needless_pass_by_ref_mut)]
    pub(super) fn encoder(encoder: &mut dyn Any) -> Option<(mln_render_backend, mln_custom_layer_encoder_data)> {
        // SAFETY: zeros are a valid union of pointers and integers.
        let mut data: mln_custom_layer_encoder_data = unsafe { core::mem::zeroed() };
        #[cfg(not(any(target_vendor = "apple", target_family = "wasm")))]
        if let Some(encoder) = encoder.downcast_ref::<mln_vulkan::CustomLayerEncoder>() {
            use mln_vulkan::ash::vk::Handle as _;
            data.vulkan = crate::generated::types::mln_vulkan_custom_layer_encoder {
                context: vulkan_context(&encoder.device),
                command_buffer: encoder.command_buffer.as_raw() as *mut c_void,
            };
            return Some((crate::generated::types::MLN_RENDER_BACKEND_VULKAN, data));
        }
        #[cfg(target_vendor = "apple")]
        if let Some(encoder) = encoder.downcast_ref::<mln_mtl::CustomLayerEncoder>() {
            data.metal = crate::generated::types::mln_metal_custom_layer_encoder {
                command_buffer: mln_mtl::objc2::rc::Retained::as_ptr(&encoder.command_buffer).cast_mut().cast(),
            };
            return Some((crate::generated::types::MLN_RENDER_BACKEND_METAL, data));
        }
        #[cfg(any(all(unix, not(target_vendor = "apple")), all(target_family = "wasm", feature = "gl")))]
        if encoder.is::<mln_gl::CustomLayerEncoder>() {
            return Some((crate::generated::types::MLN_RENDER_BACKEND_OPEN_GL, data));
        }
        None
    }

    #[allow(unused_variables, unused_mut, clippy::needless_pass_by_ref_mut)]
    pub(super) fn pass(pass: &mut dyn Any) -> Option<(mln_render_backend, mln_custom_layer_pass_data)> {
        // SAFETY: zeros are a valid union of pointers and integers.
        let mut data: mln_custom_layer_pass_data = unsafe { core::mem::zeroed() };
        #[cfg(not(any(target_vendor = "apple", target_family = "wasm")))]
        if let Some(pass) = pass.downcast_ref::<mln_vulkan::CustomLayerPass>() {
            use mln_vulkan::ash::vk::Handle as _;
            data.vulkan = crate::generated::types::mln_vulkan_custom_layer_pass {
                context: vulkan_context(&pass.device),
                command_buffer: pass.command_buffer.as_raw() as *mut c_void,
                render_pass: pass.render_pass.as_raw(),
                subpass: pass.subpass,
                sample_count: pass.samples.as_raw(),
                color_format: pass.color_format.as_raw() as u32,
                depth_stencil_format: pass.depth_stencil_format.as_raw() as u32,
            };
            return Some((crate::generated::types::MLN_RENDER_BACKEND_VULKAN, data));
        }
        #[cfg(target_vendor = "apple")]
        if let Some(pass) = pass.downcast_ref::<mln_mtl::CustomLayerPass>() {
            use mln_mtl::objc2::rc::Retained;
            let format =
                |format: Option<mln_mtl::objc2_metal::MTLPixelFormat>| format.map_or(0, |format| format.0 as u64);
            data.metal = crate::generated::types::mln_metal_custom_layer_pass {
                encoder: Retained::as_ptr(&pass.encoder).cast_mut().cast(),
                command_buffer: Retained::as_ptr(&pass.command_buffer).cast_mut().cast(),
                color_format: pass.formats.color.0 as u64,
                depth_format: format(pass.formats.depth),
                stencil_format: format(pass.formats.stencil),
                sample_count: pass.sample_count as u32,
            };
            return Some((crate::generated::types::MLN_RENDER_BACKEND_METAL, data));
        }
        #[cfg(any(all(unix, not(target_vendor = "apple")), all(target_family = "wasm", feature = "gl")))]
        if let Some(pass) = pass.downcast_ref::<mln_gl::CustomLayerPass>() {
            data.open_gl = crate::generated::types::mln_open_gl_custom_layer_pass {
                // A browser session draws into its canvas: the default
                // framebuffer.
                #[cfg(not(target_family = "wasm"))]
                framebuffer: pass.framebuffer.map_or(0, |framebuffer| framebuffer.0.get()),
                #[cfg(target_family = "wasm")]
                framebuffer: 0,
            };
            return Some((crate::generated::types::MLN_RENDER_BACKEND_OPEN_GL, data));
        }
        None
    }

    /// The header's descriptor of a Vulkan device, as
    /// `mln_gpu_device_get_vulkan_context()` reports it.
    #[cfg(not(any(target_vendor = "apple", target_family = "wasm")))]
    fn vulkan_context(device: &mln_vulkan::RawDevice) -> crate::generated::types::mln_vulkan_context_descriptor {
        use mln_vulkan::ash::vk::Handle as _;
        crate::generated::types::mln_vulkan_context_descriptor {
            instance: device.instance.as_raw() as *mut c_void,
            physical_device: device.physical_device.as_raw() as *mut c_void,
            device: device.device.as_raw() as *mut c_void,
            graphics_queue: device.queue.as_raw() as *mut c_void,
            graphics_queue_family_index: device.queue_family_index,
            get_instance_proc_addr: device.get_instance_proc_addr as *mut c_void,
            get_device_proc_addr: device.get_device_proc_addr as *mut c_void,
        }
    }
}

/// `mln_map_add_custom_layer()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mln_map_add_custom_layer(
    map: mln_map,
    layer_id: mln_buffer_view,
    before_layer_id: mln_buffer_view,
    host: *const mln_custom_layer_host,
    completion: *const mln_completion,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        // SAFETY: the caller's contract, for each view.
        let (id, before) = unsafe {
            (
                crate::style::required_text(layer_id, "layer_id")?,
                crate::style::optional_text(before_layer_id, "before_layer_id")?,
            )
        };
        // SAFETY: the caller's contract.
        let registration = unsafe { abi::read_sized(host) }?;
        let scale_factor = map::resolve(map)?.scale_factor;
        let host = Arc::new(Host {
            registration: Registration::new(
                "custom layer host",
                true,
                registration.user_data,
                registration.release_user_data,
                crate::handles::NULL,
            )?,
            prerender: registration.prerender,
            render: registration.render,
            deinitialize: registration.deinitialize,
            context_lost: registration.context_lost,
            scale_factor,
            drew: AtomicBool::new(false),
        });
        host.registration.accept();
        let layer_host: Arc<dyn CustomLayerHost> = host.clone();
        // SAFETY: the caller's contract.
        let sent = unsafe { map::command(map, completion, |map| map.add_custom_layer(id, layer_host, before)) };
        if sent.is_err() {
            // This handle outlives the copies that the failed call dropped,
            // so the registration drops unaccepted.
            host.registration.revoke();
        }
        sent
    })
}
