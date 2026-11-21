//! The backend-independent shader code: the engine's uniform blocks and
//! reserved binding ids, what a program binds and reads, and the code of its
//! shaders in each shading language.

pub mod bindings;
pub mod block;
pub mod layer_ubo;
pub mod program;
pub mod segment;
pub mod source;
