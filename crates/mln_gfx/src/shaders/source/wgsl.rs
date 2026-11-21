//! WGSL, which `mln_webgpu` compiles.

/// The WGSL of a program: its stages, and the constants and structs of its
/// own that both stages read.
///
/// A stage compiles from the backend's prelude, `prelude` and the stage's
/// source.
#[derive(Debug)]
pub struct Wgsl {
    /// The vertex stage, or `None` for an empty one.
    pub vertex: Option<&'static str>,
    /// The fragment stage, or `None` for an empty one.
    pub fragment: Option<&'static str>,
    /// What both stages read, or `None` for nothing.
    pub prelude: Option<&'static str>,
}
