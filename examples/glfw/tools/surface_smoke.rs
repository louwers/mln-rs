// Window + surface smoke test for //examples/glfw (no C++ counterpart): opens
// a GLFW window, creates the wgpu surface from its raw handles through
// `GLFWWebGPUBackend`, clears it to a colour through the gfx path the
// renderer uses (command encoder, render pass on the default renderable,
// present), reads the last presented frame back, checks its pixels and
// closes the window.
//
// Usage: glfw_surface_smoke [seconds]   (default 3)
// Exit status 0 when the read-back frame has the clear colour.

use std::time::{Duration, Instant};

use mln_gfx::backend_scope::{BackendScope, ScopeType};
use mln_gfx::render_pass::RenderPassDescriptor;
use mln_glfw::glfw_backend::{self, GLFWBackend};
use mln_glfw::glfw_webgpu_backend::GLFWWebGPUBackend;
use mln_util::color::Color;

fn main() {
    let seconds: f64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(3.0);

    let mut glfw = match glfw_backend::init_glfw() {
        Ok(glfw) => glfw,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    glfw.window_hint(glfw::WindowHint::ClientApi(glfw::ClientApiHint::NoApi));
    let Some((mut window, events)) =
        glfw.create_window(640, 480, "mln-glfw surface smoke test", glfw::WindowMode::Windowed)
    else {
        eprintln!("failed to initialize window");
        std::process::exit(2);
    };
    window.set_all_polling(true);

    let backend = match GLFWWebGPUBackend::new(&window, true) {
        Ok(backend) => backend,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let surface = backend.surface_backend().clone();
    let info = surface.get_adapter_info();
    println!(
        "platform={:?} adapter=\"{}\" backend={:?} driver=\"{}\" ({}) format={:?} framebuffer={:?}",
        glfw.get_platform(),
        info.name,
        info.backend,
        info.driver,
        info.driver_info,
        surface.get_swap_chain_format(),
        backend.get_framebuffer_size(),
    );

    let clear = Color::new(0.2, 0.4, 0.8, 1.0);
    let started = Instant::now();
    let mut frames = 0u32;
    let renderer_backend = backend.get_renderer_backend();
    loop {
        glfw.poll_events();
        for (_, event) in glfw::flush_messages(&events) {
            if let glfw::WindowEvent::FramebufferSize(w, h) = event {
                backend.set_framebuffer_size(mln_util::size::Size::new(w.max(0) as u32, h.max(0) as u32));
            }
        }
        let last = started.elapsed().as_secs_f64() >= seconds || window.should_close();
        if last {
            surface.request_readback();
        }

        {
            let _scope = BackendScope::new(&*renderer_backend, ScopeType::Explicit);
            let context = renderer_backend.get_context();
            let renderable = renderer_backend.get_default_renderable();
            context.begin_frame();
            let encoder = context.create_command_encoder();
            let pass = encoder.create_render_pass(
                "main buffer",
                &RenderPassDescriptor {
                    renderable,
                    clear_color: Some(clear),
                    clear_depth: Some(1.0),
                    clear_stencil: Some(0),
                },
            );
            drop(pass);
            encoder.present(renderable);
            context.end_frame();
        }
        frames += 1;
        if last {
            break;
        }
        std::thread::sleep(Duration::from_millis(16));
    }

    let image = surface.take_readback();
    drop(renderer_backend);
    drop(backend);
    window.set_should_close(true);
    drop(window);

    let Some(image) = image else {
        eprintln!("FAIL: no frame was read back after {frames} frames");
        std::process::exit(1);
    };
    let expected = [51u8, 102, 204, 255];
    let matching =
        image.data.chunks_exact(4).filter(|px| px.iter().zip(expected).all(|(&a, b)| a.abs_diff(b) <= 1)).count();
    let total = image.data.len() / 4;
    println!(
        "read back {}x{} after {frames} frames: {matching}/{total} pixels are rgba{expected:?}",
        image.size.width, image.size.height
    );
    if total == 0 || matching != total {
        eprintln!("FAIL");
        std::process::exit(1);
    }
    println!("PASS");
}
