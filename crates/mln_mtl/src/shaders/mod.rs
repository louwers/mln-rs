//! The Metal shaders: their MSL, and the declarations of what each program
//! reads, which the MSL takes from the program.

mod declarations;
#[rustfmt::skip]
pub(crate) mod msl_sources;
pub(crate) mod shader_source;

#[cfg(test)]
mod msl_compile_test;
