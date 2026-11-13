// A custom layer whose host draws a green quad with the backend's own GPU
// commands, in the map's render pass, at its place among the map's layers: over
// a red background and under a half-transparent blue one, or over both.

// The hosts draw through each backend's own API.
#![allow(unsafe_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use mln_actor::RunLoop;
use mln_headless::HeadlessFrontend;
use mln_map::{Map, MapMode, MapOptions};
use mln_runtime::{BackendType, Device, DeviceOptions};
use mln_style::layers::{CustomLayerHost, CustomLayerPrerenderParameters, CustomLayerRenderParameters};
use mln_util::error::ErrorKind;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

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
        *self.frame.lock().unwrap() =
            Some((frame.width, frame.height, frame.projection.far_depth, parameters.layer_depth));
        self.renders.fetch_add(1, Ordering::Relaxed);
    }
}

/// Renders the style with `host`'s layer below the layer named `before`, or
/// above every layer, on `backend`, and drops the renderer. `None` where the
/// machine has no device of `backend`, such as no EGL.
fn render(backend: BackendType, host: Arc<dyn CustomLayerHost>, before: Option<&str>) -> Option<PremultipliedImage> {
    let _run_loop = RunLoop::new();
    let size = Size::new(SIDE, SIDE);
    let device = match Device::with_backend(backend, DeviceOptions::default()) {
        Ok(device) => device,
        Err(error) if error.kind == ErrorKind::Unsupported => {
            eprintln!("skipped: no {backend:?} device: {error}");
            return None;
        }
        Err(error) => panic!("a {backend:?} device: {error}"),
    };
    let mut frontend = HeadlessFrontend::with_device(device, size, 1.0, true, Arc::default()).expect("a frontend");
    let mut map =
        Map::new(MapOptions::new().with_map_mode(MapMode::Static).with_size(size).with_pixel_ratio(1.0)).unwrap();
    frontend.attach(&map);
    map.style_mut().load_json(STYLE);
    map.style_mut().add_custom_layer("quad", host, before).expect("the custom layer");
    Some(frontend.render(&mut map).expect("a frame").image)
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
    let (width, height, far_depth, layer_depth) = calls.frame.lock().unwrap().expect("a render");
    assert_eq!((width, height), (SIDE, SIDE));
    assert!(far_depth > 0.0 && far_depth < 1.0 && layer_depth > far_depth && layer_depth <= 1.0);

    let (above, _) = host();
    let image = render(backend, above, None).expect("a device, as for the first frame");
    assert_near(pixel(&image, SIDE / 2, SIDE / 2), [0, 255, 0, 255], "the quad over every layer");
    assert_near(pixel(&image, 2, 2), [128, 0, 128, 255], "the map around the quad");
}

mod webgpu {
    use mln_style::layers::{CustomLayerPrerenderParameters, CustomLayerRenderParameters};

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
            let mut pipeline = self.pipeline.lock().unwrap();
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
            self.pipeline.lock().unwrap().take();
            self.calls.deinitializations.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn a_quad_draws_at_its_place_among_the_layers() {
        check(BackendType::WebGPU, || {
            let host = Arc::new(Host::default());
            (host.clone() as Arc<dyn CustomLayerHost>, host.calls.clone())
        });
    }
}

#[cfg(not(any(target_vendor = "apple", target_family = "wasm")))]
mod vulkan {
    use mln_style::layers::{CustomLayerPrerenderParameters, CustomLayerRenderParameters};
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
            let mut objects = self.objects.lock().unwrap();
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
            if let Some(objects) = self.objects.lock().unwrap().take() {
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
            (host.clone() as Arc<dyn CustomLayerHost>, host.calls.clone())
        });
    }
}

#[cfg(all(unix, not(target_vendor = "apple")))]
mod gl {
    use glow::HasContext as _;
    use mln_style::layers::{CustomLayerPrerenderParameters, CustomLayerRenderParameters};

    use super::{Arc, BackendType, Calls, CustomLayerHost, Mutex, Ordering, check};

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

    #[derive(Default)]
    struct Host {
        calls: Arc<Calls>,
        objects: Mutex<Option<(glow::NativeProgram, glow::NativeVertexArray)>>,
    }

    impl CustomLayerHost for Host {
        fn prerender(&self, parameters: &mut CustomLayerPrerenderParameters<'_>) {
            assert!(parameters.encoder.is::<mln_gl::CustomLayerEncoder>());
            self.calls.prerender(parameters);
        }

        fn render(&self, parameters: &mut CustomLayerRenderParameters<'_>) {
            self.calls.render(parameters);
            let pass = parameters.pass.downcast_ref::<mln_gl::CustomLayerPass>().expect("an OpenGL pass");
            let gl = pass.gl();
            let mut objects = self.objects.lock().unwrap();
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
            self.objects.lock().unwrap().take();
            self.calls.deinitializations.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn a_quad_draws_at_its_place_among_the_layers() {
        check(BackendType::OpenGL, || {
            let host = Arc::new(Host::default());
            (host.clone() as Arc<dyn CustomLayerHost>, host.calls.clone())
        });
    }
}
