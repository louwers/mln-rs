use ash::vk;
use mln_gfx::frame::FrameTarget as _;
use mln_gfx::{RendererBackend as _, SyncKind};
use mln_util::error::ErrorKind;
use mln_util::size::Size;

use crate::borrowed_target::Imports;
use crate::{BorrowedImage, BorrowedImages, BorrowedTarget, Device, ImageOwner, RendererBackend};

/// A ring of two images of `size`. The handles are null: nothing here draws
/// into them.
fn images(size: Size) -> BorrowedImages {
    let image = BorrowedImage { image: vk::Image::null(), view: vk::ImageView::null() };
    BorrowedImages {
        images: vec![image; 2],
        size,
        format: vk::Format::R8G8B8A8_UNORM,
        final_layout: vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
        owner: ImageOwner::GraphicsQueue,
        depth: None,
    }
}

fn backend() -> (Device, RendererBackend) {
    let device = Device::new(None).expect("Vulkan device");
    let backend = RendererBackend::new(&device).expect("a backend");
    (device, backend)
}

/// Sizes that no image of the device has.
fn bad_sizes(device: &Device) -> [Size; 3] {
    let max = device.shared.properties().limits.max_image_dimension2_d;
    [Size::new(0, 4), Size::new(4, 0), Size::new(max + 1, 4)]
}

// An error past the first import returns, with what was imported destroyed.
#[test]
fn imports_that_fail_are_destroyed() {
    let (device, mut backend) = backend();
    for size in bad_sizes(&device) {
        let error = Imports::new(backend.gpu(), &images(size)).err().expect("no depth texture of that size");
        assert_eq!(error.kind, ErrorKind::Native, "{size:?}");
    }
}

#[test]
fn a_ring_of_images_of_no_size_or_too_large_is_rejected() {
    let (device, mut backend) = backend();
    for size in bad_sizes(&device) {
        let error =
            BorrowedTarget::new(&mut backend, images(size), SyncKind::CpuComplete).err().expect("no ring of that size");
        assert_eq!(error.kind, ErrorKind::InvalidArgument, "{size:?}");
    }

    let mut target = BorrowedTarget::new(&mut backend, images(Size::new(4, 4)), SyncKind::CpuComplete)
        .expect("a ring of 4x4 images");
    let gpu = backend.gpu();
    for size in bad_sizes(&device) {
        let error = target.replace(gpu, images(size)).err().expect("no ring of that size");
        assert_eq!(error.kind, ErrorKind::InvalidArgument, "{size:?}");
    }
    target.replace(gpu, images(Size::new(8, 8))).expect("a ring of 8x8 images");
    target.tear_down(gpu);
}
