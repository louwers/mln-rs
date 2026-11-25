//! The Metal shaders: the engine's MSL, and the declarations of what each
//! program reads, which a program's MSL takes from the program.

mod declarations;
/// The engine's MSL.
#[rustfmt::skip]
pub(crate) mod prelude;
pub(crate) mod shader_source;

#[cfg(test)]
mod msl_compile_test;
