// The background shader end to end on the drawable path, as
// src/mln/renderer/layers/render_background_layer.cpp builds the drawable,
// src/mln/renderer/layers/background_layer_tweaker.cpp sets its uniforms
// (the MLN_UBO_CONSOLIDATION branch WebGPU uses) and
// src/mln/renderer/renderer_impl.cpp runs the frame: upload pass, main render
// pass with the clear color, opaque/translucent layer group render, present,
// headless readback.
//
// The tile matrix is a hand-made one that maps the tile's x extent to the
// left half of the viewport, so a single frame checks both the drawn
// background color and the clear color.

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
use mln_gfx::renderer::layer_group::{LayerGroupBasePtr, visit_layer_group_drawables};
use mln_gfx::renderer::render_pass::RenderPass;
use mln_gfx::renderer_backend::RendererBackend as _;
use mln_gfx::scissor_rect::ScissorRect;
use mln_gfx::shader_registry::ShaderRegistry;
use mln_gfx::shaders::background_layer_ubo::{BackgroundDrawableUBO, BackgroundDrawableUnionUBO, BackgroundPropsUBO};
use mln_gfx::shaders::clipping_mask_ubo::ClipUBO;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{
    ID_BACKGROUND_DRAWABLE_UBO, ID_BACKGROUND_POS_VERTEX_ATTRIBUTE, ID_BACKGROUND_PROPS_UBO, ID_PROJECTION_UBO,
};
use mln_gfx::stencil_mode::{StencilMode, Test};
use mln_gfx::types::{DepthMaskType, StencilOpType};
use mln_gfx::uniform_buffer::UniformBufferPtr;
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::Color;
use mln_util::range::Range;
use mln_util::size::Size;

use crate::{Device, OffscreenTarget, RendererBackend};

/// A backend on a device of its own, and an offscreen target on it.
struct Headless {
    backend: std::rc::Rc<RendererBackend>,
    target: OffscreenTarget,
}

impl Headless {
    fn new(size: Size) -> Headless {
        let device = Device::new(None).expect("WebGPU device");
        let backend = std::rc::Rc::new(RendererBackend::new(&device));
        Headless { target: OffscreenTarget::new(backend.clone(), size), backend }
    }

    /// The last frame's pixels.
    fn read_still_image(&self) -> mln_util::image::PremultipliedImage {
        self.target.read_image().expect("a frame")
    }
}

impl std::ops::Deref for Headless {
    type Target = RendererBackend;

    fn deref(&self) -> &RendererBackend {
        &self.backend
    }
}

const EXTENT: i16 = 8192;

const NUM_SUBLAYERS: u32 = 3;
const DEPTH_EPSILON: f32 = 1.0 / (1 << 16) as f32;
/// `1 - (layer_groups + 2) * NUM_SUBLAYERS * DEPTH_EPSILON`, for one layer
/// group.
const DEPTH_RANGE_SIZE: f32 = 1.0 - ((1 + 2) * NUM_SUBLAYERS) as f32 * DEPTH_EPSILON;

/// The gfx-facing part of the renderer's `PaintParameters`, with the values
/// `renderer_impl.cpp` uses for a plain 2D frame.
struct TestPaintParameters<'a> {
    context: &'a dyn Context,
    encoder: Box<dyn CommandEncoder>,
    render_pass: Option<Box<dyn GfxRenderPass>>,
    pass: RenderPass,
    scissor_rect: ScissorRect,
    current_layer: u32,
    opaque_pass_cutoff: u32,
    /// What `stencil_mode_for_clipping` returns (one clipped tile).
    clip_stencil: StencilMode,
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
    /// `paint_parameters.cpp`, with `renderer_impl.cpp`'s `depthRangeSize` for
    /// one layer group
    fn depth_mode_for_sublayer(&self, n: u8, mask: DepthMaskType) -> DepthMode {
        if self.current_layer < self.opaque_pass_cutoff {
            return DepthMode::disabled();
        }
        let depth = DEPTH_RANGE_SIZE + ((1 + self.current_layer) * NUM_SUBLAYERS + u32::from(n)) as f32 * DEPTH_EPSILON;
        DepthMode { func: mln_gfx::types::DepthFunctionType::LessEqual, mask, range: Range::new(depth, depth) }
    }
    fn depth_mode_for_3d(&self, mask: DepthMaskType) -> DepthMode {
        DepthMode { func: mln_gfx::types::DepthFunctionType::LessEqual, mask, range: Range::new(0.0, DEPTH_RANGE_SIZE) }
    }
    fn render_tile_clipping_masks(&mut self, _tile_ids: &[UnwrappedTileID]) -> bool {
        true
    }
    fn stencil_mode_for_clipping(&self, _tile_id: &UnwrappedTileID) -> StencilMode {
        self.clip_stencil
    }
    fn stencil_mode_for_3d(&mut self) -> StencilMode {
        StencilMode::disabled()
    }
    fn stencil_modes_for_3d(&mut self, modes: &mut [StencilMode]) -> bool {
        modes.fill(StencilMode::disabled());
        true
    }
}

struct Background {
    color: Color,
    opacity: f32,
}

/// `render_background_layer.cpp` `update` for one tile: the tile layer group
/// with one background drawable.
fn build_layer_group(context: &dyn Context, registry: &ShaderRegistry, passes: RenderPass) -> LayerGroupBasePtr {
    let layer_group = context.create_tile_layer_group(0, 64, "background".to_owned()).expect("layer group");
    let shader = context
        .get_generic_shader(registry, "BackgroundShader", ProjectionVariant::Mercator)
        .expect("BackgroundShader");

    // RenderStaticData::tileVertices(), quadTriangleIndices(), tileTriangleSegments()
    let tile_vertices: [[i16; 2]; 4] = [[0, 0], [EXTENT, 0], [0, EXTENT], [EXTENT, EXTENT]];
    let raw_vertices: Vec<u8> = bytemuck::cast_slice(&tile_vertices).to_vec();
    let indexes: Vec<u16> = vec![0, 1, 2, 1, 2, 3];
    let segs = [SegmentBase::with_lengths(0, 0, 4, 6)];

    let mut builder = context.create_drawable_builder();
    builder.set_render_pass(passes);
    builder.set_shader(Some(shader));
    builder.set_depth_type(DepthMaskType::ReadWrite);
    builder.set_color_mode(&if passes == RenderPass::Translucent {
        ColorMode::alpha_blended()
    } else {
        ColorMode::unblended()
    });
    builder.set_vertex_attr_id(ID_BACKGROUND_POS_VERTEX_ATTRIBUTE);
    builder.set_raw_vertices(raw_vertices, tile_vertices.len(), AttributeDataType::Short2);
    builder.set_segments(DrawMode::Triangles(), indexes, &segs);
    builder.flush(context);

    let tile_id = OverscaledTileID::from_zxy(0, 0, 0);
    {
        let mut group = layer_group.borrow_mut();
        let tile_group = group.as_tile_layer_group_mut().expect("tile layer group");
        for mut drawable in builder.clear_drawables() {
            drawable.set_tile_id(tile_id);
            tile_group.add_drawable(passes, &tile_id, drawable);
        }
        assert_eq!(tile_group.get_drawable_count_for(passes, &tile_id), 1);
    }
    layer_group
}

/// `background_layer_tweaker.cpp` `execute` (non-pattern, UBO consolidation).
fn tweak(context: &dyn Context, layer_group: &LayerGroupBasePtr, bg: &Background, matrix: [f32; 16]) {
    let mut group = layer_group.borrow_mut();
    let props_ubo = BackgroundPropsUBO {
        color: [bg.color.r, bg.color.g, bg.color.b, bg.color.a],
        opacity: bg.opacity,
        pad1: 0.0,
        pad2: 0.0,
        pad3: 0.0,
    };
    group.mutable_uniform_buffers().create_or_update_pod(ID_BACKGROUND_PROPS_UBO, &props_ubo, context);

    let mut drawable_ubo_vector: Vec<BackgroundDrawableUnionUBO> = Vec::new();
    let mut projection_ubo_vector: Vec<ProjectionUBO> = Vec::new();
    let mut i = 0u32;
    visit_layer_group_drawables(&mut *group, |drawable| {
        assert!(drawable.get_tile_id().is_some());
        drawable_ubo_vector.push(BackgroundDrawableUnionUBO::from_drawable(&BackgroundDrawableUBO { matrix }));
        // The vertex shader projects with the projection block, which on
        // Mercator carries the same matrix.
        projection_ubo_vector.push(ProjectionUBO { matrix, fallback_matrix: matrix, ..ProjectionUBO::default() });
        drawable.set_ubo_index(i);
        i += 1;
    });
    let drawable_uniform_buffer: UniformBufferPtr =
        context.create_uniform_buffer(bytemuck::cast_slice(&drawable_ubo_vector), false, true);
    group.mutable_uniform_buffers().set(ID_BACKGROUND_DRAWABLE_UBO, Some(drawable_uniform_buffer));
    let projection_uniform_buffer: UniformBufferPtr =
        context.create_uniform_buffer(bytemuck::cast_slice(&projection_ubo_vector), false, true);
    group.mutable_uniform_buffers().set(ID_PROJECTION_UBO, Some(projection_uniform_buffer));
}

/// Column-major matrix mapping tile x in [0, EXTENT] to clip x in [-1, 0]
/// (left half) and tile y in [0, EXTENT] to clip y in [1, -1].
fn left_half_matrix() -> [f32; 16] {
    let e = f32::from(EXTENT);
    [
        1.0 / e,
        0.0,
        0.0,
        0.0, //
        0.0,
        -2.0 / e,
        0.0,
        0.0, //
        0.0,
        0.0,
        1.0,
        0.0, //
        -1.0,
        1.0,
        0.0,
        1.0,
    ]
}

/// One frame as `renderer_impl.cpp` runs it; returns the readback.
fn render_frame(
    backend: &Headless,
    registry: &ShaderRegistry,
    passes: RenderPass,
    bg: &Background,
    clear: Color,
) -> mln_util::image::PremultipliedImage {
    let layer_group = build_layer_group(backend.get_context(), registry, passes);
    render_layer_group(backend, &layer_group, bg, left_half_matrix(), clear)
}

/// One frame of `layer_group`, tweaked with `matrix`; returns the readback.
fn render_layer_group(
    backend: &Headless,
    layer_group: &LayerGroupBasePtr,
    bg: &Background,
    matrix: [f32; 16],
    clear: Color,
) -> mln_util::image::PremultipliedImage {
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
        current_layer: 0,
        opaque_pass_cutoff: 0,
        clip_stencil: StencilMode::disabled(),
    };

    {
        let upload_pass = parameters.encoder.create_upload_pass("layerGroup-upload", renderable);
        tweak(context, layer_group, bg, matrix);
        layer_group.borrow_mut().upload(&*upload_pass);
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

    // drawableOpaquePass + drawableTranslucentPass
    for pass in [RenderPass::Opaque, RenderPass::Translucent] {
        parameters.pass = pass;
        parameters.current_layer = 0;
        layer_group.borrow_mut().render(&mut parameters);
    }

    context.unbind_global_uniform_buffers(parameters.render_pass.as_deref().unwrap());
    parameters.render_pass = None;
    backend.target.end_frame(&*parameters.encoder);
    context.end_frame();
    backend.read_still_image()
}

fn pixel(img: &mln_util::image::PremultipliedImage, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * img.size.width + x) * 4) as usize;
    img.data[i..i + 4].try_into().unwrap()
}

fn assert_near(actual: [u8; 4], expected: [u8; 4], what: &str) {
    for c in 0..4 {
        assert!(
            (i16::from(actual[c]) - i16::from(expected[c])).abs() <= 1,
            "{what}: got {actual:?}, expected {expected:?}"
        );
    }
}

fn check_halves(img: &mln_util::image::PremultipliedImage, left: [u8; 4], right: [u8; 4]) {
    let (w, h) = (img.size.width, img.size.height);
    for y in [0, h / 2, h - 1] {
        for x in [0, w / 4, w / 2 - 1] {
            assert_near(pixel(img, x, y), left, &format!("background at ({x}, {y})"));
        }
        for x in [w / 2, 3 * w / 4, w - 1] {
            assert_near(pixel(img, x, y), right, &format!("clear color at ({x}, {y})"));
        }
    }
}

#[test]
fn background_shader_opaque_and_translucent() {
    let backend = Headless::new(Size { width: 64, height: 32 });
    let info = backend.device().adapter_info();
    eprintln!("adapter: {} ({:?})", info.name, info.backend);
    let _scope = BackendScope::new(&*backend.backend, ScopeType::Explicit);

    let registry = ShaderRegistry::new();
    backend.init_shaders(&registry, &ProgramParameters::new(1.0, false));
    assert!(registry.get_shader_group("BackgroundShader").is_some());

    // Opaque: a solid color at opacity 1 renders in both the opaque and the
    // translucent pass list, unblended (render_background_layer.cpp).
    let green = Color::new(0.0, 0.5, 0.0, 1.0);
    let blue = Color::new(0.0, 0.0, 1.0, 1.0);
    let img = render_frame(
        &backend,
        &registry,
        RenderPass::Opaque | RenderPass::Translucent,
        &Background { color: green, opacity: 1.0 },
        blue,
    );
    assert_eq!(img.size, Size { width: 64, height: 32 });
    check_halves(&img, [0, 128, 0, 255], [0, 0, 255, 255]);

    // Translucent: opacity 0.5, alpha blended (One, OneMinusSrcAlpha) over
    // the clear color: (1,0,0,1) * 0.5 + (0,0,1,1) * (1 - 0.5).
    let red = Color::new(1.0, 0.0, 0.0, 1.0);
    let img =
        render_frame(&backend, &registry, RenderPass::Translucent, &Background { color: red, opacity: 0.5 }, blue);
    check_halves(&img, [128, 0, 128, 255], [0, 0, 255, 255]);

    // The shader program was created once and is reused from the group.
    let context = backend.get_context();
    let a = context.get_generic_shader(&registry, "BackgroundShader", ProjectionVariant::Mercator).unwrap();
    let b = context.get_generic_shader(&registry, "BackgroundShader", ProjectionVariant::Mercator).unwrap();
    assert!(std::rc::Rc::ptr_eq(&a, &b));
}

#[test]
fn a_drawable_binds_the_layer_buffers_of_each_frame() {
    // Each frame's tweak replaces the layer's drawable and projection UBOs,
    // so the second frame draws with the new matrix only if the drawable
    // binds the new buffers.
    let backend = Headless::new(Size { width: 32, height: 16 });
    let _scope = BackendScope::new(&*backend.backend, ScopeType::Explicit);
    let registry = ShaderRegistry::new();
    backend.init_shaders(&registry, &ProgramParameters::new(1.0, false));
    let layer_group = build_layer_group(backend.get_context(), &registry, RenderPass::Opaque | RenderPass::Translucent);
    let bg = Background { color: Color::new(0.0, 0.5, 0.0, 1.0), opacity: 1.0 };
    let blue = Color::new(0.0, 0.0, 1.0, 1.0);

    let img = render_layer_group(&backend, &layer_group, &bg, left_half_matrix(), blue);
    check_halves(&img, [0, 128, 0, 255], [0, 0, 255, 255]);
    let img = render_layer_group(&backend, &layer_group, &bg, full_matrix(), blue);
    check_halves(&img, [0, 128, 0, 255], [0, 128, 0, 255]);
}

#[test]
fn clear_only_frame_reads_back_clear_color() {
    // The path most M1 tests take: a main render pass with the background
    // color as clear color, no drawables, present, read back.
    let backend = Headless::new(Size { width: 16, height: 8 });
    let _scope = BackendScope::new(&*backend.backend, ScopeType::Explicit);
    let context = backend.get_context();
    let image = backend.target.begin_frame().expect("a frame");
    let renderable = &*image;
    context.begin_frame();
    let encoder = context.create_command_encoder();
    let pass = encoder.create_render_pass(
        "main buffer",
        &RenderPassDescriptor {
            target: renderable,
            clear_color: Some(Color::new(0.25, 0.5, 0.75, 1.0)),
            clear_depth: Some(1.0),
            clear_stencil: Some(0),
        },
    );
    drop(pass);
    backend.target.end_frame(&*encoder);
    context.end_frame();
    let img = backend.read_still_image();
    assert_eq!(img.data.len(), 16 * 8 * 4);
    for px in img.data.as_chunks::<4>().0 {
        assert_near(*px, [64, 128, 191, 255], "clear");
    }
}

/// Column-major matrix mapping the tile extent to the whole viewport.
fn full_matrix() -> [f32; 16] {
    let e = f32::from(EXTENT);
    [
        2.0 / e,
        0.0,
        0.0,
        0.0, //
        0.0,
        -2.0 / e,
        0.0,
        0.0, //
        0.0,
        0.0,
        1.0,
        0.0, //
        -1.0,
        1.0,
        0.0,
        1.0,
    ]
}

#[test]
fn tile_clipping_masks_limit_stenciled_drawables() {
    // webgpu::Context::renderTileClippingMasks writes stencil ref 1 for a
    // clip "tile" covering the left half; a full-viewport background
    // drawable with stencil enabled and PaintParameters::stencilModeForClipping
    // (Equal 1, as paint_parameters.cpp builds it) only lands there.
    let backend = Headless::new(Size { width: 32, height: 16 });
    let _scope = BackendScope::new(&*backend.backend, ScopeType::Explicit);
    let registry = ShaderRegistry::new();
    backend.init_shaders(&registry, &ProgramParameters::new(1.0, false));
    let context = backend.get_context();

    let layer_group = build_layer_group(context, &registry, RenderPass::Opaque | RenderPass::Translucent);
    visit_layer_group_drawables(&mut *layer_group.borrow_mut(), |d| d.set_enable_stencil(true));

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
        current_layer: 0,
        opaque_pass_cutoff: 0,
        clip_stencil: StencilMode {
            test: Test::Equal { mask: 0b1111_1111 },
            ref_: 1,
            mask: 0b0000_0000,
            fail: StencilOpType::Keep,
            depth_fail: StencilOpType::Keep,
            pass: StencilOpType::Replace,
        },
    };
    {
        let upload_pass = parameters.encoder.create_upload_pass("layerGroup-upload", renderable);
        tweak(
            context,
            &layer_group,
            &Background { color: Color::new(1.0, 1.0, 0.0, 1.0), opacity: 1.0 },
            full_matrix(),
        );
        layer_group.borrow_mut().upload(&*upload_pass);
    }
    parameters.render_pass = Some(parameters.encoder.create_render_pass(
        "main buffer",
        &RenderPassDescriptor {
            target: renderable,
            clear_color: Some(Color::new(0.0, 0.0, 0.0, 1.0)),
            clear_depth: Some(1.0),
            clear_stencil: Some(0),
        },
    ));
    let clip = ClipUBO { matrix: left_half_matrix(), stencil_ref: 1, ..Default::default() };
    assert!(context.render_tile_clipping_masks(parameters.render_pass.as_deref().unwrap(), &registry, &[clip]));
    for pass in [RenderPass::Opaque, RenderPass::Translucent] {
        parameters.pass = pass;
        layer_group.borrow_mut().render(&mut parameters);
    }
    parameters.render_pass = None;
    backend.target.end_frame(&*parameters.encoder);
    context.end_frame();

    let img = backend.read_still_image();
    check_halves(&img, [255, 255, 0, 255], [0, 0, 0, 255]);
}

// Port-only: a zero-size target has no frame to draw into, where C++ makes
// zero-size offscreen textures, a validation error.
#[test]
fn zero_size_target_has_no_frame() {
    let backend = Headless::new(Size { width: 0, height: 0 });
    assert!(backend.target.begin_frame().is_none());
}
