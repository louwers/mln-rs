// A custom layer whose host draws a green quad with the backend's own GPU
// commands, in the map's render pass, at its place among the map's layers: over
// a red background and under a half-transparent blue one, or over both.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use mln_engine::exec::pool::Pool;
use mln_engine::exec::queue::Queue;
use mln_engine::map::{Map, MapMode, MapOptions};
use mln_engine::{Custom, CustomLayerHost, CustomLayerPrerenderParameters, CustomLayerRenderParameters};
use mln_headless::HeadlessFrontend;
use mln_runtime::{BackendType, Device, DeviceOptions};
use mln_style::bitmap::PremultipliedImage;
use mln_style::error::ErrorKind;
use mln_style::size::Size;
use parking_lot::Mutex;

const SIDE: u32 = 64;

const STYLE: &str = r##"{
  "version": 8,
  "sources": {},
  "layers": [
    {"id": "red", "type": "background", "paint": {"background-color": "#ff0000"}},
    {"id": "blue", "type": "background", "paint": {"background-color": "#0000ff", "background-opacity": 0.5}}
  ]
}"##;

/// The quad's corners in clip space: the middle half of the frame.
const QUAD_WGSL: &str = "
@vertex fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    var corners = array<vec2<f32>, 6>(
        vec2(-0.5, -0.5), vec2(0.5, -0.5), vec2(-0.5, 0.5),
        vec2(-0.5, 0.5), vec2(0.5, -0.5), vec2(0.5, 0.5),
    );
    return vec4<f32>(corners[index], 0.0, 1.0);
}

@fragment fn fs() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 1.0, 0.0, 1.0);
}
";

/// What the host's callbacks saw.
#[derive(Default)]
struct Calls {
    prerenders: AtomicUsize,
    renders: AtomicUsize,
    deinitializations: AtomicUsize,
    /// The frame size and layer depth that the last render got.
    frame: Mutex<Option<(u32, u32, f64, f64)>>,
}

impl Calls {
    fn prerender(&self, parameters: &CustomLayerPrerenderParameters<'_>) {
        assert_eq!((parameters.frame.width, parameters.frame.height), (SIDE, SIDE));
        self.prerenders.fetch_add(1, Ordering::Relaxed);
    }

    fn render(&self, parameters: &CustomLayerRenderParameters<'_>) {
        // Every prerender of a frame comes before its renders.
        assert_eq!(self.prerenders.load(Ordering::Relaxed), self.renders.load(Ordering::Relaxed) + 1);
        let frame = parameters.frame;
        *self.frame.lock() = Some((frame.width, frame.height, frame.projection.far_depth, parameters.layer_depth));
        self.renders.fetch_add(1, Ordering::Relaxed);
    }
}

/// Renders the style with `host`'s layer below the layer named `before`, or
/// above every layer, on `backend`, and drops the renderer. `None` where the
/// machine has no device of `backend`, such as no EGL.
fn render(backend: BackendType, host: Arc<dyn CustomLayerHost>, before: Option<&str>) -> Option<PremultipliedImage> {
    let run_loop = Queue::new();
    let (mut frontend, mut map) = session(backend, host, before, &run_loop)?;
    Some(frontend.render(&mut map).expect("a frame").image)
}

/// A frontend on `backend` and a map of the style with `host`'s layer below
/// the layer named `before`, or above every layer, on `run_loop`.
/// `None` where the machine has no device of `backend`.
fn session(
    backend: BackendType,
    host: Arc<dyn CustomLayerHost>,
    before: Option<&str>,
    run_loop: &Queue<()>,
) -> Option<(HeadlessFrontend, Map)> {
    let size = Size::new(SIDE, SIDE);
    let pool = Pool::new(4);
    let device = match Device::with_backend(backend, DeviceOptions::default()) {
        Ok(device) => device,
        Err(error) if error.kind == ErrorKind::Unsupported => {
            eprintln!("skipped: no {backend:?} device: {error}");
            return None;
        }
        Err(error) => panic!("a {backend:?} device: {error}"),
    };
    let mut frontend =
        HeadlessFrontend::with_device(device, size, 1.0, Arc::default(), run_loop, &pool).expect("a frontend");
    let mut map = Map::new(
        MapOptions { map_mode: MapMode::Static, size, pixel_ratio: 1.0, ..Default::default() },
        mln_builtins::registry(),
        &pool,
    )
    .unwrap();
    frontend.attach(&mut map);
    map.load_style_json(STYLE);
    map.edit_style(|style| style.add_layer(Custom::layer(style.registry(), "quad", host), before))
        .expect("the custom layer");
    Some((frontend, map))
}

fn pixel(image: &PremultipliedImage, x: u32, y: u32) -> [u8; 4] {
    let start = ((y * image.size.width + x) * 4) as usize;
    image.data[start..start + 4].try_into().unwrap()
}

fn assert_near(actual: [u8; 4], expected: [u8; 4], what: &str) {
    let near = actual.iter().zip(expected).all(|(a, e)| a.abs_diff(e) <= 3);
    assert!(near, "{what}: {actual:?}, expected about {expected:?}");
}

/// Draws the quad under and over the blue layer on `backend`, and checks the
/// pixels, the callbacks, and that the renderer deinitialized the host.
fn check(backend: BackendType, host: impl Fn() -> (Arc<dyn CustomLayerHost>, Arc<Calls>)) {
    let (below, calls) = host();
    let Some(image) = render(backend, below, Some("blue")) else { return };
    assert_near(pixel(&image, SIDE / 2, SIDE / 2), [0, 128, 128, 255], "the quad under the blue layer");
    assert_near(pixel(&image, 2, 2), [128, 0, 128, 255], "the map around the quad");
    assert_eq!(calls.prerenders.load(Ordering::Relaxed), 1);
    assert_eq!(calls.renders.load(Ordering::Relaxed), 1);
    assert_eq!(calls.deinitializations.load(Ordering::Relaxed), 1, "the renderer that drew the layer went");
    let (width, height, far_depth, layer_depth) = calls.frame.lock().expect("a render");
    assert_eq!((width, height), (SIDE, SIDE));
    assert!(far_depth > 0.0 && far_depth < 1.0 && layer_depth > far_depth && layer_depth <= 1.0);

    let (above, _) = host();
    let image = render(backend, above, None).expect("a device, as for the first frame");
    assert_near(pixel(&image, SIDE / 2, SIDE / 2), [0, 255, 0, 255], "the quad over every layer");
    assert_near(pixel(&image, 2, 2), [128, 0, 128, 255], "the map around the quad");
}

mod webgpu {
    use mln_engine::{CustomLayerPrerenderParameters, CustomLayerRenderParameters};

    use super::{Arc, BackendType, Calls, CustomLayerHost, Mutex, Ordering, QUAD_WGSL, check};

    #[derive(Default)]
    struct Host {
        calls: Arc<Calls>,
        pipeline: Mutex<Option<wgpu::RenderPipeline>>,
    }

    impl CustomLayerHost for Host {
        fn prerender(&self, parameters: &mut CustomLayerPrerenderParameters<'_>) {
            assert!(parameters.encoder.is::<mln_webgpu::CustomLayerEncoder>());
            self.calls.prerender(parameters);
        }

        fn render(&self, parameters: &mut CustomLayerRenderParameters<'_>) {
            self.calls.render(parameters);
            let pass = parameters.pass.downcast_mut::<mln_webgpu::CustomLayerPass>().expect("a WebGPU pass");
            let mut pipeline = self.pipeline.lock();
            let pipeline = pipeline.get_or_insert_with(|| {
                let module = pass.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("quad"),
                    source: wgpu::ShaderSource::Wgsl(QUAD_WGSL.into()),
                });
                pass.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("quad"),
                    layout: None,
                    vertex: wgpu::VertexState {
                        module: &module,
                        entry_point: Some("vs"),
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                        buffers: &[],
                    },
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: pass.depth_stencil_format.map(|format| wgpu::DepthStencilState {
                        format,
                        depth_write_enabled: Some(false),
                        depth_compare: Some(wgpu::CompareFunction::Always),
                        stencil: wgpu::StencilState::default(),
                        bias: wgpu::DepthBiasState::default(),
                    }),
                    multisample: wgpu::MultisampleState { count: pass.sample_count, ..Default::default() },
                    fragment: Some(wgpu::FragmentState {
                        module: &module,
                        entry_point: Some("fs"),
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                        targets: &[Some(pass.color_format.into())],
                    }),
                    multiview_mask: None,
                    cache: None,
                })
            });
            pass.pass.set_pipeline(pipeline);
            pass.pass.draw(0..6, 0..1);
        }

        fn deinitialize(&self) {
            self.pipeline.lock().take();
            self.calls.deinitializations.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn a_quad_draws_at_its_place_among_the_layers() {
        check(BackendType::WebGPU, || {
            let host = Arc::new(Host::default());
            (Arc::<Host>::clone(&host), Arc::clone(&host.calls))
        });
    }
}

#[cfg(not(any(target_vendor = "apple", target_family = "wasm")))]
#[expect(unsafe_code, reason = "the host draws through ash")]
mod vulkan {
    use mln_engine::{CustomLayerPrerenderParameters, CustomLayerRenderParameters};
    use mln_vulkan::ash::{self, vk};

    use super::{Arc, BackendType, Calls, CustomLayerHost, Mutex, Ordering, QUAD_WGSL, check};

    /// The host's objects, made on the session's device.
    struct Objects {
        device: ash::Device,
        module: vk::ShaderModule,
        layout: vk::PipelineLayout,
        pipeline: vk::Pipeline,
        render_pass: vk::RenderPass,
    }

    #[derive(Default)]
    struct Host {
        calls: Arc<Calls>,
        objects: Mutex<Option<Objects>>,
    }

    fn spirv() -> Vec<u32> {
        let module = naga::front::wgsl::parse_str(QUAD_WGSL).expect("WGSL");
        let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
            .validate(&module)
            .expect("a valid module");
        naga::back::spv::write_vec(&module, &info, &naga::back::spv::Options::default(), None).expect("SPIR-V")
    }

    fn make(pass: &mln_vulkan::CustomLayerPass) -> Objects {
        let raw = &pass.device;
        let static_fn = ash::StaticFn { get_instance_proc_addr: raw.get_instance_proc_addr };
        // SAFETY: the session's live instance and device, and their loaders.
        let device = unsafe {
            let instance = ash::Instance::load(&static_fn, raw.instance);
            ash::Device::load(instance.fp_v1_0(), raw.device)
        };
        let code = spirv();
        // SAFETY: valid create infos for the device; the objects go in
        // `deinitialize`.
        unsafe {
            let module = device.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&code), None).unwrap();
            let layout = device.create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default(), None).unwrap();
            let stages = [
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk::ShaderStageFlags::VERTEX)
                    .module(module)
                    .name(c"vs"),
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk::ShaderStageFlags::FRAGMENT)
                    .module(module)
                    .name(c"fs"),
            ];
            let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
            let input_assembly =
                vk::PipelineInputAssemblyStateCreateInfo::default().topology(vk::PrimitiveTopology::TRIANGLE_LIST);
            let viewport = vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);
            let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
                .polygon_mode(vk::PolygonMode::FILL)
                .cull_mode(vk::CullModeFlags::NONE)
                .line_width(1.0);
            let multisample = vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(pass.samples);
            let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default();
            let attachments =
                [vk::PipelineColorBlendAttachmentState::default().color_write_mask(vk::ColorComponentFlags::RGBA)];
            let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&attachments);
            let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
            let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
            let info = vk::GraphicsPipelineCreateInfo::default()
                .stages(&stages)
                .vertex_input_state(&vertex_input)
                .input_assembly_state(&input_assembly)
                .viewport_state(&viewport)
                .rasterization_state(&rasterization)
                .multisample_state(&multisample)
                .depth_stencil_state(&depth_stencil)
                .color_blend_state(&blend)
                .dynamic_state(&dynamic)
                .layout(layout)
                .render_pass(pass.render_pass)
                .subpass(pass.subpass);
            let pipeline = device
                .create_graphics_pipelines(vk::PipelineCache::null(), &[info], None)
                .map_err(|(_, error)| error)
                .unwrap()[0];
            Objects { device, module, layout, pipeline, render_pass: pass.render_pass }
        }
    }

    impl CustomLayerHost for Host {
        fn prerender(&self, parameters: &mut CustomLayerPrerenderParameters<'_>) {
            let encoder = parameters.encoder.downcast_ref::<mln_vulkan::CustomLayerEncoder>().expect("Vulkan");
            assert_ne!(encoder.command_buffer, vk::CommandBuffer::null());
            self.calls.prerender(parameters);
        }

        fn render(&self, parameters: &mut CustomLayerRenderParameters<'_>) {
            self.calls.render(parameters);
            let pass = parameters.pass.downcast_ref::<mln_vulkan::CustomLayerPass>().expect("a Vulkan pass");
            let mut objects = self.objects.lock();
            let objects = objects.get_or_insert_with(|| make(pass));
            assert_eq!(objects.render_pass, pass.render_pass);
            let extent = pass.extent;
            // SAFETY: the command buffer records in the pipeline's render
            // pass.
            unsafe {
                let device = &objects.device;
                device.cmd_bind_pipeline(pass.command_buffer, vk::PipelineBindPoint::GRAPHICS, objects.pipeline);
                device.cmd_set_viewport(
                    pass.command_buffer,
                    0,
                    &[vk::Viewport {
                        x: 0.0,
                        y: 0.0,
                        width: extent.width as f32,
                        height: extent.height as f32,
                        min_depth: 0.0,
                        max_depth: 1.0,
                    }],
                );
                device.cmd_set_scissor(
                    pass.command_buffer,
                    0,
                    &[vk::Rect2D { offset: vk::Offset2D::default(), extent }],
                );
                device.cmd_draw(pass.command_buffer, 6, 1, 0, 0);
            }
        }

        fn deinitialize(&self) {
            if let Some(objects) = self.objects.lock().take() {
                // SAFETY: the GPU no longer uses them, as `deinitialize`
                // promises.
                unsafe {
                    objects.device.destroy_pipeline(objects.pipeline, None);
                    objects.device.destroy_pipeline_layout(objects.layout, None);
                    objects.device.destroy_shader_module(objects.module, None);
                }
            }
            self.calls.deinitializations.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn a_quad_draws_at_its_place_among_the_layers() {
        check(BackendType::Vulkan, || {
            let host = Arc::new(Host::default());
            (Arc::<Host>::clone(&host), Arc::clone(&host.calls))
        });
    }
}

#[cfg(all(unix, not(target_vendor = "apple")))]
#[expect(unsafe_code, reason = "the host draws through glow")]
mod gl {
    use std::sync::OnceLock;

    use glow::HasContext as _;
    use khronos_egl as egl;
    use mln_engine::map::Map;
    use mln_engine::{CustomLayerPrerenderParameters, CustomLayerRenderParameters};
    use mln_headless::HeadlessFrontend;
    use mln_style::Edit;

    use super::{Arc, BackendType, Calls, Custom, CustomLayerHost, Mutex, Ordering, Queue, check, session};

    const VERTEX: &str = "#version 300 es
const vec2 corners[6] = vec2[6](
    vec2(-0.5, -0.5), vec2(0.5, -0.5), vec2(-0.5, 0.5),
    vec2(-0.5, 0.5), vec2(0.5, -0.5), vec2(0.5, 0.5));
void main() { gl_Position = vec4(corners[gl_VertexID], 0.0, 1.0); }
";
    const FRAGMENT: &str = "#version 300 es
precision mediump float;
out vec4 color;
void main() { color = vec4(0.0, 1.0, 0.0, 1.0); }
";

    /// The EGL context current on the thread, if any.
    fn current_context() -> Option<usize> {
        static EGL: OnceLock<egl::DynamicInstance<egl::EGL1_4>> = OnceLock::new();
        // SAFETY: loads the system's EGL, which the OpenGL device loaded
        // already.
        let egl = EGL.get_or_init(|| unsafe { egl::DynamicInstance::<egl::EGL1_4>::load_required() }.expect("EGL"));
        egl.get_current_context().map(|context| context.as_ptr() as usize)
    }

    #[derive(Default)]
    struct Host {
        calls: Arc<Calls>,
        objects: Mutex<Option<(glow::NativeProgram, glow::NativeVertexArray)>>,
        /// The context the host drew on.
        context: Mutex<Option<usize>>,
        /// Whether that context was current when the host deinitialized.
        deinitialized_on_its_context: Mutex<Option<bool>>,
    }

    impl CustomLayerHost for Host {
        fn prerender(&self, parameters: &mut CustomLayerPrerenderParameters<'_>) {
            assert!(parameters.encoder.is::<mln_gl::CustomLayerEncoder>());
            self.calls.prerender(parameters);
        }

        fn render(&self, parameters: &mut CustomLayerRenderParameters<'_>) {
            self.calls.render(parameters);
            *self.context.lock() = current_context();
            let pass = parameters.pass.downcast_ref::<mln_gl::CustomLayerPass>().expect("an OpenGL pass");
            let gl = pass.gl();
            let mut objects = self.objects.lock();
            // SAFETY: the session's context is current, and the objects are
            // made on it.
            unsafe {
                let (program, vertex_array) = *objects.get_or_insert_with(|| {
                    let program = gl.create_program().unwrap();
                    for (kind, source) in [(glow::VERTEX_SHADER, VERTEX), (glow::FRAGMENT_SHADER, FRAGMENT)] {
                        let shader = gl.create_shader(kind).unwrap();
                        gl.shader_source(shader, source);
                        gl.compile_shader(shader);
                        assert!(gl.get_shader_compile_status(shader), "{}", gl.get_shader_info_log(shader));
                        gl.attach_shader(program, shader);
                        gl.delete_shader(shader);
                    }
                    gl.link_program(program);
                    assert!(gl.get_program_link_status(program), "{}", gl.get_program_info_log(program));
                    (program, gl.create_vertex_array().unwrap())
                });
                gl.disable(glow::DEPTH_TEST);
                gl.disable(glow::STENCIL_TEST);
                gl.disable(glow::BLEND);
                gl.disable(glow::SCISSOR_TEST);
                gl.disable(glow::CULL_FACE);
                gl.color_mask(true, true, true, true);
                gl.use_program(Some(program));
                gl.bind_vertex_array(Some(vertex_array));
                gl.draw_arrays(glow::TRIANGLES, 0, 6);
            }
        }

        fn deinitialize(&self) {
            // The program and vertex array go with the session's context.
            self.objects.lock().take();
            let drawn_on = *self.context.lock();
            *self.deinitialized_on_its_context.lock() = Some(drawn_on.is_some() && current_context() == drawn_on);
            self.calls.deinitializations.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn a_quad_draws_at_its_place_among_the_layers() {
        check(BackendType::OpenGL, || {
            let host = Arc::new(Host::default());
            (Arc::<Host>::clone(&host), Arc::clone(&host.calls))
        });
    }

    /// Draws a frame with the host's layer, lets go of the layer with
    /// `end`, which can drop the frontend, and checks that the host
    /// deinitialized with the context it drew on current.
    fn check_end(end: impl FnOnce(&mut Option<HeadlessFrontend>, &mut Map)) {
        let run_loop = Queue::new();
        let host = Arc::new(Host::default());
        let Some((frontend, mut map)) = session(BackendType::OpenGL, Arc::<Host>::clone(&host), None, &run_loop) else {
            return;
        };
        let mut frontend = Some(frontend);
        frontend.as_mut().unwrap().render(&mut map).expect("a frame");
        assert_eq!(host.calls.renders.load(Ordering::Relaxed), 1);
        end(&mut frontend, &mut map);
        assert_eq!(host.calls.deinitializations.load(Ordering::Relaxed), 1);
        assert_eq!(*host.deinitialized_on_its_context.lock(), Some(true), "the context was current");
    }

    #[test]
    fn a_removed_layer_deinitializes_on_its_context() {
        check_end(|frontend, map| {
            map.edit_style(|style| style.apply(Edit::RemoveLayer("quad".to_owned()))).expect("the layer");
            frontend.as_mut().unwrap().render(map).expect("a frame");
        });
    }

    #[test]
    fn a_replaced_host_deinitializes_on_its_context() {
        check_end(|frontend, map| {
            map.edit_style(|style| style.apply(Edit::RemoveLayer("quad".to_owned()))).expect("the layer");
            map.edit_style(|style| {
                style.add_layer(Custom::layer(style.registry(), "quad", Arc::new(Host::default())), None)
            })
            .expect("the new host");
            frontend.as_mut().unwrap().render(map).expect("a frame");
        });
    }

    #[test]
    fn clearing_the_data_deinitializes_on_its_context() {
        check_end(|frontend, _| frontend.as_mut().unwrap().clear_data());
    }

    #[test]
    fn dropping_the_renderer_deinitializes_on_its_context() {
        check_end(|frontend, _| drop(frontend.take()));
    }
}

#[cfg(target_vendor = "apple")]
#[expect(unsafe_code, reason = "the host draws through objc2-metal")]
mod metal {
    use std::sync::atomic::AtomicUsize;

    use mln_engine::{CustomLayerPrerenderParameters, CustomLayerRenderParameters};
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_foundation::NSString;
    use objc2_metal::{
        MTLCommandBuffer, MTLCommandBufferStatus, MTLCommandEncoder, MTLCompareFunction, MTLCullMode,
        MTLDepthStencilDescriptor, MTLDepthStencilState, MTLDevice, MTLLibrary, MTLPrimitiveType,
        MTLRenderCommandEncoder, MTLRenderPipelineDescriptor, MTLRenderPipelineState,
    };

    use super::{Arc, BackendType, Calls, CustomLayerHost, Mutex, Ordering, check};

    /// The quad of `QUAD_WGSL`, in MSL.
    const QUAD_MSL: &str = "
#include <metal_stdlib>
using namespace metal;

vertex float4 vs(uint index [[vertex_id]]) {
    const float2 corners[6] = { float2(-0.5, -0.5), float2(0.5, -0.5), float2(-0.5, 0.5),
                                float2(-0.5, 0.5), float2(0.5, -0.5), float2(0.5, 0.5) };
    return float4(corners[index], 0.0, 1.0);
}

fragment float4 fs() { return float4(0.0, 1.0, 0.0, 1.0); }
";

    /// The host's objects, made on the session's device.
    struct Objects {
        pipeline: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
        depth: Retained<ProtocolObject<dyn MTLDepthStencilState>>,
    }

    #[derive(Default)]
    struct Host {
        calls: Arc<Calls>,
        objects: Mutex<Option<Objects>>,
        /// The blits `prerender` encoded on the frame's command buffer.
        prerender_blits: AtomicUsize,
    }

    fn make(pass: &mln_mtl::CustomLayerPass) -> Objects {
        let device = pass.encoder.device();
        let library = device.newLibraryWithSource_options_error(&NSString::from_str(QUAD_MSL), None).expect("MSL");
        let function = |name: &str| library.newFunctionWithName(&NSString::from_str(name));
        let descriptor = MTLRenderPipelineDescriptor::new();
        descriptor.setVertexFunction(function("vs").as_deref());
        descriptor.setFragmentFunction(function("fs").as_deref());
        // SAFETY: index 0 of the 8 color attachments.
        unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) }.setPixelFormat(pass.formats.color);
        descriptor.setRasterSampleCount(pass.sample_count);
        if let Some(depth) = pass.formats.depth {
            descriptor.setDepthAttachmentPixelFormat(depth);
        }
        if let Some(stencil) = pass.formats.stencil {
            descriptor.setStencilAttachmentPixelFormat(stencil);
        }
        let pipeline = device.newRenderPipelineStateWithDescriptor_error(&descriptor).expect("a pipeline");
        let depth_descriptor = MTLDepthStencilDescriptor::new();
        depth_descriptor.setDepthCompareFunction(MTLCompareFunction::Always);
        depth_descriptor.setDepthWriteEnabled(false);
        let depth = device.newDepthStencilStateWithDescriptor(&depth_descriptor).expect("a depth-stencil state");
        Objects { pipeline, depth }
    }

    impl CustomLayerHost for Host {
        fn prerender(&self, parameters: &mut CustomLayerPrerenderParameters<'_>) {
            let encoder = parameters.encoder.downcast_ref::<mln_mtl::CustomLayerEncoder>().expect("a Metal encoder");
            let command_buffer = &encoder.command_buffer;
            assert_eq!(command_buffer.status(), MTLCommandBufferStatus::NotEnqueued, "an open command buffer");
            let blit = command_buffer.blitCommandEncoder().expect("a blit encoder on the frame's command buffer");
            blit.setLabel(Some(&NSString::from_str("custom layer prerender")));
            blit.endEncoding();
            self.prerender_blits.fetch_add(1, Ordering::Relaxed);
            self.calls.prerender(parameters);
        }

        fn render(&self, parameters: &mut CustomLayerRenderParameters<'_>) {
            self.calls.render(parameters);
            assert_eq!(self.prerender_blits.load(Ordering::Relaxed), self.calls.prerenders.load(Ordering::Relaxed));
            let pass = parameters.pass.downcast_ref::<mln_mtl::CustomLayerPass>().expect("a Metal pass");
            let mut objects = self.objects.lock();
            let objects = objects.get_or_insert_with(|| make(pass));
            pass.encoder.setRenderPipelineState(&objects.pipeline);
            pass.encoder.setDepthStencilState(Some(&objects.depth));
            pass.encoder.setCullMode(MTLCullMode::None);
            // SAFETY: the pipeline's vertex function reads 6 vertices.
            unsafe { pass.encoder.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 6) };
        }

        fn deinitialize(&self) {
            self.objects.lock().take();
            self.calls.deinitializations.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn a_quad_draws_at_its_place_among_the_layers() {
        check(BackendType::Metal, || {
            let host = Arc::new(Host::default());
            (Arc::<Host>::clone(&host), Arc::clone(&host.calls))
        });
    }
}

#[cfg(windows)]
#[expect(unsafe_code, reason = "the host draws through the windows crate's D3D12")]
mod d3d12 {
    use std::mem::ManuallyDrop;

    use mln_d3d12::windows::Win32::Foundation::RECT;
    use mln_d3d12::windows::Win32::Graphics::Direct3D::{D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, ID3DBlob};
    use mln_d3d12::windows::Win32::Graphics::Direct3D12::{
        D3D_ROOT_SIGNATURE_VERSION_1, D3D12_BLEND_DESC, D3D12_COLOR_WRITE_ENABLE_ALL, D3D12_CULL_MODE_NONE,
        D3D12_DEPTH_STENCIL_DESC, D3D12_FILL_MODE_SOLID, D3D12_GRAPHICS_PIPELINE_STATE_DESC,
        D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE, D3D12_RASTERIZER_DESC, D3D12_RENDER_TARGET_BLEND_DESC,
        D3D12_ROOT_SIGNATURE_DESC, D3D12_SHADER_BYTECODE, D3D12_VIEWPORT, D3D12SerializeRootSignature,
        ID3D12GraphicsCommandList, ID3D12PipelineState, ID3D12RootSignature,
    };
    use mln_d3d12::windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC};
    use mln_d3d12::{ShaderCompiler, ShaderStage};
    use mln_engine::{CustomLayerPrerenderParameters, CustomLayerRenderParameters};

    use super::{Arc, BackendType, Calls, CustomLayerHost, Mutex, Ordering, check};

    /// The quad's corners in clip space, the middle half of the frame, as
    /// `QUAD_WGSL` draws it.
    const QUAD_HLSL: &str = "
float4 vs(uint index : SV_VertexID) : SV_Position {
    const float2 corners[6] = {
        float2(-0.5, -0.5), float2(0.5, -0.5), float2(-0.5, 0.5),
        float2(-0.5, 0.5), float2(0.5, -0.5), float2(0.5, 0.5),
    };
    return float4(corners[index], 0.0, 1.0);
}

float4 ps() : SV_Target {
    return float4(0.0, 1.0, 0.0, 1.0);
}
";

    /// The host's objects, made on the session's device.
    struct Objects {
        root_signature: ID3D12RootSignature,
        pipeline: ID3D12PipelineState,
    }

    #[derive(Default)]
    struct Host {
        calls: Arc<Calls>,
        objects: Mutex<Option<Objects>>,
    }

    fn make(pass: &mln_d3d12::CustomLayerPass) -> Objects {
        let compiler = ShaderCompiler::new().expect("DXC");
        let vertex = compiler.compile_hlsl(ShaderStage::Vertex, QUAD_HLSL, "vs").expect("the vertex shader");
        let fragment = compiler.compile_hlsl(ShaderStage::Fragment, QUAD_HLSL, "ps").expect("the pixel shader");
        // SAFETY: valid descriptions for the session's device, which outlive
        // the calls; the objects go in `deinitialize`.
        unsafe {
            let mut blob: Option<ID3DBlob> = None;
            let empty = D3D12_ROOT_SIGNATURE_DESC::default();
            D3D12SerializeRootSignature(&raw const empty, D3D_ROOT_SIGNATURE_VERSION_1, &raw mut blob, None)
                .expect("a root signature");
            let blob = blob.unwrap();
            let bytes = std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize());
            let root_signature: ID3D12RootSignature = pass.device.CreateRootSignature(0, bytes).unwrap();
            let bytecode = |code: &[u8]| D3D12_SHADER_BYTECODE {
                pShaderBytecode: code.as_ptr().cast(),
                BytecodeLength: code.len(),
            };
            let mut blend = D3D12_BLEND_DESC::default();
            blend.RenderTarget[0] = D3D12_RENDER_TARGET_BLEND_DESC {
                RenderTargetWriteMask: D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8,
                ..Default::default()
            };
            let mut formats = [DXGI_FORMAT_UNKNOWN; 8];
            formats[0] = pass.color_format;
            let desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
                pRootSignature: ManuallyDrop::new(Some(root_signature.clone())),
                VS: bytecode(&vertex),
                PS: bytecode(&fragment),
                BlendState: blend,
                SampleMask: u32::MAX,
                RasterizerState: D3D12_RASTERIZER_DESC {
                    FillMode: D3D12_FILL_MODE_SOLID,
                    CullMode: D3D12_CULL_MODE_NONE,
                    DepthClipEnable: true.into(),
                    ..Default::default()
                },
                DepthStencilState: D3D12_DEPTH_STENCIL_DESC::default(),
                PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
                NumRenderTargets: 1,
                RTVFormats: formats,
                DSVFormat: pass.depth_stencil_format,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                ..Default::default()
            };
            let pipeline = pass.device.CreateGraphicsPipelineState(&raw const desc);
            drop(ManuallyDrop::into_inner(desc.pRootSignature));
            Objects { root_signature, pipeline: pipeline.expect("a pipeline state") }
        }
    }

    impl CustomLayerHost for Host {
        fn prerender(&self, parameters: &mut CustomLayerPrerenderParameters<'_>) {
            let encoder = parameters.encoder.downcast_ref::<mln_d3d12::CustomLayerEncoder>().expect("D3D12");
            let _: &ID3D12GraphicsCommandList = &encoder.command_list;
            self.calls.prerender(parameters);
        }

        fn render(&self, parameters: &mut CustomLayerRenderParameters<'_>) {
            self.calls.render(parameters);
            let (width, height) = (parameters.frame.width, parameters.frame.height);
            let pass = parameters.pass.downcast_ref::<mln_d3d12::CustomLayerPass>().expect("a D3D12 pass");
            let mut objects = self.objects.lock();
            let objects = objects.get_or_insert_with(|| make(pass));
            let list = &pass.command_list;
            // SAFETY: the command list records in the map's render pass, with
            // the pipeline of its formats.
            unsafe {
                list.SetPipelineState(&objects.pipeline);
                list.SetGraphicsRootSignature(&objects.root_signature);
                list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
                list.RSSetViewports(&[D3D12_VIEWPORT {
                    TopLeftX: 0.0,
                    TopLeftY: 0.0,
                    Width: width as f32,
                    Height: height as f32,
                    MinDepth: 0.0,
                    MaxDepth: 1.0,
                }]);
                list.RSSetScissorRects(&[RECT { left: 0, top: 0, right: width as i32, bottom: height as i32 }]);
                list.DrawInstanced(6, 1, 0, 0);
            }
        }

        fn deinitialize(&self) {
            self.objects.lock().take();
            self.calls.deinitializations.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn a_quad_draws_at_its_place_among_the_layers() {
        check(BackendType::D3D12, || {
            let host = Arc::new(Host::default());
            (Arc::<Host>::clone(&host), Arc::clone(&host.calls))
        });
    }
}

/// How a host that drew hears that the renderer stopped, on each backend.
mod ending {
    use mln_style::Edit;

    use super::{
        Arc, AtomicUsize, BackendType, Custom, CustomLayerHost, HeadlessFrontend, Map, Ordering, Queue, session,
    };

    /// A host that draws nothing, and counts how it ended.
    #[derive(Default)]
    struct Host {
        deinitializations: AtomicUsize,
        losses: AtomicUsize,
    }

    impl CustomLayerHost for Host {
        fn deinitialize(&self) {
            self.deinitializations.fetch_add(1, Ordering::Relaxed);
        }

        fn context_lost(&self) {
            self.losses.fetch_add(1, Ordering::Relaxed);
        }
    }

    const BACKENDS: &[BackendType] = &[
        BackendType::WebGPU,
        #[cfg(target_vendor = "apple")]
        BackendType::Metal,
        #[cfg(not(any(target_vendor = "apple", target_family = "wasm")))]
        BackendType::Vulkan,
        #[cfg(all(unix, not(target_vendor = "apple")))]
        BackendType::OpenGL,
        #[cfg(windows)]
        BackendType::D3D12,
    ];

    /// Draws a frame with the host's layer on each backend, lets go of the
    /// layer with `end`, which can drop the frontend, and checks that the
    /// host deinitialized once and lost nothing.
    fn check_end(end: impl Fn(&mut Option<HeadlessFrontend>, &mut Map)) {
        for &backend in BACKENDS {
            let run_loop = Queue::new();
            let host = Arc::new(Host::default());
            let Some((frontend, mut map)) = session(backend, Arc::<Host>::clone(&host), None, &run_loop) else {
                continue;
            };
            let mut frontend = Some(frontend);
            frontend.as_mut().unwrap().render(&mut map).expect("a frame");
            end(&mut frontend, &mut map);
            assert_eq!(host.deinitializations.load(Ordering::Relaxed), 1, "{backend:?}");
            assert_eq!(host.losses.load(Ordering::Relaxed), 0, "{backend:?}");
        }
    }

    #[test]
    fn a_removed_layer_deinitializes() {
        check_end(|frontend, map| {
            map.edit_style(|style| style.apply(Edit::RemoveLayer("quad".to_owned()))).expect("the layer");
            frontend.as_mut().unwrap().render(map).expect("a frame");
        });
    }

    #[test]
    fn a_replaced_host_deinitializes() {
        check_end(|frontend, map| {
            map.edit_style(|style| style.apply(Edit::RemoveLayer("quad".to_owned()))).expect("the layer");
            map.edit_style(|style| {
                style.add_layer(Custom::layer(style.registry(), "quad", Arc::new(Host::default())), None)
            })
            .expect("the new host");
            frontend.as_mut().unwrap().render(map).expect("a frame");
        });
    }

    #[test]
    fn clearing_the_data_deinitializes() {
        check_end(|frontend, _| frontend.as_mut().unwrap().clear_data());
    }

    #[test]
    fn dropping_the_renderer_deinitializes() {
        check_end(|frontend, _| drop(frontend.take()));
    }
}
