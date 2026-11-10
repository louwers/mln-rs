// Ports test/util/offscreen_texture.test.cpp.
//
// The C++ file is `#if MLN_RENDER_BACKEND_OPENGL`. EmptyRed and
// RenderToTexture drive OpenGL directly (glClear, GL shader programs and
// framebuffers) and stay OpenGL-only; ClearRenderPassColor only uses the gfx
// abstractions, so it runs here on the WebGPU headless backend, compared
// with the same fixture.

use mln_gfx::backend_scope::{BackendScope, ScopeType};
use mln_gfx::render_pass::RenderPassDescriptor;
use mln_gfx::renderer_backend::RendererBackend as _;
use mln_gfx::types::TextureChannelDataType;
use mln_image::decode_image;
use mln_util::Color;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use mln_webgpu::HeadlessBackend;

fn read_fixture(path: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join(path);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// test/src/mln/test/util.cpp `getImageDiff` (without writing actual.png and
/// diff.png into the read-only fixtures).
fn get_image_diff(base: &str, actual: &PremultipliedImage, pixel_threshold: f64) -> f64 {
    let expected = decode_image(&read_fixture(&format!("{base}/expected.png"))).expect("expected.png");
    if expected.size != actual.size {
        return 1.0;
    }
    let pixels = pixelmatch::pixelmatch(
        &actual.data,
        &expected.data,
        expected.size.width as usize,
        expected.size.height as usize,
        None,
        pixel_threshold,
        false,
    );
    pixels as f64 / (expected.size.width as f64 * expected.size.height as f64)
}

/// `test::checkImage(base, actual, imageThreshold, pixelThreshold)`.
#[track_caller]
fn check_image(base: &str, actual: &PremultipliedImage, image_threshold: f64, pixel_threshold: f64) {
    let diff = get_image_diff(base, actual, pixel_threshold);
    assert!(diff <= image_threshold, "{base}: diff {diff} > {image_threshold}");
}

// gtest: OffscreenTexture.EmptyRed
#[test]
#[ignore = "OpenGL-only: clears the default framebuffer with glClear and checks the scissor state of gl::HeadlessBackend::bind()"]
fn empty_red() {}

// gtest: OffscreenTexture.RenderToTexture
#[test]
#[ignore = "OpenGL-only: hand-written GL shader programs, buffers and framebuffers"]
fn render_to_texture() {}

// gtest: OffscreenTexture.ClearRenderPassColor
#[test]
fn clear_render_pass_color() {
    let backend = HeadlessBackend::create(Size::new(128, 256));
    let _scope = BackendScope::new(&backend, ScopeType::Explicit);
    let context = backend.get_context();
    let encoder = context.create_command_encoder();
    let offscreen_texture = context.create_offscreen_texture(Size::new(128, 256), TextureChannelDataType::UnsignedByte);
    let render_pass = encoder.create_render_pass(
        "offscreen texture",
        &RenderPassDescriptor {
            renderable: &*offscreen_texture,
            clear_color: Some(Color::new(1.0, 0.0, 0.0, 1.0)),
            clear_depth: None,
            clear_stencil: None,
        },
    );
    drop(render_pass);
    // The OpenGL encoder runs the clear immediately; a WebGPU command
    // encoder records it, and releasing it (C++ `encoder.reset()`) submits
    // nothing. `present` submits the command buffer, as the renderer does at
    // the end of a frame.
    encoder.present(&*offscreen_texture);
    drop(encoder);

    let image = offscreen_texture.read_still_image();
    check_image("test/fixtures/offscreen_texture/clear-render-pass-color", &image, 0.0, 0.0);
}
