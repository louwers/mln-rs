//! maplibre-native-ffi's C ABI, as a C library (`libmaplibre_native_c`, shared
//! and static) for hosts that link maplibre-native-ffi's.
//!
//! The headers in `include/` are maplibre-native-ffi's. `src/generated` comes
//! from them (`just generate-c-abi`): their types, and a stub for each function
//! this crate doesn't define, which fails with `MLN_STATUS_UNSUPPORTED`. To
//! implement a function, define it outside `generated/` as
//! `#[unsafe(no_mangle)] pub unsafe extern "C" fn` with the header's signature
//! and regenerate; the generated code then checks that signature.
//! maplibre-native-ffi's ABI suite runs against the library (`just c-abi-test`).

// Every export takes raw pointers from C callers.
#![allow(unsafe_code)]

mod diagnostic;
mod plugin;

#[rustfmt::skip]
#[allow(dead_code, non_camel_case_types, non_snake_case, non_upper_case_globals, unused_variables, clippy::all)]
mod generated {
    pub mod types;
    mod stubs;
    mod signatures;
}

use generated::types::MLN_C_VERSION;

/// `mln_c_version()`: the API epoch of the headers this library implements.
#[unsafe(no_mangle)]
pub extern "C" fn mln_c_version() -> u32 {
    MLN_C_VERSION
}
