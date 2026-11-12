//! `base.h`: what the library reports about itself.

use crate::generated::types::{MLN_C_VERSION, mln_render_backend_flags};

/// `mln_c_version()`: the API epoch of the headers this library implements.
#[unsafe(no_mangle)]
pub extern "C" fn mln_c_version() -> u32 {
    MLN_C_VERSION
}

/// `mln_supported_render_backend_mask()`: the backends that
/// `mln_gpu_device_create()` makes devices of on this platform.
#[unsafe(no_mangle)]
pub extern "C" fn mln_supported_render_backend_mask() -> mln_render_backend_flags {
    crate::gpu_device::supported_backend_mask()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_library_reports_its_epoch() {
        assert_eq!(mln_c_version(), MLN_C_VERSION);
    }
}
