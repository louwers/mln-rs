//! Custom layers: the library's own `custom` layer type, whose layers a
//! host's `mln_custom_layer_host` callbacks draw with the backend's
//! commands, which the frame lends the layer and the callbacks get as the
//! header's variants.

// How a custom layer's host hears that its session ended: a host that drew
// through a renderer's layer hears it once the layer lets go of it, as the
// layer leaves the renderer or the style gives its id a new host. The layer
// queues the end for after the GPU (`AfterGpu`), so that `deinitialize` runs
// once the GPU finished what the host drew, and `context_lost` instead when
// the renderer's graphics objects are lost.

use std::any::Any;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use mln_engine::draw::{AfterGpu, Draws, GpuEnd, LendAt, Lent};
use mln_engine::layer::{EvaluateCx, Evaluation, FrameTile, Layer, LayerValues, NoTiles, PrepareCx};
use mln_style::expression::functions::Functions;
use mln_style::layer::{Layer as StyleLayer, LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use mln_style::property::table::{PropertyTable, PropertyVisitor};

use crate::callback::Registration;
use crate::generated::types::{
    mln_buffer_view, mln_completion, mln_custom_layer_encoder_data, mln_custom_layer_frame, mln_custom_layer_host,
    mln_custom_layer_pass_data, mln_custom_layer_prerender_callback, mln_custom_layer_prerender_parameters,
    mln_custom_layer_render_callback, mln_custom_layer_render_parameters, mln_custom_layer_session_end_callback,
    mln_diagnostic, mln_map, mln_render_backend, mln_status,
};
use crate::{abi, map, projection, render_session};

abi::versioned!(mln_custom_layer_host);

/// The style half of the `custom` layer type. Its layers aren't made from
/// style JSON: `mln_map_add_custom_layer()` makes one ([`layer`]).
pub(crate) struct Custom;

static PROPERTIES: PropertyTable = PropertyTable::new(&[], &[]);

impl StyleLayerType for Custom {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "custom", source: SourceKind::None, from_json: false };
    const PROPERTIES: &'static PropertyTable = &PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = NoProperties;
    type Extra = HostRef;

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(_slot: usize, _visitor: V) -> Option<V::Output> {
        None
    }
}

/// The host of a custom layer, as its style data refers to it: there while
/// the style has the layer or a renderer draws it. A renderer that is
/// leaked with its style data, because its graphics objects are gone,
/// doesn't keep it, so its user data is still released.
#[derive(Clone, Default)]
pub(crate) struct HostRef(Weak<Host>);

impl HostRef {
    /// The host, unless the style no longer has the layer and no renderer
    /// draws it.
    fn get(&self) -> Option<Arc<Host>> {
        self.0.upgrade()
    }

    /// Whether `host` is this one.
    fn is(&self, host: &Arc<Host>) -> bool {
        std::ptr::eq(self.0.as_ptr(), Arc::as_ptr(host))
    }
}

/// A layer named `id` that `host` draws, whose filter and visibility parse
/// against `functions`. The layer holds `host`, and its style data only
/// refers to it.
fn layer(functions: Arc<Functions>, id: String, host: Arc<Host>) -> StyleLayer {
    let mut style = LayerStyle::<Custom>::new(id, String::new());
    style.extra = HostRef(Arc::downgrade(&host));
    StyleLayer::from_style_holding(Arc::new(style), functions, host)
}

/// The render half of a custom layer, which keeps no tiles and pushes no
/// draws: what it draws is what its host draws with the commands the frame
/// lends it.
pub(crate) struct CustomLayer {
    /// The host, while the renderer draws through it.
    host: Option<Arc<Host>>,
    /// Where the host's end waits for the GPU, once a frame lent the host
    /// its commands through this layer: a host that drew hears that the
    /// renderer stopped.
    drew: Option<AfterGpu>,
}

impl CustomLayer {
    /// Lets go of the host, which hears that the renderer stopped once the
    /// GPU finished what it drew, if it drew.
    fn end(&mut self) {
        if let (Some(host), Some(after_gpu)) = (self.host.take(), self.drew.take()) {
            after_gpu.push(move |end| host.end(end));
        }
    }
}

impl Drop for CustomLayer {
    /// The layer leaves the renderer.
    fn drop(&mut self) {
        self.end();
    }
}

impl Layer for CustomLayer {
    type Style = Custom;
    type Input = NoTiles;
    type Tile = ();

    fn new(values: &LayerValues<Custom>) -> Self {
        CustomLayer { host: values.style.extra.get(), drew: None }
    }

    /// A new host for the same layer id is a new layer to the host: the old
    /// one ends.
    fn evaluate(&mut self, cx: EvaluateCx<'_, Self>) -> Evaluation {
        let host = &cx.values.style.extra;
        if !self.host.as_ref().is_some_and(|drawn| host.is(drawn)) {
            self.end();
            self.host = host.get();
        }
        Evaluation::TRANSLUCENT
    }

    /// Asks for the frame's commands before its passes, for the host to
    /// prerender with, and for the translucent pass, for it to render in.
    fn prepare(&mut self, _cx: &mut PrepareCx<'_, '_, Self>, _tiles: &[FrameTile<'_, Self>], out: &mut Draws) {
        out.lend(LendAt::BeforePasses).lend(LendAt::Pass);
    }

    fn lend(&mut self, _at: LendAt, lent: Lent<'_>) {
        let Some(host) = &self.host else { return };
        let Lent { raw, frame, projection: frame_projection, layer_depth, after_gpu } = lent;
        let view = frame.view().map_view(f64::from(frame.pixel_ratio()));
        let frame = mln_custom_layer_frame {
            view: host.view(&view),
            frame_projection: render_session::frame_projection(frame_projection),
            width: frame.size().width,
            height: frame.size().height,
        };
        match layer_depth {
            None => host.prerender(raw, &frame),
            Some(layer_depth) => host.render(raw, &frame, layer_depth),
        }
        self.drew.get_or_insert_with(|| after_gpu.clone());
    }
}

/// A host's callbacks, which a session runs on its graphics thread.
pub(crate) struct Host {
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
    /// The header's view of `view`, with the map's exact scale factor.
    fn view(&self, view: &mln_engine::camera::MapView) -> crate::generated::types::mln_map_view {
        let mut view = projection::from_map_view(view);
        view.extent.scale_factor = map::exact_scale_factor(self.scale_factor, view.extent.scale_factor);
        view
    }

    fn prerender(&self, encoder: &mut dyn Any, frame: &mln_custom_layer_frame) {
        let Some((backend, encoder)) = native::encoder(encoder) else {
            return;
        };
        self.drew.store(true, Ordering::Release);
        let Some(callback) = self.prerender.filter(|_| !crate::process_exit::exiting()) else { return };
        let parameters = mln_custom_layer_prerender_parameters {
            size: size_of::<mln_custom_layer_prerender_parameters>() as u32,
            backend,
            encoder,
            frame: *frame,
        };
        // SAFETY: the host's callback, with its user data and parameters that
        // stay valid until it returns.
        unsafe { callback(self.registration.user_data().get(), &raw const parameters) };
    }

    fn render(&self, pass: &mut dyn Any, frame: &mln_custom_layer_frame, layer_depth: f64) {
        let Some((backend, pass)) = native::pass(pass) else {
            return;
        };
        self.drew.store(true, Ordering::Release);
        let Some(callback) = self.render.filter(|_| !crate::process_exit::exiting()) else { return };
        let parameters = mln_custom_layer_render_parameters {
            size: size_of::<mln_custom_layer_render_parameters>() as u32,
            backend,
            pass,
            frame: *frame,
            layer_depth,
        };
        // SAFETY: as for `prerender`.
        unsafe { callback(self.registration.user_data().get(), &raw const parameters) };
    }

    /// Ends the host's session, if it drew since the last ended, as `end`
    /// says the GPU's work ended: `deinitialize` once it finished,
    /// `context_lost` once it was lost.
    fn end(&self, end: GpuEnd) {
        if !self.drew.swap(false, Ordering::AcqRel) {
            return;
        }
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
    #[cfg(backend_vulkan)]
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
        let object = map::resolve(map)?;
        let (scale_factor, functions) = (object.scale_factor, Arc::clone(object.map.registry().functions()));
        drop(object);
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
        let layer = layer(functions, id, Arc::clone(&host));
        // SAFETY: the caller's contract.
        let sent = unsafe { map::command(map, completion, |map| map.add_style_layer(layer, before)) };
        if sent.is_err() {
            // This handle outlives the copies that the failed call dropped,
            // so the registration drops unaccepted.
            host.registration.revoke();
        }
        sent
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use mln_style::layer::LayerStyle;

    use super::{Custom, Host, layer};
    use crate::callback::Registration;

    fn host() -> Arc<Host> {
        Arc::new(Host {
            registration: Registration::new(
                "custom layer host",
                true,
                std::ptr::null_mut(),
                None,
                crate::handles::NULL,
            )
            .unwrap(),
            prerender: None,
            render: None,
            deinitialize: None,
            context_lost: None,
            scale_factor: 1.0,
            drew: AtomicBool::new(false),
        })
    }

    /// A renderer that is leaked with the style data, because its graphics
    /// objects are gone, must not keep the host, whose user data is
    /// released once the host goes.
    #[test]
    fn the_style_data_of_a_removed_layer_doesnt_keep_its_host() {
        let host = host();
        let functions = Arc::clone(crate::registry::registry().functions());
        let layer = layer(functions, "custom".to_owned(), Arc::clone(&host));
        let style = LayerStyle::<Custom>::recover(layer.style()).unwrap();
        assert!(style.extra.is(&host));
        assert!(style.extra.get().is_some());
        drop(layer);
        assert!(style.extra.get().is_some(), "the caller still holds the host");
        drop(host);
        assert!(style.extra.get().is_none());
    }
}
