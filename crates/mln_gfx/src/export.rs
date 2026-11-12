//! Rings of images that a target allocates and shares with the host: the
//! ways a texture's memory crosses to another API or process, how a ring
//! picks one of those that the host imports, and what each frame hands over.

/// A way to share a texture's memory with another API or process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExportKind {
    /// An `IOSurfaceRef`.
    IoSurface,
    /// A Linux dmabuf: file descriptors with a DRM format and modifier.
    DmaBuf,
    /// An `AHardwareBuffer*`.
    AndroidHardwareBuffer,
    /// A DXGI shared texture's NT handle.
    DxgiSharedHandle,
    /// The memory of a Vulkan image, as an opaque file descriptor that only
    /// APIs of the same driver import.
    OpaqueFd,
}

impl ExportKind {
    /// Every kind, in the order a ring prefers them: each platform's own
    /// interchange first, and last the opaque file descriptor, which crosses
    /// only between the APIs of one driver.
    pub const BY_PREFERENCE: [ExportKind; 5] = [
        ExportKind::IoSurface,
        ExportKind::AndroidHardwareBuffer,
        ExportKind::DxgiSharedHandle,
        ExportKind::DmaBuf,
        ExportKind::OpaqueFd,
    ];

    fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// A set of [`ExportKind`]s.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ExportKinds(u8);

impl ExportKinds {
    pub const NONE: ExportKinds = ExportKinds(0);

    #[must_use]
    pub fn of(kinds: &[ExportKind]) -> ExportKinds {
        ExportKinds(kinds.iter().fold(0, |bits, kind| bits | kind.bit()))
    }

    #[must_use]
    pub fn contains(self, kind: ExportKind) -> bool {
        self.0 & kind.bit() != 0
    }

    #[must_use]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    #[must_use]
    pub fn with(self, kind: ExportKind) -> ExportKinds {
        ExportKinds(self.0 | kind.bit())
    }

    #[must_use]
    pub fn union(self, other: ExportKinds) -> ExportKinds {
        ExportKinds(self.0 | other.0)
    }

    #[must_use]
    pub fn intersection(self, other: ExportKinds) -> ExportKinds {
        ExportKinds(self.0 & other.0)
    }

    /// The kinds, in [`ExportKind::BY_PREFERENCE`] order.
    pub fn iter(self) -> impl Iterator<Item = ExportKind> {
        ExportKind::BY_PREFERENCE.into_iter().filter(move |kind| self.contains(*kind))
    }
}

/// `DRM_FORMAT_ABGR8888`: red in the lowest byte, as Vulkan's
/// `R8G8B8A8_UNORM` lays it out.
pub const DRM_FORMAT_ABGR8888: u32 = u32::from_le_bytes(*b"AB24");
/// `DRM_FORMAT_ARGB8888`: blue in the lowest byte, as Vulkan's
/// `B8G8R8A8_UNORM` lays it out.
pub const DRM_FORMAT_ARGB8888: u32 = u32::from_le_bytes(*b"AR24");
/// `DRM_FORMAT_MOD_LINEAR`.
pub const DRM_FORMAT_MOD_LINEAR: u64 = 0;
/// The most planes that a dmabuf has.
pub const DMA_BUF_MAX_PLANES: usize = 4;

/// A DRM format and modifier that the host imports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DmaBufFormat {
    pub fourcc: u32,
    pub modifier: u64,
}

/// The fourcc that a ring renders, with the modifiers of it that both the
/// host and the device take, most preferred first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DmaBufChoice {
    pub fourcc: u32,
    pub modifiers: Vec<u64>,
}

/// The first of the host's fourccs, in its order, that the renderers draw
/// (ABGR8888 or ARGB8888) and for which `supported` takes one of the host's
/// modifiers, with each such modifier. `None` when no fourcc has one.
pub fn choose_dma_buf_format(
    formats: &[DmaBufFormat],
    supported: impl Fn(&DmaBufFormat) -> bool,
) -> Option<DmaBufChoice> {
    formats.iter().filter(|candidate| matches!(candidate.fourcc, DRM_FORMAT_ABGR8888 | DRM_FORMAT_ARGB8888)).find_map(
        |candidate| {
            let modifiers: Vec<u64> = formats
                .iter()
                .filter(|format| format.fourcc == candidate.fourcc && supported(format))
                .map(|format| format.modifier)
                .collect();
            (!modifiers.is_empty()).then_some(DmaBufChoice { fourcc: candidate.fourcc, modifiers })
        },
    )
}

/// A ring of images that the target allocates, as the host asks for it:
/// `depth` images, shared in a kind of `kinds`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportedRing {
    pub depth: usize,
    /// The kinds that the host imports.
    pub kinds: ExportKinds,
    /// The dmabuf formats and modifiers that the host imports, most
    /// preferred first.
    pub dma_buf_formats: Vec<DmaBufFormat>,
    /// More `AHARDWAREBUFFER_USAGE_*` bits that the host's reads need.
    pub android_hardware_buffer_usage: u64,
}

/// Where one plane of a dmabuf keeps its data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DmaBufPlane {
    /// The dmabuf's file descriptor, which the target owns.
    pub fd: i32,
    pub offset: u32,
    pub stride: u32,
}

/// The shared memory of a frame's image. The objects it names are the
/// target's, and stay valid until the host gives the frame back.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ExportedImage {
    /// An `IOSurfaceRef`, of the pixel format `'BGRA'`.
    IoSurface { surface: u64 },
    /// A dmabuf of `fourcc` with `modifier`, and its planes in order.
    DmaBuf { fourcc: u32, modifier: u64, planes: Vec<DmaBufPlane> },
    /// A Vulkan image's memory: `allocation_size` bytes, the image's own
    /// when `dedicated`, holding an image of the `VkFormat` `format` with
    /// optimal tiling.
    OpaqueFd { fd: i32, allocation_size: u64, dedicated: bool, format: u32 },
}

/// A target's generations of images: the ones it renders into now, and the
/// older ones that frames the host held at a resize keep alive, each with
/// the slots still held. A generation goes once the host gives back each of
/// its held slots.
pub struct Generations<G> {
    current: G,
    retired: Vec<(G, Vec<usize>)>,
}

impl<G> Generations<G> {
    pub fn new(current: G) -> Self {
        Generations { current, retired: Vec::new() }
    }

    pub fn current(&self) -> &G {
        &self.current
    }

    /// Renders into `next` from now on. The current generation stays while
    /// the host holds one of the `held` slots, and goes now otherwise.
    pub fn replace(&mut self, next: G, held: &[usize]) {
        let previous = std::mem::replace(&mut self.current, next);
        // A slot that the host held already in an older generation isn't
        // the previous one's.
        let held: Vec<usize> = held.iter().copied().filter(|slot| self.retired_holding(*slot).is_none()).collect();
        if !held.is_empty() {
            self.retired.push((previous, held));
        }
    }

    fn retired_holding(&self, slot: usize) -> Option<usize> {
        self.retired.iter().position(|(_, held)| held.contains(&slot))
    }

    /// The image of an older generation that the host holds at `slot`.
    pub fn retired(&self, slot: usize) -> Option<&G> {
        self.retired_holding(slot).map(|index| &self.retired[index].0)
    }

    /// The host gave `slot` back: true when an older generation held it,
    /// which goes once it holds no slot. The current generation's slot is
    /// the caller's to give back then.
    pub fn give_back(&mut self, slot: usize) -> bool {
        let Some(index) = self.retired_holding(slot) else { return false };
        let held = &mut self.retired[index].1;
        held.retain(|held| *held != slot);
        if held.is_empty() {
            drop(self.retired.remove(index));
        }
        true
    }

    /// Lets go of every older generation.
    pub fn clear_retired(&mut self) {
        self.retired.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_iterate_by_preference() {
        let kinds = ExportKinds::of(&[ExportKind::OpaqueFd, ExportKind::DmaBuf, ExportKind::IoSurface]);
        assert_eq!(kinds.iter().collect::<Vec<_>>(), [ExportKind::IoSurface, ExportKind::DmaBuf, ExportKind::OpaqueFd]);
        assert_eq!(
            kinds.intersection(ExportKinds::of(&[ExportKind::OpaqueFd])).iter().next(),
            Some(ExportKind::OpaqueFd)
        );
        assert!(ExportKinds::NONE.iter().next().is_none());
    }

    #[test]
    fn a_dma_buf_format_follows_the_hosts_order() {
        let format = |fourcc, modifier| DmaBufFormat { fourcc, modifier };
        // NV12, which no renderer draws, comes first.
        let nv12 = u32::from_le_bytes(*b"NV12");
        let formats = [
            format(nv12, 0),
            format(DRM_FORMAT_ARGB8888, 7),
            format(DRM_FORMAT_ABGR8888, 0),
            format(DRM_FORMAT_ARGB8888, 0),
        ];
        assert_eq!(
            choose_dma_buf_format(&formats, |_| true),
            Some(DmaBufChoice { fourcc: DRM_FORMAT_ARGB8888, modifiers: vec![7, 0] })
        );
        // A fourcc whose modifiers the device lacks gives way to the next.
        assert_eq!(
            choose_dma_buf_format(&formats, |format| format.fourcc == DRM_FORMAT_ABGR8888),
            Some(DmaBufChoice { fourcc: DRM_FORMAT_ABGR8888, modifiers: vec![0] })
        );
        assert_eq!(choose_dma_buf_format(&formats, |_| false), None);
        assert_eq!(choose_dma_buf_format(&[format(nv12, 0)], |_| true), None);
    }

    #[test]
    fn a_generation_stays_until_its_held_slots_come_back() {
        let mut generations = Generations::new("first");
        generations.replace("second", &[0, 2]);
        assert_eq!(*generations.current(), "second");
        assert_eq!(generations.retired(0), Some(&"first"));
        assert_eq!(generations.retired(1), None);
        // Slot 0 is still held at the next resize, in the first generation.
        generations.replace("third", &[0, 1]);
        assert_eq!(generations.retired(0), Some(&"first"));
        assert_eq!(generations.retired(1), Some(&"second"));
        assert!(generations.give_back(0));
        assert_eq!(generations.retired(2), Some(&"first"));
        assert!(generations.give_back(2));
        assert_eq!(generations.retired(2), None);
        assert!(!generations.give_back(2));
        assert!(generations.give_back(1));
        // Nothing held: the previous generation goes at once.
        generations.replace("fourth", &[]);
        assert_eq!(generations.retired(0), None);
    }
}
