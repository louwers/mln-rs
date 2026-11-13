#![cfg(not(target_arch = "wasm32"))]

// A drawable's vertex arrays against the index buffer they were bound with
// (src/mln/gl/drawable_gl.cpp `upload`), after test/gl/drawable.test.cpp.

use std::rc::Rc;

use mln_gfx::backend_scope::{BackendScope, ScopeType};
use mln_gfx::color_mode::ColorMode;
use mln_gfx::command_encoder::CommandEncoder;
use mln_gfx::context::Context as _;
use mln_gfx::depth_mode::DepthMode;
use mln_gfx::draw_mode::DrawMode;
use mln_gfx::drawable::Drawable as _;
use mln_gfx::frame::FrameTarget as _;
use mln_gfx::index_vector::{IndexVectorBase, IndexVectorBasePtr};
use mln_gfx::paint_parameters::PaintParameters;
use mln_gfx::render_pass::{RenderPass as GfxRenderPass, RenderPassDescriptor};
use mln_gfx::renderer::render_pass::RenderPass;
use mln_gfx::renderer_backend::RendererBackend as _;
use mln_gfx::scissor_rect::ScissorRect;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::ID_PROJECTION_UBO;
use mln_gfx::shaders::shader_program_base::ShaderProgramBasePtr;
use mln_gfx::stencil_mode::StencilMode;
use mln_gfx::types::DepthMaskType;
use mln_tile_id::UnwrappedTileID;
use mln_util::Color;
use mln_util::size::Size;

use crate::context::Context;
use crate::drawable::Drawable;
use crate::shaders::shader_program::ShaderProgram;
use crate::shaders::shader_source::{GlslShaderSource, ShaderSource, UniformBlockInfo};
use crate::upload_pass::UploadPass;
use crate::{Api, Device, DeviceOptions, OffscreenTarget, RendererBackend};

/// The API that `MLN_RENDER_BACKEND` asks for: desktop OpenGL for
/// `gl-desktop`, and otherwise the platform's.
fn api() -> Api {
    if knob().is_some_and(|backend| backend.eq_ignore_ascii_case("gl-desktop")) { Api::Gl } else { Api::default() }
}

fn knob() -> Option<String> {
    std::env::var("MLN_RENDER_BACKEND").ok()
}

/// A backend and an offscreen target on it, or none where there is no EGL
/// to make a context with, as on macOS: the test is skipped there, unless
/// `MLN_RENDER_BACKEND` asks for this backend (`gl` or `gl-desktop`).
fn headless(size: Size) -> Option<(Rc<RendererBackend>, OffscreenTarget)> {
    let made = Device::with_options(DeviceOptions { api: api(), ..DeviceOptions::default() })
        .and_then(|device| RendererBackend::new(&device));
    match made {
        Ok(backend) => {
            let backend = Rc::new(backend);
            let target = OffscreenTarget::new(Rc::clone(&backend), size);
            Some((backend, target))
        }
        Err(error)
            if knob().is_some_and(|backend| {
                backend.eq_ignore_ascii_case("gl") || backend.eq_ignore_ascii_case("gl-desktop")
            }) =>
        {
            panic!("{error}")
        }
        Err(error) => {
            eprintln!("skipped: {error}");
            None
        }
    }
}

/// What `Drawable::draw` reads of the renderer's paint parameters, for a 2D
/// draw without depth or stencil.
struct TestPaintParameters<'a> {
    context: &'a Context,
    scissor_rect: ScissorRect,
}

impl PaintParameters for TestPaintParameters<'_> {
    fn context(&self) -> &dyn mln_gfx::context::Context {
        self.context
    }
    fn encoder(&self) -> Option<&dyn CommandEncoder> {
        None
    }
    fn render_pass(&self) -> Option<&dyn GfxRenderPass> {
        None
    }
    fn pass(&self) -> RenderPass {
        RenderPass::Opaque
    }
    fn scissor_rect(&self) -> ScissorRect {
        self.scissor_rect
    }
    fn depth_mode_for_sublayer(&self, _n: u8, _mask: DepthMaskType) -> DepthMode {
        DepthMode::disabled()
    }
    fn depth_mode_for_3d(&self, _mask: DepthMaskType) -> DepthMode {
        DepthMode::disabled()
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

/// Draws vertex `gl_VertexID` of six fixed positions in green: indices 0-2
/// cover the target, and indices 3-5 miss it. The vertex prelude declares
/// the projection block, which some compilers report though no code reads
/// it.
static FIXED_TRIANGLES: ShaderSource = ShaderSource {
    glsl: &GlslShaderSource {
        vertex: r"const vec2 positions[6] = vec2[6](
    vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0),
    vec2(2.0, 2.0), vec2(3.0, 2.0), vec2(2.0, 3.0));
void main() { gl_Position = vec4(positions[gl_VertexID], 0.0, 1.0); }
",
        fragment: r"void main() { fragColor = vec4(0.0, 1.0, 0.0, 1.0); }
",
    },
    name: "FixedTriangles",
    uniform_blocks: &[UniformBlockInfo {
        name: "ProjectionUBO",
        id: ID_PROJECTION_UBO,
        size: size_of::<ProjectionUBO>(),
        element_size: 0,
    }],
    attributes: &[],
    instance_attributes: &[],
    textures: &[],
};

/// An index vector that changes gets a new buffer at the next upload of any
/// drawable that shares it. Each drawable then draws with the new buffer,
/// though it may have the GL name of a buffer it replaced.
#[test]
fn draws_with_replaced_index_buffer() {
    let size = Size { width: 16, height: 16 };
    let Some((backend, target)) = headless(size) else {
        return;
    };
    let _scope = BackendScope::new(&*backend, ScopeType::Explicit);
    let context = backend.get_context().downcast_ref::<Context>().expect("a GL context");

    let program =
        ShaderProgram::create(context, "FixedTriangles", &ProgramParameters::new(1.0, false), "", &FIXED_TRIANGLES, "")
            .expect("the program links");
    let shader: ShaderProgramBasePtr = Rc::new(program);

    let indexes: IndexVectorBasePtr = Rc::new(std::cell::RefCell::new(IndexVectorBase::new(vec![0, 1, 2])));
    let segments = [SegmentBase::with_lengths(0, 0, 6, 3)];
    let mut first = Drawable::new("first".to_owned());
    let mut second = Drawable::new("second".to_owned());
    for drawable in [&mut first, &mut second] {
        drawable.set_shader(Some(Rc::clone(&shader)));
        drawable.set_enable_depth(false);
        drawable.set_color_mode(&ColorMode::unblended());
        drawable.update_vertex_attributes(None, 6, DrawMode::Triangles(), Some(Rc::clone(&indexes)), &segments);
    }

    let image = target.begin_frame().expect("a frame");
    let renderable = &*image;
    context.begin_frame();
    let encoder = context.create_command_encoder();
    let upload = |drawable: &mut Drawable| {
        let pass = encoder.create_upload_pass("upload", renderable);
        drawable.upload(pass.downcast_ref::<UploadPass>().expect("a GL upload pass"));
    };
    upload(&mut first);
    upload(&mut second);

    // Replace the buffer twice through the other drawable. Once the first
    // buffer is deleted, the second replacement can take its GL name.
    for _ in 0..2 {
        for i in 0..3 {
            *indexes.borrow_mut().at_mut(i) = i as u16 + 3;
        }
        upload(&mut second);
        context.perform_cleanup();
    }

    upload(&mut first);
    {
        let _pass = encoder.create_render_pass(
            "draw",
            &RenderPassDescriptor {
                target: renderable,
                clear_color: Some(Color::new(0.0, 0.0, 0.0, 1.0)),
                clear_depth: Some(1.0),
                clear_stencil: Some(0),
            },
        );
        let mut parameters =
            TestPaintParameters { context, scissor_rect: ScissorRect { x: 0, y: 0, width: 16, height: 16 } };
        first.draw(&mut parameters);
    }
    target.end_frame(&*encoder);
    context.end_frame();

    let image = target.read_image().expect("a frame");
    let center = ((8 * size.width + 8) * 4) as usize;
    assert_eq!(&image.data[center..center + 4], &[0, 0, 0, 255], "drew the replaced indices");
}
