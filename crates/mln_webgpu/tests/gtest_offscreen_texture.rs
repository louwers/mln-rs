// ClearRenderPassColor: a pass that clears a texture, read back and compared
// with its fixture.

use mln_gfx::gpu::{ColorAttachment, Gpu as _, LoadOp, Pass, TextureDesc, TextureFormat, TextureRegion, TextureUsage};
use mln_image::decode_image;
use mln_util::Color;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use mln_webgpu::{Device, WebGpu};

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
    pixels as f64 / (f64::from(expected.size.width) * f64::from(expected.size.height))
}

#[track_caller]
fn check_image(base: &str, actual: &PremultipliedImage, image_threshold: f64, pixel_threshold: f64) {
    let diff = get_image_diff(base, actual, pixel_threshold);
    assert!(diff <= image_threshold, "{base}: diff {diff} > {image_threshold}");
}

// gtest: OffscreenTexture.ClearRenderPassColor
#[test]
fn clear_render_pass_color() {
    let device = Device::new(None).expect("device");
    let mut gpu = WebGpu::new(&device);
    let size = Size::new(128, 256);
    let texture = gpu
        .create_texture(&TextureDesc {
            label: "offscreen texture",
            size,
            format: TextureFormat::Rgba8Unorm,
            usage: TextureUsage { render_target: true, copy_src: true, ..TextureUsage::default() },
        })
        .expect("a texture");
    let red = Color::new(1.0, 0.0, 0.0, 1.0);
    let pass = Pass {
        label: "offscreen texture".to_owned(),
        color: Some(ColorAttachment { texture, load: LoadOp::Clear(red) }),
        depth_stencil: None,
        commands: Vec::new(),
    };
    gpu.submit(&[pass]).expect("the pass");
    let texels = gpu.read_texels(texture, TextureRegion::whole(size)).expect("the texels");
    let image = PremultipliedImage::from_vec(size, texels);
    check_image("test/fixtures/offscreen_texture/clear-render-pass-color", &image, 0.0, 0.0);
}
