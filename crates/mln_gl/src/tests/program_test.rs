#![cfg(not(target_arch = "wasm32"))]
//! Compiles and links every program on an OpenGL ES 3.0 context and on a
//! desktop OpenGL one, through `GlGpu`, in the variants of none, all, each
//! alone and all but each of its flags, and checks that the driver lays out
//! each uniform block as `mln_shaders` does.

use mln_gfx::gpu::ShaderKey;
use mln_util::hash::HashSet;

use crate::{Api, Device, DeviceOptions, RendererBackend};

/// A backend on a device of `api`, or none where there is no EGL to make a
/// context of it with, as on macOS: the test is skipped there, unless
/// `MLN_RENDER_BACKEND` asks for this backend with this API (`gl` for
/// OpenGL ES, `gl-desktop` for desktop OpenGL).
fn headless_backend(api: Api) -> Option<RendererBackend> {
    let options = DeviceOptions { api, ..DeviceOptions::default() };
    let knob = match api {
        Api::Gles => "gl",
        Api::Gl => "gl-desktop",
    };
    match Device::with_options(options).and_then(|device| RendererBackend::new(&device)) {
        Ok(backend) => Some(backend),
        Err(error) if std::env::var("MLN_RENDER_BACKEND").is_ok_and(|backend| backend.eq_ignore_ascii_case(knob)) => {
            panic!("{error}")
        }
        Err(error) => {
            eprintln!("skipped: {error}");
            None
        }
    }
}

#[test]
fn every_program_links_on_open_gl_es() {
    if let Some(backend) = headless_backend(Api::Gles) {
        assert!(glow::HasContext::version(backend.context().gl()).is_embedded);
        every_program_links(&backend);
    }
}

/// The programs are GLSL 3.30 there.
#[test]
fn every_program_links_on_desktop_open_gl() {
    if let Some(backend) = headless_backend(Api::Gl) {
        assert!(!glow::HasContext::version(backend.context().gl()).is_embedded);
        every_program_links(&backend);
    }
}

fn every_program_links(backend: &RendererBackend) {
    let version = glow::HasContext::version(backend.context().gl());
    println!("GL_RENDERER: {}, GL_VERSION: {}", backend.get_renderer_name(), version.vendor_info);
    let target = mln_shaders::Target::Gl;
    for program in mln_shaders::programs() {
        for flags in mln_shaders::variants(&mln_shaders::flags_of(program), 0) {
            let key = ShaderKey::new(program, flags.into_iter().chain(["DEVICE_PIXEL_RATIO=1.0".to_owned()]));
            let what = format!("{key:?}");
            let linked =
                backend.gpu().program(&key).unwrap_or_else(|error| panic!("{what} failed to compile or link: {error}"));
            let wesl = mln_shaders::program_of(&key, target).expect("links");
            for block in &linked.blocks {
                let resource = wesl
                    .resources
                    .iter()
                    .find(|resource| resource.id as usize == block.id && resource.size > 0)
                    .unwrap_or_else(|| panic!("{what}: a block at {} that the program doesn't declare", block.id));
                assert_eq!(
                    block.size.next_multiple_of(16) as u64,
                    resource.size,
                    "{what}: the driver's size of {}",
                    resource.name
                );
            }
            let mut locations = HashSet::default();
            for &(_, location) in &linked.locations {
                assert!(locations.insert(location), "{what}: two attributes at location {location}");
            }
        }
    }
}
