/// One of the frame's passes over the layers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pass {
    /// Opaque geometry, front to back, which the depth buffer orders.
    Opaque,
    /// The layers blended, bottom to top.
    Translucent,
}
