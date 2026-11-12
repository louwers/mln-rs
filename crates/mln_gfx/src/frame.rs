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

/// How one side of a shared texture waits for the other's GPU work on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SyncKind {
    /// The work has finished, so there is nothing to wait for.
    CpuComplete,
    /// A `GLsync` in the share group of both sides.
    OpenGlFence,
    /// The work went to the queue that both sides submit to, so work
    /// submitted to it later runs after it.
    QueueOrder,
    /// A timeline `VkSemaphore` and the value it reaches.
    VulkanTimelineSemaphore,
    /// A sync file descriptor, which signals once.
    SyncFd,
    /// An `MTLSharedEvent` and the value it reaches.
    MetalSharedEvent,
    /// A shared handle to an `ID3D12Fence` and the value it reaches.
    D3d12Fence,
}

impl SyncKind {
    /// Every kind, from the one that waits least to the one that waits most:
    /// queue order waits for nothing, a GPU object makes only the GPU wait,
    /// a sync file crosses APIs, and a finished frame makes the CPU wait.
    pub const BY_PREFERENCE: [SyncKind; 7] = [
        SyncKind::QueueOrder,
        SyncKind::OpenGlFence,
        SyncKind::VulkanTimelineSemaphore,
        SyncKind::MetalSharedEvent,
        SyncKind::D3d12Fence,
        SyncKind::SyncFd,
        SyncKind::CpuComplete,
    ];

    /// Whether a sync of this kind names an object to wait on.
    #[must_use]
    pub fn names_object(self) -> bool {
        !matches!(self, SyncKind::CpuComplete | SyncKind::QueueOrder)
    }

    fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// A set of [`SyncKind`]s.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SyncKinds(u8);

impl SyncKinds {
    pub const NONE: SyncKinds = SyncKinds(0);

    #[must_use]
    pub fn of(kinds: &[SyncKind]) -> SyncKinds {
        SyncKinds(kinds.iter().fold(0, |bits, kind| bits | kind.bit()))
    }

    #[must_use]
    pub fn contains(self, kind: SyncKind) -> bool {
        self.0 & kind.bit() != 0
    }

    #[must_use]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    #[must_use]
    pub fn with(self, kind: SyncKind) -> SyncKinds {
        SyncKinds(self.0 | kind.bit())
    }

    #[must_use]
    pub fn union(self, other: SyncKinds) -> SyncKinds {
        SyncKinds(self.0 | other.0)
    }

    pub fn iter(self) -> impl Iterator<Item = SyncKind> {
        SyncKind::BY_PREFERENCE.into_iter().filter(move |kind| self.contains(*kind))
    }

    /// The kind of `self`, the ones a host accepts, that `supported` has and
    /// that waits least. A host that names none accepts a finished frame
    /// where `supported` has one, and queue order otherwise.
    #[must_use]
    pub fn pick(self, supported: SyncKinds) -> Option<SyncKind> {
        let accepted = if !self.is_empty() {
            self
        } else if supported.contains(SyncKind::CpuComplete) {
            SyncKinds::of(&[SyncKind::CpuComplete])
        } else {
            SyncKinds::of(&[SyncKind::QueueOrder])
        };
        SyncKind::BY_PREFERENCE.into_iter().find(|kind| accepted.contains(*kind) && supported.contains(*kind))
    }
}

/// A point in one side's GPU work that the other side waits for before it
/// uses a shared texture: the object to wait on, as an integer, for the
/// kinds that name one, and the value a timeline reaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GpuSync {
    pub kind: SyncKind,
    pub handle: u64,
    pub value: u64,
}

impl GpuSync {
    /// A sync of a kind that names no object.
    #[must_use]
    pub fn of(kind: SyncKind) -> GpuSync {
        GpuSync { kind, handle: 0, value: 0 }
    }
}

/// What the host's reads of a frame's texture end with, which a target
/// waits for before it draws into the texture again. The target takes the
/// sync file, or the `GLsync` that `sync` names; dropping it closes the sync
/// file.
#[derive(Debug)]
pub struct ConsumerSync {
    /// The kind, and the timeline or the `GLsync` with its value. A sync
    /// file's descriptor is `sync_file`.
    pub sync: GpuSync,
    #[cfg(unix)]
    pub sync_file: Option<std::os::fd::OwnedFd>,
}

impl ConsumerSync {
    /// A sync of a kind that names no object, or names one that the host
    /// keeps.
    #[must_use]
    pub fn of(sync: GpuSync) -> ConsumerSync {
        ConsumerSync {
            sync,
            #[cfg(unix)]
            sync_file: None,
        }
    }
}

/// Where a frame's image went when its frame ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Handoff {
    /// A window or surface shows it.
    Presented,
    /// The target keeps it, for its owner to read back.
    ReadBack,
    /// The host's texture at `slot` of its ring holds it, once the host
    /// waits for `sync`. The object `sync` names stays the target's until
    /// the host gives the slot back.
    GivenBack { slot: usize, sync: GpuSync },
    /// The target's own image at `slot` of its exported ring holds it, for
    /// the host to acquire and read once it waits for `sync`. The image and
    /// the object `sync` names stay the target's until the host gives the
    /// slot back.
    Queued { slot: usize, sync: GpuSync },
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
