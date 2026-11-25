//! The backend-independent shader code: what a program reads and the text
//! its shaders compile from, uniform blocks, and the binding ids and
//! defines. The renderer declares the programs.

pub mod bindings;
pub mod block;
pub mod layer_ubo;
pub mod program;
pub mod program_parameters;
pub mod segment;
pub mod source;
