//! Links the browser build's module with a stack the size of the threads'
//! that other builds parse on (`mln_util::thread::STACK_SIZE`): JSON and the
//! expressions built from it recurse as deep as their input nests, and the
//! browser's instance parses on its one thread (`src/parse_thread.rs`).
//!
//! Also sets the cfgs of the native backends that the build draws with: a
//! backend's feature, on a platform that has the backend.
//!
//! - `backend_vulkan`: Vulkan, everywhere but Apple platforms and the browser.
//! - `backend_gl`: OpenGL through EGL on Linux, FreeBSD, Android and
//!   `OpenHarmony`, and through WGL on Windows.

fn main() {
    let feature = |name: &str| std::env::var_os(format!("CARGO_FEATURE_{}", name.to_uppercase())).is_some();
    let cfg = |name: &str| std::env::var(format!("CARGO_CFG_{}", name.to_uppercase())).unwrap_or_default();
    let family = cfg("target_family");
    let families: Vec<&str> = family.split(',').collect();
    let apple = cfg("target_vendor") == "apple";
    let wasm = families.contains(&"wasm");
    let windows = families.contains(&"windows");
    let unix = families.contains(&"unix");
    println!("cargo::rustc-check-cfg=cfg(backend_vulkan, backend_gl)");
    if feature("vulkan") && !apple && !wasm {
        println!("cargo::rustc-cfg=backend_vulkan");
    }
    if feature("gl") && (windows || (unix && !apple)) {
        println!("cargo::rustc-cfg=backend_gl");
    }
    if wasm {
        println!("cargo::rustc-link-arg-cdylib=-zstack-size=2097152");
    }
}
