//! maplibre-native-ffi's C ABI, as a C library (`libmaplibre_native_c`, shared
//! and static) for hosts that link maplibre-native-ffi's.
//!
//! The headers in `include/` are maplibre-native-ffi's. `src/generated` comes
//! from them (`mise run generate:c-abi`): their types, and a stub for each
//! function this crate doesn't define, which fails with
//! `MLN_STATUS_UNSUPPORTED`. To implement a function, define it outside
//! `generated/` as `#[unsafe(no_mangle)] pub unsafe extern "C" fn` with the
//! header's signature and regenerate; the generated code then checks that
//! signature. maplibre-native-ffi's ABI suite runs against the library
//! (`mise run test:c-abi`).
//!
//! Each header's functions live in the module of its name, except
//! `mln_frame_pacer_create()`, which lives in `frame_pacer` beside each
//! platform's source of display refreshes. The modules the headers share
//! hold the C-only rules: `export!` bodies, `handles`, struct versioning
//! (`abi`), `buffer`s, `callback` registrations, `delivery` queues and
//! `completion`s, the event batches that handlers receive (`events`), the
//! headers' geometry values (`geo`), and the
//! `parse_thread` that parses for callers that wait.
//!
//! In the browser, the library is a wasm-bindgen package per backend (`mise
//! run c-web:build webgpu` or `gl`): its instance's exports are the raw
//! functions and its memory, and `web` holds what a JavaScript host needs beside them, such
//! as the object handles of `web.h` and the callback trampolines that
//! tools/c-abi-gen generates. Everything runs on the thread that loaded the
//! module, and nothing waits.

#![expect(unsafe_code, reason = "every export takes raw pointers from C callers")]
// Whether a cast between C's types is trivial depends on the target, so
// no `expect` holds on every one.
#![allow(
    trivial_numeric_casts,
    reason = "a cast between C's types can be to the same type on one platform only: `c_char` is signed on some and unsigned on others, `c_ulong` 32 or 64 bits"
)]

#[macro_use]
mod export;

mod abi;
mod acquired_frame;
mod android;
mod base;
mod binding_support;
mod buffer;
mod callback;
mod camera;
mod completion;
mod custom_layer;
mod delivery;
mod diagnostic;
mod error;
mod events;
mod frame_pacer;
mod geo;
mod gpu_device;
mod handles;
mod logging;
mod map;
mod offline;
mod parse_thread;
mod process_exit;
mod projection;
mod query;
mod render_session;
mod render_target;
mod resources;
mod runtime;
mod style;
mod style_expression;
mod texture;
mod transforms;
mod url_rules;
mod web;
#[cfg(all(test, windows))]
mod wgl_tests;

#[rustfmt::skip]
mod generated {
    // An allow, not an expect: which lints the generated code trips changes
    // with the headers and the build's target.
    #![allow(
        dead_code,
        non_camel_case_types,
        non_snake_case,
        non_upper_case_globals,
        unreachable_pub,
        unused_variables,
        clippy::all,
        clippy::pedantic,
        reason = "tools/c-abi-gen declares every item of the C headers, with C's names"
    )]
    pub(crate) mod types;
    mod stubs;
    mod signatures;
    #[cfg(target_family = "wasm")]
    pub(crate) mod trampolines;
}
