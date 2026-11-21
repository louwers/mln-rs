//! The backend-independent shader code: the engine's uniform blocks and
//! reserved binding ids, what a program binds and reads, and the code of its
//! shaders in each shading language.

pub mod block;
pub mod layer_ubo;
pub mod program;
pub mod program_parameters;
pub mod segment;
pub mod shader_defines;
pub mod source;
