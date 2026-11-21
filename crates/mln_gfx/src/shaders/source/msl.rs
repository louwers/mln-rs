//! MSL, which `mln_mtl` compiles.

/// The MSL of a program. Its library compiles from the backend's prelude,
/// `prelude` and `source`.
#[derive(Debug)]
pub struct Msl {
    /// The name of the vertex function.
    pub vertex_main_function: &'static str,
    /// The name of the fragment function.
    pub fragment_main_function: &'static str,
    /// The shader's own constants and structs, "" if none.
    pub prelude: &'static str,
    /// The functions.
    pub source: &'static str,
}
