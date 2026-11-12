//! How the renderer draws a frame into its target: it begins the frame, draws
//! into the image the target gives it, and ends the frame, which hands the
//! image off.

use std::any::Any;
use std::rc::Rc;

use mln_util::size::Size;

use crate::command_encoder::CommandEncoder;

/// What a render pass draws into: a frame's image, or a texture that the
/// renderer draws into ahead of the frame. Each backend downcasts it to its
/// own type.
pub trait DrawTarget: Any {
    /// The size in physical pixels.
    fn size(&self) -> Size;
    fn as_any(&self) -> &dyn Any;
}

/// Where a frame's image went when its frame ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Handoff {
    /// A window or surface shows it.
    Presented,
    /// The target keeps it, for its owner to read back.
    ReadBack,
    /// Nothing took it: the target had no image to hand off.
    Dropped,
}

/// A target that a renderer draws frames into: a window's swapchain, or an
/// image of the library's that is read back. It belongs to the thread that
/// built it.
pub trait FrameTarget {
    /// Begins a frame, and returns what the renderer draws it into, or `None`
    /// when the target has nothing to draw into now: it has no surface, or
    /// the surface is hidden or out of date. The caller then skips the frame.
    /// Until [`end_frame`](Self::end_frame), each call returns the same
    /// image.
    fn begin_frame(&self) -> Option<Rc<dyn DrawTarget>>;

    /// Submits what the renderer recorded into `encoder` for the frame, and
    /// hands the frame's image off.
    fn end_frame(&self, encoder: &dyn CommandEncoder) -> Handoff;
}
