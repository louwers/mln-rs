//! The engine's programs, with their uniform blocks and their sources in
//! each shading language the build has: WGSL with the `wgsl` feature, MSL
//! with `msl` on Apple platforms, GLSL ES with `glsl-es`, and the GLSL of
//! `mln_glsl` with `glsl` outside Apple platforms and the browser.

/// The [`Sources`](mln_gfx::shaders::program::Sources) of a program: of
/// each language the build has, the const named for it in the language's
/// module beside the caller's, each followed by a comma.
macro_rules! sources {
    ($(wgsl: $wgsl:ident,)? $(msl: $msl:ident,)? $(glsl_es: $glsl_es:ident,)? $(glsl: $glsl:ident,)?) => {
        mln_gfx::shaders::program::Sources {
            $(#[cfg(feature = "wgsl")] wgsl: Some(&super::wgsl::$wgsl),)?
            $(#[cfg(all(feature = "msl", target_vendor = "apple"))] msl: Some(&super::msl::$msl),)?
            $(#[cfg(feature = "glsl-es")] glsl_es: Some(&super::glsl_es::$glsl_es),)?
            $(#[cfg(all(feature = "glsl", not(any(target_vendor = "apple", target_family = "wasm"))))]
            glsl: Some(&super::glsl::$glsl),)?
            ..mln_gfx::shaders::program::Sources::NONE
        }
    };
}

#[cfg(all(feature = "glsl", not(any(target_vendor = "apple", target_family = "wasm"))))]
mod glsl;
#[cfg(feature = "glsl-es")]
mod glsl_es;
#[cfg(all(feature = "msl", target_vendor = "apple"))]
mod msl;
pub(crate) mod programs;
pub(crate) mod ubo;
#[cfg(feature = "wgsl")]
mod wgsl;
