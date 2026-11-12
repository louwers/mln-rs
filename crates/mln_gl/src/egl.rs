// The EGL library, which the share groups (device.rs) and the
// window targets (surface_target.rs) make their contexts with. It is loaded
// at run time, once, so builds need no system development packages.

use std::sync::OnceLock;

use khronos_egl as egl;

pub(crate) type Egl = egl::DynamicInstance<egl::EGL1_4>;

struct Library(Result<Egl, String>);

// SAFETY: the library's functions may be called from any thread; EGL keeps
// what is current per thread.
unsafe impl Send for Library {}
// SAFETY: as above.
unsafe impl Sync for Library {}

/// The system's EGL, or why it couldn't be loaded.
pub(crate) fn library() -> Result<&'static Egl, String> {
    static LIBRARY: OnceLock<Library> = OnceLock::new();
    let library = LIBRARY.get_or_init(|| {
        // SAFETY: loading the system's EGL runs its initializers, which a
        // conforming library lets any process do.
        Library(unsafe { Egl::load_required() }.map_err(|e| format!("Failed to load EGL: {e}")))
    });
    library.0.as_ref().map_err(Clone::clone)
}
