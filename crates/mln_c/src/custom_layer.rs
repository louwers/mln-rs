//! Custom layers: the library's `custom` layer type, whose layers a host's
//! `mln_custom_layer_host` callbacks draw with the backend's commands, which
//! the frame lends the layer and the callbacks get as the header's variants.

use std::any::Any;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};

use mln_rs::layer::{
    AfterGpu, Draws, EvaluateCx, Evaluation, GpuEnd, Layer, LayerStyle, LayerTypeInfo, LendAt, Lent, NewCx,
    NoProperties, NoTiles, PrepareCx, PropertyTable, PropertyVisitor, SourceKind, StyleLayerType,
};
use mln_rs::mln_render::{Registry, RegistryBuilder};
use mln_rs::mln_style::layer::Layer as StyleLayer;

use crate::callback::Registration;
use crate::generated::types::{
    mln_buffer_view, mln_completion, mln_custom_layer_encoder_data, mln_custom_layer_frame, mln_custom_layer_host,
    mln_custom_layer_pass_data, mln_custom_layer_prerender_callback, mln_custom_layer_prerender_parameters,
    mln_custom_layer_render_callback, mln_custom_layer_render_parameters, mln_custom_layer_session_end_callback,
    mln_diagnostic, mln_map, mln_render_backend, mln_status,
};
use crate::{abi, map, projection, render_session};

abi::versioned!(mln_custom_layer_host);

/// The registry of the library's runtimes and tile workers: the built-ins,
/// and the `custom` layer type.
pub(crate) fn registry() -> Arc<Registry> {
    static REGISTRY: LazyLock<Arc<Registry>> = LazyLock::new(|| {
        let mut builder = RegistryBuilder::new();
        // An empty builder has no name the built-ins or `custom` could clash
        // with.
        let registered = mln_rs::register_builtins(&mut builder).and_then(|()| builder.add_layer_type::<CustomLayer>());
        debug_assert_eq!(registered, Ok(()));
        builder.build()
    });
    Arc::clone(&REGISTRY)
}

/// The style half of the `custom` layer type. Its layers aren't made from
/// style JSON: `mln_map_add_custom_layer()` makes one, whose data is its
/// host.
struct Custom;

static PROPERTIES: PropertyTable = PropertyTable::new(&[], &[]);

impl StyleLayerType for Custom {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "custom", source: SourceKind::None, from_json: false };
    const PROPERTIES: &'static PropertyTable = &PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = NoProperties;
    type Data = Option<Arc<Host>>;

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(_slot: usize, _visitor: V) -> Option<V::Output> {
        None
    }
}

/// A custom layer as a renderer draws it: through its host, whose session
/// ends once the GPU finished what it drew.
struct CustomLayer {
    host: Option<Arc<Host>>,
    after_gpu: AfterGpu,
}

impl CustomLayer {
    /// Lets go of the host, whose session ends once the GPU finished what it
    /// drew, if it drew since its session began.
    fn end(&mut self) {
        let Some(host) = self.host.take().filter(|host| host.drew.swap(false, Ordering::AcqRel)) else { return };
        self.after_gpu.push(move |end| host.end(end));
    }
}

impl Layer for CustomLayer {
    type Style = Custom;
    type Input = NoTiles;
    type Tile = ();

    fn new(cx: &mut NewCx<'_, Self>) -> Self {
        CustomLayer { host: cx.layer.style.data.clone(), after_gpu: cx.after_gpu.clone() }
    }

    /// A new host of the layer's id is a new layer to the host: the old one
    /// ends.
    fn evaluate(&mut self, cx: EvaluateCx<'_, Self>) -> Evaluation {
        let host = &cx.layer.style.data;
        if !matches!((&self.host, host), (Some(drawn), Some(host)) if Arc::ptr_eq(drawn, host)) {
            self.end();
            self.host.clone_from(host);
        }
        Evaluation::TRANSLUCENT
    }

    fn prepare(&self, _cx: &PrepareCx<'_, Self>, out: &mut Draws) {
        out.lend(LendAt::BeforePasses).lend(LendAt::Pass);
    }

    fn lend(&mut self, _at: LendAt, lent: Lent<'_>) {
        if let Some(host) = &self.host {
            host.draw(lent);
        }
    }
}

impl Drop for CustomLayer {
    fn drop(&mut self) {
        self.end();
    }
}

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
    /// Whether a prerender or render reached the host since its session
    /// began, so that the host hears of its end.
    drew: AtomicBool,
}

impl Host {
    /// Draws with `lent`: a prerender before the frame's passes, which lend
    /// no layer depth, and a render in the pass.
    fn draw(&self, lent: Lent<'_>) {
        let frame = self.frame(&lent);
        match lent.layer_depth {
            None => self.prerender(lent.raw, frame),
            Some(layer_depth) => self.render(lent.raw, frame, layer_depth),
        }
    }

    fn frame(&self, lent: &Lent<'_>) -> mln_custom_layer_frame {
        let frame = lent.frame;
        let mut view = projection::from_map_view(&frame.camera.map_view(f64::from(frame.pixel_ratio)));
        view.extent.scale_factor = map::exact_scale_factor(self.scale_factor, view.extent.scale_factor);
        mln_custom_layer_frame {
            view,
            frame_projection: render_session::frame_projection(lent.projection),
            width: frame.target_size.width,
            height: frame.target_size.height,
        }
    }

    fn prerender(&self, encoder: &mut dyn Any, frame: mln_custom_layer_frame) {
        let Some((backend, encoder)) = native::encoder(encoder) else {
            return;
        };
        self.drew.store(true, Ordering::Release);
        let Some(callback) = self.prerender.filter(|_| !crate::process_exit::exiting()) else { return };
        let parameters = mln_custom_layer_prerender_parameters {
            size: size_of::<mln_custom_layer_prerender_parameters>() as u32,
            backend,
            encoder,
            frame,
        };
        // SAFETY: the host's callback, with its user data and parameters that
        // stay valid until it returns.
        unsafe { callback(self.registration.user_data().get(), &raw const parameters) };
    }

    fn render(&self, pass: &mut dyn Any, frame: mln_custom_layer_frame, layer_depth: f64) {
        let Some((backend, pass)) = native::pass(pass) else {
            return;
        };
        self.drew.store(true, Ordering::Release);
        let Some(callback) = self.render.filter(|_| !crate::process_exit::exiting()) else { return };
        let parameters = mln_custom_layer_render_parameters {
            size: size_of::<mln_custom_layer_render_parameters>() as u32,
            backend,
            pass,
            frame,
            layer_depth,
        };
        // SAFETY: as for `prerender`.
        unsafe { callback(self.registration.user_data().get(), &raw const parameters) };
    }

    /// Ends the host's session, as `end` says the GPU's work ended:
    /// `deinitialize` once it finished, `context_lost` once it was lost.
    fn end(&self, end: GpuEnd) {
        let callback = match end {
            GpuEnd::Finished => self.deinitialize,
            GpuEnd::Lost => self.context_lost,
        };
        if let Some(callback) = callback
            && !crate::process_exit::exiting()
        {
            // SAFETY: the host's callback, with its user data, as the header
            // promises; it runs on the session's graphics thread.
            unsafe { callback(self.registration.user_data().get()) };
        }
    }
}

/// The header's variants of the backends' commands. A backend without one,
/// such as WebGPU outside a browser, gives the host nothing to draw with.
mod native {
    #[cfg(any(backend_vulkan, backend_d3d12))]
    use core::ffi::c_void;

    use super::{Any, mln_custom_layer_encoder_data, mln_custom_layer_pass_data, mln_render_backend};

    #[cfg_attr(
        not(any(
            backend_vulkan,
            backend_d3d12,
            target_vendor = "apple",
            backend_gl,
            all(target_family = "wasm", feature = "gl")
        )),
        expect(unused_variables, reason = "the build has no backend with commands for the host")
    )]
    #[cfg_attr(
        not(any(backend_vulkan, backend_d3d12, target_vendor = "apple")),
        expect(unused_mut, reason = "only Vulkan, D3D12 and Metal fill in the data")
    )]
    pub(super) fn encoder(encoder: &mut dyn Any) -> Option<(mln_render_backend, mln_custom_layer_encoder_data)> {
        // SAFETY: zeros are a valid union of pointers and integers.
        let mut data: mln_custom_layer_encoder_data = unsafe { core::mem::zeroed() };
        #[cfg(backend_vulkan)]
        if let Some(encoder) = encoder.downcast_ref::<mln_vulkan::CustomLayerEncoder>() {
            use mln_vulkan::ash::vk::Handle as _;
            data.vulkan = crate::generated::types::mln_vulkan_custom_layer_encoder {
                context: vulkan_context(&encoder.device),
                command_buffer: encoder.command_buffer.as_raw() as *mut c_void,
            };
            return Some((crate::generated::types::MLN_RENDER_BACKEND_VULKAN, data));
        }
        #[cfg(backend_d3d12)]
        if let Some(encoder) = encoder.downcast_ref::<mln_d3d12::CustomLayerEncoder>() {
            use mln_d3d12::windows::core::Interface as _;
            data.d3d12 = crate::generated::types::mln_d3d12_custom_layer_encoder {
                device: encoder.device.as_raw(),
                command_list: encoder.command_list.as_raw(),
            };
            return Some((crate::generated::types::MLN_RENDER_BACKEND_D3D12, data));
        }
        #[cfg(target_vendor = "apple")]
        if let Some(encoder) = encoder.downcast_ref::<mln_mtl::CustomLayerEncoder>() {
            data.metal = crate::generated::types::mln_metal_custom_layer_encoder {
                command_buffer: objc2::rc::Retained::as_ptr(&encoder.command_buffer).cast_mut().cast(),
            };
            return Some((crate::generated::types::MLN_RENDER_BACKEND_METAL, data));
        }
        #[cfg(any(backend_gl, all(target_family = "wasm", feature = "gl")))]
        if encoder.is::<mln_gl::CustomLayerEncoder>() {
            return Some((crate::generated::types::MLN_RENDER_BACKEND_OPEN_GL, data));
        }
        None
    }

    #[cfg_attr(
        not(any(
            backend_vulkan,
            backend_d3d12,
            target_vendor = "apple",
            backend_gl,
            all(target_family = "wasm", feature = "gl")
        )),
        expect(unused_variables, unused_mut, reason = "the build has no backend with commands for the host")
    )]
    pub(super) fn pass(pass: &mut dyn Any) -> Option<(mln_render_backend, mln_custom_layer_pass_data)> {
        // SAFETY: zeros are a valid union of pointers and integers.
        let mut data: mln_custom_layer_pass_data = unsafe { core::mem::zeroed() };
        #[cfg(backend_vulkan)]
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
        #[cfg(backend_d3d12)]
        if let Some(pass) = pass.downcast_ref::<mln_d3d12::CustomLayerPass>() {
            use mln_d3d12::windows::core::Interface as _;
            data.d3d12 = crate::generated::types::mln_d3d12_custom_layer_pass {
                device: pass.device.as_raw(),
                command_list: pass.command_list.as_raw(),
                render_target_view: pass.render_target.ptr as u64,
                color_format: pass.color_format.0 as u32,
                depth_stencil_view: pass.depth_stencil.map_or(0, |view| view.ptr as u64),
                depth_stencil_format: pass.depth_stencil_format.0 as u32,
            };
            return Some((crate::generated::types::MLN_RENDER_BACKEND_D3D12, data));
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
        #[cfg(any(backend_gl, all(target_family = "wasm", feature = "gl")))]
        #[cfg_attr(
            target_family = "wasm",
            expect(unused_variables, reason = "a browser session draws into its canvas: the default framebuffer")
        )]
        if let Some(pass) = pass.downcast_ref::<mln_gl::CustomLayerPass>() {
            data.open_gl = crate::generated::types::mln_open_gl_custom_layer_pass {
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
    #[cfg(backend_vulkan)]
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
        let layer = Arc::clone(&host);
        // SAFETY: the caller's contract.
        let sent = unsafe {
            map::command(map, completion, |map| {
                let mut style = LayerStyle::<Custom>::new(id, String::new());
                style.data = Some(layer);
                let functions = Arc::clone(map.registry().style().functions());
                map.add_style_layer(StyleLayer::from_style(Arc::new(style), functions), before)
            })
        };
        if sent.is_err() {
            // This handle outlives the copies that the failed call dropped,
            // so the registration drops unaccepted.
            host.registration.revoke();
        }
        sent
    })
}
