#![cfg(not(any(target_vendor = "apple", target_family = "wasm")))]
// Creating the headless surface through ash is the FFI here.
#![allow(unsafe_code)]

// The window target (`SurfaceTarget`) without a window: on a surface of
// VK_EXT_headless_surface, whose swapchain takes and gives images like a
// window's and shows them nowhere. The frames go through the swapchain as
// the desktop app's do (acquire, render pass, overlay, present), and each
// one checked is read back (`request_readback`).
//
// A frame clears to one color and draws a background drawable in another
// over half of it, as background_test.rs does: a frame that shows another
// frame's image, or reads another frame's uniforms, has the wrong colors.
//
// A backend made for a surface keeps two frames in flight, so a uniform buffer
// has a copy for each: the frames that aren't read back aren't waited for,
// and the ones after them read the copy the frame before didn't write.
//
// Without VK_EXT_headless_surface the tests are skipped, and fail on CI.

use std::cell::{Cell, RefCell};
use std::ffi::CStr;
use std::rc::Rc;

use ash::vk;
use mln_gfx::backend_scope::{BackendScope, ScopeType};
use mln_gfx::color_mode::ColorMode;
use mln_gfx::command_encoder::CommandEncoder;
use mln_gfx::context::Context;
use mln_gfx::depth_mode::DepthMode;
use mln_gfx::draw_mode::DrawMode;
use mln_gfx::frame::FrameTarget as _;
use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::paint_parameters::PaintParameters;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::render_pass::{RenderPass as GfxRenderPass, RenderPassDescriptor};
use mln_gfx::renderer::layer_group::LayerGroupBasePtr;
use mln_gfx::renderer::render_pass::RenderPass;
use mln_gfx::renderer_backend::RendererBackend as _;
use mln_gfx::scissor_rect::ScissorRect;
use mln_gfx::shader_registry::ShaderRegistry;
use mln_gfx::shaders::background_layer_ubo::{BackgroundDrawableUBO, BackgroundDrawableUnionUBO, BackgroundPropsUBO};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{
    ID_BACKGROUND_DRAWABLE_UBO, ID_BACKGROUND_POS_VERTEX_ATTRIBUTE, ID_BACKGROUND_PROPS_UBO, ID_PROJECTION_UBO,
};
use mln_gfx::stencil_mode::StencilMode;
use mln_gfx::texture2d::Texture2D as _;
use mln_gfx::types::{DepthMaskType, TextureChannelDataType, TexturePixelType};
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::Color;
use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use mln_vulkan::renderer_backend::SurfaceProvider;
use mln_vulkan::surface_target::{Overlay, OverlayTarget, present_mode};
use mln_vulkan::{Device, RendererBackend, SurfaceTarget};

const EXTENT: i16 = 8192;

/// A surface that no window system shows. It counts the surfaces it was
/// asked for, and makes none while `fail` is set.
#[derive(Default)]
struct HeadlessSurface {
    made: Cell<u32>,
    fail: Cell<bool>,
}

impl SurfaceProvider for HeadlessSurface {
    fn instance_extensions(&self) -> Vec<&'static CStr> {
        vec![ash::khr::surface::NAME, ash::ext::headless_surface::NAME]
    }

    fn create_surface(&self, entry: &ash::Entry, instance: &ash::Instance) -> Result<vk::SurfaceKHR, Error> {
        let loader = ash::ext::headless_surface::Instance::new(entry, instance);
        self.made.set(self.made.get() + 1);
        if self.fail.get() {
            return Err(Error::native("no surface now"));
        }
        // SAFETY: the instance was created with VK_EXT_headless_surface.
        unsafe { loader.create_headless_surface(&vk::HeadlessSurfaceCreateInfoEXT::default(), None) }
            .map_err(|e| Error::native(format!("Failed to create a headless surface: {e}")))
    }
}

/// Why the tests can't run here: no Vulkan, or none with headless surfaces.
fn unavailable() -> Option<String> {
    // SAFETY: the Vulkan library's initializers have no preconditions, and
    // the query is a plain one of the loader.
    let extensions = unsafe {
        let entry = match ash::Entry::load() {
            Ok(entry) => entry,
            Err(e) => return Some(format!("no Vulkan library: {e}")),
        };
        match entry.enumerate_instance_extension_properties(None) {
            Ok(extensions) => extensions,
            Err(e) => return Some(format!("no Vulkan instance extensions: {e}")),
        }
    };
    let has = |name: &CStr| extensions.iter().any(|e| e.extension_name_as_c_str() == Ok(name));
    [ash::khr::surface::NAME, ash::ext::headless_surface::NAME]
        .into_iter()
        .find(|name| !has(name))
        .map(|name| format!("Vulkan has no {}", name.to_string_lossy()))
}

/// A backend made for a headless surface, and a target that presents to it.
struct Window {
    backend: Rc<RendererBackend>,
    target: SurfaceTarget,
}

impl Window {
    /// Takes the next frame; false when there is none to draw into.
    fn acquire_next_texture(&self) -> bool {
        self.target.begin_frame().is_some()
    }

    fn set_framebuffer_size(&self, size: Size) {
        self.target.resize(size);
    }

    fn get_framebuffer_size(&self) -> Size {
        self.target.size()
    }

    fn request_readback(&self) {
        self.target.request_readback();
    }

    fn take_readback(&self) -> Option<PremultipliedImage> {
        self.target.take_readback()
    }

    fn set_overlay(&self, overlay: Option<Overlay>) {
        self.target.set_overlay(overlay);
    }
}

impl std::ops::Deref for Window {
    type Target = RendererBackend;

    fn deref(&self) -> &RendererBackend {
        &self.backend
    }
}

/// A backend and a target on a headless surface that `provider` makes, or
/// `None` where there can't be one, which fails the test on CI.
fn surface_backend_on(provider: Rc<HeadlessSurface>, size: Size, cap_frame_rate: bool) -> Option<Window> {
    if let Some(reason) = unavailable() {
        assert!(std::env::var_os("CI").is_none(), "the swapchain tests can't run on CI: {reason}");
        eprintln!("skipping the swapchain test: {reason}");
        return None;
    }
    let device = Device::new(None).expect("Vulkan device");
    let (backend, target) =
        SurfaceTarget::with_backend_from_provider(&device, provider, size, cap_frame_rate).expect("Vulkan backend");
    eprintln!("rendering with {}", backend.get_device_name());
    Some(Window { backend, target })
}

fn surface_backend(size: Size, cap_frame_rate: bool) -> Option<Window> {
    surface_backend_on(Rc::default(), size, cap_frame_rate)
}

/// `PaintParameters::numSublayers` and `depthEpsilon`.
const NUM_SUBLAYERS: u32 = 3;
const DEPTH_EPSILON: f32 = 1.0 / (1 << 16) as f32;
/// `1 - (numLayerGroups() + 2) * numSublayers * depthEpsilon` for one layer group.
const DEPTH_RANGE_SIZE: f32 = 1.0 - ((1 + 2) * NUM_SUBLAYERS) as f32 * DEPTH_EPSILON;

/// The gfx-facing part of the renderer's `PaintParameters`, as in
/// `background_test.rs`.
struct TestPaintParameters<'a> {
    context: &'a dyn Context,
    encoder: Box<dyn CommandEncoder>,
    render_pass: Option<Box<dyn GfxRenderPass>>,
    pass: RenderPass,
    scissor_rect: ScissorRect,
}

impl PaintParameters for TestPaintParameters<'_> {
    fn context(&self) -> &dyn Context {
        self.context
    }
    fn encoder(&self) -> Option<&dyn CommandEncoder> {
        Some(&*self.encoder)
    }
    fn render_pass(&self) -> Option<&dyn GfxRenderPass> {
        self.render_pass.as_deref()
    }
    fn pass(&self) -> RenderPass {
        self.pass
    }
    fn scissor_rect(&self) -> ScissorRect {
        self.scissor_rect
    }
    fn depth_mode_for_sublayer(&self, n: u8, mask: DepthMaskType) -> DepthMode {
        let depth = DEPTH_RANGE_SIZE + (NUM_SUBLAYERS + u32::from(n)) as f32 * DEPTH_EPSILON;
        DepthMode {
            func: mln_gfx::types::DepthFunctionType::LessEqual,
            mask,
            range: mln_util::range::Range::new(depth, depth),
        }
    }
    fn depth_mode_for_3d(&self, mask: DepthMaskType) -> DepthMode {
        DepthMode {
            func: mln_gfx::types::DepthFunctionType::LessEqual,
            mask,
            range: mln_util::range::Range::new(0.0, DEPTH_RANGE_SIZE),
        }
    }
    fn render_tile_clipping_masks(&mut self, _tile_ids: &[UnwrappedTileID]) -> bool {
        true
    }
    fn stencil_mode_for_clipping(&self, _tile_id: &UnwrappedTileID) -> StencilMode {
        StencilMode::disabled()
    }
    fn stencil_mode_for_3d(&mut self) -> StencilMode {
        StencilMode::disabled()
    }
    fn stencil_modes_for_3d(&mut self, modes: &mut [StencilMode]) -> bool {
        modes.fill(StencilMode::disabled());
        true
    }
}

/// Column-major matrix mapping the tile to the left or the right half of
/// the viewport.
fn half_matrix(right: bool) -> [f32; 16] {
    let e = f32::from(EXTENT);
    let x = if right { 0.0 } else { -1.0 };
    [1.0 / e, 0.0, 0.0, 0.0, 0.0, -2.0 / e, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, x, 1.0, 0.0, 1.0]
}

/// One opaque background drawable in a tile layer group, which keeps its
/// uniform buffers from frame to frame, as a layer of the renderer does.
struct Scene {
    /// Keeps the shader's group.
    _registry: ShaderRegistry,
    layer_group: LayerGroupBasePtr,
}

impl Scene {
    fn new(backend: &Window) -> Scene {
        let registry = ShaderRegistry::new();
        backend.init_shaders(&registry, &ProgramParameters::new(1.0, false));
        let context = backend.get_context();
        let layer_group = context.create_tile_layer_group(0, 64, "background".to_owned()).expect("layer group");
        let shader = context
            .get_generic_shader(&registry, "BackgroundShader", ProjectionVariant::Mercator)
            .expect("BackgroundShader");

        let tile_vertices: [[i16; 2]; 4] = [[0, 0], [EXTENT, 0], [0, EXTENT], [EXTENT, EXTENT]];
        let passes = RenderPass::Opaque | RenderPass::Translucent;
        let mut builder = context.create_drawable_builder("background".to_owned());
        builder.set_render_pass(passes);
        builder.set_shader(Some(shader));
        builder.set_depth_type(DepthMaskType::ReadWrite);
        builder.set_color_mode(&ColorMode::unblended());
        builder.set_vertex_attr_id(ID_BACKGROUND_POS_VERTEX_ATTRIBUTE);
        builder.set_raw_vertices(
            bytemuck::cast_slice(&tile_vertices).to_vec(),
            tile_vertices.len(),
            AttributeDataType::Short2,
        );
        builder.set_segments(DrawMode::Triangles(), vec![0, 1, 2, 1, 2, 3], &[SegmentBase::with_lengths(0, 0, 4, 6)]);
        builder.flush(context);

        let tile_id = OverscaledTileID::from_zxy(0, 0, 0);
        {
            let mut group = layer_group.borrow_mut();
            let tile_group = group.as_tile_layer_group_mut().expect("tile layer group");
            for mut drawable in builder.clear_drawables() {
                drawable.set_tile_id(tile_id);
                drawable.set_ubo_index(0);
                tile_group.add_drawable(passes, &tile_id, drawable);
            }
        }
        Scene { _registry: registry, layer_group }
    }

    /// Writes the background's color into the layer's uniform buffer: the
    /// buffer the frames before used, once it exists.
    fn set_color(&self, context: &dyn Context, color: Color) {
        let props = BackgroundPropsUBO {
            color: [color.r, color.g, color.b, color.a],
            opacity: 1.0,
            pad1: 0.0,
            pad2: 0.0,
            pad3: 0.0,
        };
        self.layer_group.borrow_mut().mutable_uniform_buffers().create_or_update_pod(
            ID_BACKGROUND_PROPS_UBO,
            &props,
            context,
        );
    }

    /// Writes the half the background covers into the layer's storage
    /// buffers: the buffers the frames before used, once they exist.
    fn set_half(&self, context: &dyn Context, right: bool) {
        let matrix = half_matrix(right);
        let drawable = [BackgroundDrawableUnionUBO::from_drawable(&BackgroundDrawableUBO { matrix })];
        let projection = [ProjectionUBO { matrix, fallback_matrix: matrix, ..ProjectionUBO::default() }];
        let mut group = self.layer_group.borrow_mut();
        let buffers = group.mutable_uniform_buffers();
        for (id, data) in [
            (ID_BACKGROUND_DRAWABLE_UBO, bytemuck::cast_slice::<_, u8>(&drawable)),
            (ID_PROJECTION_UBO, bytemuck::cast_slice::<_, u8>(&projection)),
        ] {
            match buffers.get(id) {
                Some(buffer) => buffer.update(data),
                None => {
                    buffers.set(id, Some(context.create_uniform_buffer(data, false, true)));
                }
            }
        }
    }
}

/// What a frame changes before it draws. `None` leaves the uniform buffers
/// as the frame before had them.
#[derive(Clone, Copy, Default)]
struct Change {
    color: Option<Color>,
    right: Option<bool>,
}

/// One frame as the desktop app runs it: the backend says whether it can
/// draw, the frame begins (which acquires the swapchain's image), uploads,
/// draws the scene over `clear` and presents. With `read`, the presented
/// frame is read back.
fn render_frame(
    backend: &Window,
    scene: Option<&Scene>,
    change: Change,
    clear: Color,
    read: bool,
) -> Option<PremultipliedImage> {
    assert!(backend.acquire_next_texture());
    if read {
        backend.request_readback();
        assert!(backend.take_readback().is_none(), "nothing is read before the frame is presented");
    }
    let context = backend.get_context();
    let image = backend.target.begin_frame().expect("a frame");
    let renderable = &*image;
    context.begin_frame();

    let size = renderable.size();
    let mut parameters = TestPaintParameters {
        context,
        encoder: context.create_command_encoder(),
        render_pass: None,
        pass: RenderPass::Opaque,
        scissor_rect: ScissorRect { x: 0, y: 0, width: size.width, height: size.height },
    };
    if let Some(scene) = scene {
        let upload_pass = parameters.encoder.create_upload_pass("layerGroup-upload", renderable);
        if let Some(color) = change.color {
            scene.set_color(context, color);
        }
        if let Some(right) = change.right {
            scene.set_half(context, right);
        }
        scene.layer_group.borrow_mut().upload(&*upload_pass);
    }
    parameters.render_pass = Some(parameters.encoder.create_render_pass(
        "main buffer",
        &RenderPassDescriptor {
            target: renderable,
            clear_color: Some(clear),
            clear_depth: Some(1.0),
            clear_stencil: Some(0),
        },
    ));
    context.bind_global_uniform_buffers(parameters.render_pass.as_deref().unwrap());
    if let Some(scene) = scene {
        for pass in [RenderPass::Opaque, RenderPass::Translucent] {
            parameters.pass = pass;
            scene.layer_group.borrow_mut().render(&mut parameters);
        }
    }
    context.unbind_global_uniform_buffers(parameters.render_pass.as_deref().unwrap());
    parameters.render_pass = None;
    backend.target.end_frame(&*parameters.encoder);
    context.end_frame();

    let image = backend.take_readback();
    assert_eq!(image.is_some(), read, "a frame is read back when asked, once");
    assert!(backend.take_readback().is_none(), "a frame is read back once");
    image
}

fn pixel(image: &PremultipliedImage, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * image.size.width + x) * 4) as usize;
    image.data[i..i + 4].try_into().unwrap()
}

fn rgba(color: Color) -> [u8; 4] {
    [color.r, color.g, color.b, color.a].map(|c| (c * 255.0).round() as u8)
}

fn assert_near(actual: [u8; 4], expected: [u8; 4], what: &str) {
    for c in 0..4 {
        assert!(
            (i16::from(actual[c]) - i16::from(expected[c])).abs() <= 1,
            "{what}: got {actual:?}, expected {expected:?}"
        );
    }
}

/// Checks the image's size, and `left` and `right` in its halves.
fn check_halves(image: &PremultipliedImage, size: Size, left: Color, right: Color, what: &str) {
    assert_eq!(image.size, size, "{what}");
    let (w, h) = (size.width, size.height);
    for y in [0, h / 2, h - 1] {
        for x in [0, w / 4, w / 2 - 1] {
            assert_near(pixel(image, x, y), rgba(left), &format!("{what}, left half at ({x}, {y})"));
        }
        for x in [w / 2, 3 * w / 4, w - 1] {
            assert_near(pixel(image, x, y), rgba(right), &format!("{what}, right half at ({x}, {y})"));
        }
    }
}

fn color(r: f32, g: f32, b: f32) -> Color {
    Color::new(r, g, b, 1.0)
}

/// A color no other frame of a test has.
fn frame_color(frame: u32, offset: u32) -> Color {
    let step = |n: u32| (n % 6) as f32 * 0.2;
    color(step(frame + offset), step(frame * 2 + offset + 1), step(frame * 3 + offset + 2))
}

#[test]
fn present_mode_follows_the_frame_rate_cap() {
    use vk::PresentModeKHR as Mode;
    let all = [Mode::FIFO, Mode::FIFO_RELAXED, Mode::MAILBOX, Mode::IMMEDIATE];
    // Capped: mailbox, else the mode every surface has; never tearing.
    assert_eq!(present_mode(true, &all), Mode::MAILBOX);
    assert_eq!(present_mode(true, &[Mode::FIFO, Mode::IMMEDIATE]), Mode::FIFO);
    assert_eq!(present_mode(true, &[]), Mode::FIFO);
    // Uncapped: immediate, else mailbox, else the mode every surface has.
    assert_eq!(present_mode(false, &all), Mode::IMMEDIATE);
    assert_eq!(present_mode(false, &[Mode::FIFO, Mode::MAILBOX]), Mode::MAILBOX);
    assert_eq!(present_mode(false, &[Mode::FIFO, Mode::FIFO_RELAXED]), Mode::FIFO);
    assert_eq!(present_mode(false, &[]), Mode::FIFO);
}

#[test]
fn frames_present_what_they_draw() {
    // Each frame has its own clear color and background color, and moves
    // the background to the other half: an image or a uniform of another
    // frame shows. With and without the frame rate cap, which present in
    // different modes.
    for cap_frame_rate in [true, false] {
        let size = Size::new(64, 32);
        let Some(backend) = surface_backend(size, cap_frame_rate) else {
            return;
        };
        let _scope = BackendScope::new(&*backend.backend, ScopeType::Explicit);
        assert_eq!(backend.get_framebuffer_size(), size);
        assert_ne!(backend.target.get_color_format(), vk::Format::UNDEFINED);
        let scene = Scene::new(&backend);
        // More frames than the swapchain has images.
        for frame in 0..12 {
            let (clear, background) = (frame_color(frame, 0), frame_color(frame, 3));
            let right = frame % 2 == 1;
            let change = Change { color: Some(background), right: Some(right) };
            let image = render_frame(&backend, Some(&scene), change, clear, true).unwrap();
            let (left_color, right_color) = if right { (clear, background) } else { (background, clear) };
            check_halves(&image, size, left_color, right_color, &format!("frame {frame}"));
        }
    }
}

#[test]
fn uniforms_follow_each_frame_in_flight() {
    let size = Size::new(32, 16);
    let Some(backend) = surface_backend(size, false) else {
        return;
    };
    let _scope = BackendScope::new(&*backend.backend, ScopeType::Explicit);
    let scene = Scene::new(&backend);
    let clear = color(0.0, 0.0, 0.0);
    let (red, green, blue) = (color(1.0, 0.0, 0.0), color(0.0, 1.0, 0.0), color(0.0, 0.0, 1.0));
    let set = |color: Color, right: bool| Change { color: Some(color), right: Some(right) };
    let keep = Change::default();

    // (what the frame changes, whether it is read back, the background's
    // color and half it must show). The frames that aren't read back are
    // still in flight when the next one is recorded.
    let frames = [
        (set(red, false), false, red, false),
        // Nothing written: the other frame's copies of the buffers are
        // brought up to date.
        (keep, true, red, false),
        (keep, true, red, false),
        // Two writes in a row, one in each frame's copies, then none.
        (set(green, true), false, green, true),
        (set(blue, false), false, blue, false),
        (keep, true, blue, false),
        (keep, true, blue, false),
        // One buffer written, the others kept.
        (Change { color: Some(green), right: None }, true, green, false),
        (Change { color: None, right: Some(true) }, false, green, true),
        (keep, true, green, true),
        // The same contents written again.
        (set(green, true), true, green, true),
        (set(red, true), true, red, true),
        (set(red, false), false, red, false),
        (set(blue, false), true, blue, false),
    ];
    for (index, (change, read, background, right)) in frames.into_iter().enumerate() {
        let image = render_frame(&backend, Some(&scene), change, clear, read);
        if let Some(image) = image {
            let (left_color, right_color) = if right { (clear, background) } else { (background, clear) };
            check_halves(&image, size, left_color, right_color, &format!("frame {index}"));
        }
    }
}

#[test]
fn resize_rebuilds_the_swapchain() {
    let Some(backend) = surface_backend(Size::new(64, 32), true) else {
        return;
    };
    let _scope = BackendScope::new(&*backend.backend, ScopeType::Explicit);
    let scene = Scene::new(&backend);
    let mut frame = 0;
    let mut draw = |size: Size, read: bool| {
        frame += 1;
        let (clear, background) = (frame_color(frame, 1), frame_color(frame, 4));
        let change = Change { color: Some(background), right: Some(false) };
        if let Some(image) = render_frame(&backend, Some(&scene), change, clear, read) {
            check_halves(&image, size, background, clear, &format!("frame {frame} at {size:?}"));
        }
    };

    let mut size = Size::new(64, 32);
    draw(size, true);
    // Growing, shrinking, and a size the swapchain already has, each with
    // frames in flight from before, and frames after.
    for next in [Size::new(96, 48), Size::new(24, 40), Size::new(24, 40), Size::new(128, 64), Size::new(64, 32)] {
        draw(size, false);
        backend.set_framebuffer_size(next);
        size = next;
        assert_eq!(backend.get_framebuffer_size(), size);
        draw(size, true);
        draw(size, false);
        draw(size, true);
    }
    // Several sizes between two frames: the last one counts.
    backend.set_framebuffer_size(Size::new(10, 10));
    backend.set_framebuffer_size(Size::new(80, 20));
    draw(Size::new(80, 20), true);
}

#[test]
fn empty_framebuffer_draws_nothing() {
    let size = Size::new(64, 32);
    let Some(backend) = surface_backend(size, true) else {
        return;
    };
    let _scope = BackendScope::new(&*backend.backend, ScopeType::Explicit);
    let scene = Scene::new(&backend);
    let (clear, red, green) = (color(0.2, 0.4, 0.6), color(1.0, 0.0, 0.0), color(0.0, 1.0, 0.0));
    let set = |color: Color| Change { color: Some(color), right: Some(true) };
    let image = render_frame(&backend, Some(&scene), set(red), clear, true).unwrap();
    check_halves(&image, size, clear, red, "before");

    // A minimized window: no frame can be drawn, and the app draws none.
    for empty in [Size::new(0, 0), Size::new(0, 32), Size::new(64, 0)] {
        backend.set_framebuffer_size(empty);
        assert_eq!(backend.get_framebuffer_size(), empty);
        assert!(!backend.acquire_next_texture());
    }
    // A readback asked for meanwhile is of the next frame drawn.
    backend.request_readback();
    assert!(backend.take_readback().is_none());

    // Back at the size it had, and then at another.
    backend.set_framebuffer_size(size);
    let image = render_frame(&backend, Some(&scene), set(green), clear, true).unwrap();
    check_halves(&image, size, clear, green, "after, at the same size");
    backend.set_framebuffer_size(Size::new(0, 0));
    assert!(!backend.acquire_next_texture());
    let size = Size::new(40, 24);
    backend.set_framebuffer_size(size);
    let image = render_frame(&backend, Some(&scene), set(red), clear, true).unwrap();
    check_halves(&image, size, clear, red, "after, at another size");
}

/// What the test's overlay draws: `COLOR` over the top left quarter.
const OVERLAY_COLOR: [f32; 4] = [1.0, 1.0, 0.0, 1.0];

/// An overlay that fills the top left quarter of the frame, and makes a
/// texture and a buffer with the frame's context as an app's overlay does.
/// It counts its frames in `calls`.
fn quarter_overlay(calls: Rc<Cell<u32>>, sizes: Rc<RefCell<Vec<Size>>>) -> Overlay {
    let mut texture = None;
    Box::new(move |target: &OverlayTarget<'_>| {
        calls.set(calls.get() + 1);
        sizes.borrow_mut().push(target.size);
        assert_ne!(target.render_pass, vk::RenderPass::null());

        // A texture uploaded inside the overlay's pass, kept for the next
        // frames, and a buffer dropped while the frame still uses it.
        let texture = texture.get_or_insert_with(|| target.context.create_vulkan_texture_2d());
        {
            let mut texture = texture.borrow_mut();
            texture.set_format(TexturePixelType::RGBA, TextureChannelDataType::UnsignedByte);
            texture.upload_data(Some(&[0xFF; 16]), Size::new(2, 2));
            assert_ne!(texture.get_vulkan_image_view(), vk::ImageView::null());
            assert_ne!(texture.get_vulkan_sampler(), vk::Sampler::null());
        }
        let buffer = target.context.create_buffer(Some(&[0; 64]), 64, vk::BufferUsageFlags::VERTEX_BUFFER, false);
        assert!(buffer.is_valid());

        let attachment = vk::ClearAttachment::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .color_attachment(0)
            .clear_value(vk::ClearValue { color: vk::ClearColorValue { float32: OVERLAY_COLOR } });
        let rect = vk::ClearRect::default().layer_count(1).rect(vk::Rect2D {
            offset: vk::Offset2D::default(),
            extent: vk::Extent2D { width: target.size.width / 2, height: target.size.height / 2 },
        });
        // SAFETY: the command buffer is in the overlay's render pass, whose
        // color attachment the rectangle is inside, and the vertex buffer
        // is destroyed when the frame is done.
        unsafe {
            target.device.cmd_bind_vertex_buffers(target.command_buffer, 0, &[buffer.get_vulkan_buffer()], &[0]);
            target.device.cmd_clear_attachments(target.command_buffer, &[attachment], &[rect]);
        }
    })
}

/// Checks a frame the quarter overlay drew over: the overlay's color in the
/// top left quarter, and the frame's own colors in the rest of its halves.
fn check_overlaid(image: &PremultipliedImage, size: Size, left: Color, right: Color, what: &str) {
    assert_eq!(image.size, size, "{what}");
    let (w, h) = (size.width, size.height);
    let overlay = OVERLAY_COLOR.map(|c| (c * 255.0).round() as u8);
    for (x, y) in [(0, 0), (w / 4, h / 4), (w / 2 - 1, h / 2 - 1)] {
        assert_near(pixel(image, x, y), overlay, &format!("{what}, overlay at ({x}, {y})"));
    }
    for (x, y) in [(0, h / 2), (w / 4, h - 1), (w / 2 - 1, h / 2)] {
        assert_near(pixel(image, x, y), rgba(left), &format!("{what}, left half at ({x}, {y})"));
    }
    for (x, y) in [(w / 2, 0), (3 * w / 4, h / 2), (w - 1, h - 1)] {
        assert_near(pixel(image, x, y), rgba(right), &format!("{what}, right half at ({x}, {y})"));
    }
}

#[test]
fn overlay_draws_over_the_frame_and_is_read_back() {
    let mut size = Size::new(64, 32);
    let Some(backend) = surface_backend(size, true) else {
        return;
    };
    let _scope = BackendScope::new(&*backend.backend, ScopeType::Explicit);
    let scene = Scene::new(&backend);
    let (calls, sizes) = (Rc::new(Cell::new(0)), Rc::new(RefCell::new(Vec::new())));
    let (clear, blue, green) = (color(0.2, 0.2, 0.2), color(0.0, 0.0, 1.0), color(0.0, 1.0, 0.0));
    let set = |color: Color| Change { color: Some(color), right: Some(false) };

    // Without an overlay, the frame is the scene's.
    let image = render_frame(&backend, Some(&scene), set(blue), clear, true).unwrap();
    check_halves(&image, size, blue, clear, "no overlay");

    // With one, every frame has it over the scene, read back or not, and
    // after a resize too.
    backend.set_overlay(Some(quarter_overlay(calls.clone(), sizes.clone())));
    let image = render_frame(&backend, Some(&scene), set(green), clear, true).unwrap();
    check_overlaid(&image, size, green, clear, "overlay");
    render_frame(&backend, Some(&scene), set(blue), clear, false);
    let image = render_frame(&backend, Some(&scene), set(blue), clear, true).unwrap();
    check_overlaid(&image, size, blue, clear, "overlay, third frame");
    assert_eq!(calls.get(), 3);

    size = Size::new(40, 48);
    backend.set_framebuffer_size(size);
    let image = render_frame(&backend, Some(&scene), set(green), clear, true).unwrap();
    check_overlaid(&image, size, green, clear, "overlay, resized");
    assert_eq!(calls.get(), 4);
    assert_eq!(*sizes.borrow(), [Size::new(64, 32), Size::new(64, 32), Size::new(64, 32), size]);

    // An overlay over a frame with nothing else in it.
    let image = render_frame(&backend, None, Change::default(), clear, true).unwrap();
    check_overlaid(&image, size, clear, clear, "overlay over the clear color");

    // Another overlay replaces it, and none stops it.
    let (other_calls, other_sizes) = (Rc::new(Cell::new(0)), Rc::new(RefCell::new(Vec::new())));
    backend.set_overlay(Some(quarter_overlay(other_calls.clone(), other_sizes)));
    let image = render_frame(&backend, Some(&scene), set(blue), clear, true).unwrap();
    check_overlaid(&image, size, blue, clear, "second overlay");
    assert_eq!((calls.get(), other_calls.get()), (5, 1));
    backend.set_overlay(None);
    let image = render_frame(&backend, Some(&scene), set(green), clear, true).unwrap();
    check_halves(&image, size, green, clear, "overlay removed");
    assert_eq!((calls.get(), other_calls.get()), (5, 1));
}

#[test]
fn dropped_with_frames_in_flight_then_made_again() {
    let size = Size::new(48, 24);
    let (clear, red, green) = (color(0.0, 0.0, 0.4), color(1.0, 0.0, 0.0), color(0.0, 1.0, 0.0));
    let set = |color: Color| Change { color: Some(color), right: Some(false) };
    {
        let Some(backend) = surface_backend(size, false) else {
            return;
        };
        let _scope = BackendScope::new(&*backend.backend, ScopeType::Explicit);
        let scene = Scene::new(&backend);
        backend.set_overlay(Some(quarter_overlay(Rc::default(), Rc::default())));
        // Nothing waits for these frames, a resize among them; the
        // overlay, its texture and the scene go with the backend.
        for frame in 0..5 {
            if frame == 3 {
                backend.set_framebuffer_size(Size::new(64, 64));
            }
            render_frame(&backend, Some(&scene), set(frame_color(frame, 0)), clear, false);
        }
        // A readback that no frame came to.
        backend.request_readback();
        drop(scene);
    }

    // A backend dropped before it drew anything.
    drop(surface_backend(size, true));

    // Another one in the same process draws as the first.
    let Some(backend) = surface_backend(size, true) else {
        return;
    };
    let _scope = BackendScope::new(&*backend.backend, ScopeType::Explicit);
    let scene = Scene::new(&backend);
    let image = render_frame(&backend, Some(&scene), set(red), clear, true).unwrap();
    check_halves(&image, size, red, clear, "second backend");
    let image = render_frame(&backend, Some(&scene), set(green), clear, true).unwrap();
    check_halves(&image, size, green, clear, "second backend, second frame");
}

#[test]
fn a_new_target_on_the_backend_presents() {
    // What a session does with a surface the host gives it in place of
    // another: the old target's swapchain and surface go, the backend and
    // what the renderer made on it stay, and the new target presents.
    let provider = Rc::new(HeadlessSurface::default());
    let size = Size::new(48, 32);
    let Some(mut window) = surface_backend_on(provider.clone(), size, true) else {
        return;
    };
    let backend = window.backend.clone();
    let _scope = BackendScope::new(&*backend, ScopeType::Explicit);
    let scene = Scene::new(&window);
    let (clear, red, green) = (color(0.4, 0.4, 0.4), color(1.0, 0.0, 0.0), color(0.0, 1.0, 0.0));
    let set = |color: Color| Change { color: Some(color), right: Some(true) };
    assert_eq!(provider.made.get(), 1);

    // Replaced with a frame in flight: the scene and its uniforms are as
    // they were.
    render_frame(&window, Some(&scene), set(red), clear, false);
    window.target = SurfaceTarget::from_provider(backend.clone(), provider.clone(), size, true).expect("target");
    assert_eq!(provider.made.get(), 2);
    let image = render_frame(&window, Some(&scene), Change::default(), clear, true).unwrap();
    check_halves(&image, size, clear, red, "new target");

    // At another size.
    let size = Size::new(80, 24);
    window.target = SurfaceTarget::from_provider(backend.clone(), provider.clone(), size, true).expect("target");
    let image = render_frame(&window, Some(&scene), set(green), clear, true).unwrap();
    check_halves(&image, size, clear, green, "new target at another size");

    // A surface that can't be made leaves the target the window has.
    provider.fail.set(true);
    assert!(SurfaceTarget::from_provider(backend.clone(), provider.clone(), size, true).is_err());
    provider.fail.set(false);
    let image = render_frame(&window, Some(&scene), set(red), clear, true).unwrap();
    check_halves(&image, size, clear, red, "after a failed target");
}

#[test]
fn readback_is_of_the_frame_asked_for() {
    let size = Size::new(16, 16);
    let Some(backend) = surface_backend(size, true) else {
        return;
    };
    let _scope = BackendScope::new(&*backend.backend, ScopeType::Explicit);
    let (first, second, third) = (color(1.0, 0.0, 0.0), color(0.0, 1.0, 0.0), color(0.0, 0.0, 1.0));
    // No frame is read without a request.
    assert!(backend.take_readback().is_none());
    render_frame(&backend, None, Change::default(), first, false);
    assert!(backend.take_readback().is_none());
    // The frame after the request is the one read, not one before or after.
    let image = render_frame(&backend, None, Change::default(), second, true).unwrap();
    check_halves(&image, size, second, second, "the frame asked for");
    render_frame(&backend, None, Change::default(), third, false);
    assert!(backend.take_readback().is_none());
    let image = render_frame(&backend, None, Change::default(), third, true).unwrap();
    check_halves(&image, size, third, third, "the next frame asked for");
}
