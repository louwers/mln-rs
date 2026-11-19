use ash::vk;
use mln_gfx::frame::FrameTarget as _;
use mln_gfx::ring::Imports;
use mln_gfx::{RendererBackend as _, SyncKind};
use mln_util::error::ErrorKind;
use mln_util::size::Size;

use crate::{BorrowedImage, BorrowedImages, BorrowedTarget, Device, ImageOwner, RendererBackend, VulkanRing};

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
        let error =
            Imports::new::<VulkanRing>(backend.gpu(), &images(size)).err().expect("no depth texture of that size");
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
    // A replacement doesn't block: the old images are the host's again once
    // the reads that it returns end.
    let in_flight = target.replace(gpu, images(Size::new(8, 8))).expect("a ring of 8x8 images");
    wait_for(&in_flight, gpu);
    target.tear_down(gpu);
}

/// Images of the device's own that a ring draws into, which the test
/// destroys once the device is idle.
struct Owned {
    device: Device,
    images: Vec<(vk::Image, vk::ImageView, vk::DeviceMemory)>,
    timelines: Vec<vk::Semaphore>,
}

impl Owned {
    fn new(device: &Device) -> Owned {
        Owned { device: device.clone(), images: Vec::new(), timelines: Vec::new() }
    }

    /// A ring of `count` RGBA8 images of `size`, which frames draw into.
    fn ring(&mut self, count: usize, size: Size) -> BorrowedImages {
        let images = (0..count).map(|_| self.image(size)).collect();
        BorrowedImages {
            images,
            size,
            format: vk::Format::R8G8B8A8_UNORM,
            final_layout: vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            owner: ImageOwner::GraphicsQueue,
            depth: None,
        }
    }

    fn image(&mut self, size: Size) -> BorrowedImage {
        let shared = &self.device.shared;
        let device = shared.device();
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk::Format::R8G8B8A8_UNORM)
            .extent(vk::Extent3D { width: size.width, height: size.height, depth: 1 })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: valid create infos, and memory of the image's requirements.
        unsafe {
            let image = device.create_image(&info, None).unwrap();
            let requirements = device.get_image_memory_requirements(image);
            let properties = shared.instance().get_physical_device_memory_properties(shared.physical_device());
            let index = (0..properties.memory_type_count)
                .find(|&index| requirements.memory_type_bits & (1 << index) != 0)
                .unwrap();
            let allocate =
                vk::MemoryAllocateInfo::default().allocation_size(requirements.size).memory_type_index(index);
            let memory = device.allocate_memory(&allocate, None).unwrap();
            device.bind_image_memory(image, memory, 0).unwrap();
            let view_info = vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(vk::Format::R8G8B8A8_UNORM)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                });
            let view = device.create_image_view(&view_info, None).unwrap();
            self.images.push((image, view, memory));
            BorrowedImage { image, view }
        }
    }

    /// A timeline semaphore at zero.
    fn timeline(&mut self) -> vk::Semaphore {
        let mut type_info =
            vk::SemaphoreTypeCreateInfo::default().semaphore_type(vk::SemaphoreType::TIMELINE).initial_value(0);
        let info = vk::SemaphoreCreateInfo::default().push_next(&mut type_info);
        // SAFETY: a valid create info, on a device with timelines.
        let semaphore = unsafe { self.device.shared.device().create_semaphore(&info, None) }.unwrap();
        self.timelines.push(semaphore);
        semaphore
    }

    /// Queues work of the host's on the device's queue that waits for
    /// `semaphore`, a timeline, to reach `value`.
    fn queue_host_work(&self, semaphore: vk::Semaphore, value: u64) {
        let (semaphores, values, stages) = ([semaphore], [value], [vk::PipelineStageFlags::ALL_COMMANDS]);
        let mut timeline = vk::TimelineSemaphoreSubmitInfo::default().wait_semaphore_values(&values);
        let submit = vk::SubmitInfo::default()
            .wait_semaphores(&semaphores)
            .wait_dst_stage_mask(&stages)
            .push_next(&mut timeline);
        let device = self.device.shared.device();
        // SAFETY: a submission of no commands that waits for a timeline of
        // the device, on its queue, held for the call.
        self.device.with_queue(|queue| unsafe { device.queue_submit(queue, &[submit], vk::Fence::null()) }).unwrap();
    }

    /// Signals `semaphore`, a timeline, to `value` now, from the host's CPU.
    fn signal(&self, semaphore: vk::Semaphore, value: u64) {
        let info = vk::SemaphoreSignalInfo::default().semaphore(semaphore).value(value);
        // SAFETY: a timeline of the device, below `value`.
        unsafe { self.device.shared.device().signal_semaphore(&info) }.unwrap();
    }
}

/// The host's timeline at `value`.
fn timeline_sync(semaphore: vk::Semaphore, value: u64) -> mln_gfx::ConsumerSync {
    use ash::vk::Handle as _;
    mln_gfx::ConsumerSync::of(mln_gfx::GpuSync {
        kind: SyncKind::VulkanTimelineSemaphore,
        handle: semaphore.as_raw(),
        value,
    })
}

impl Drop for Owned {
    fn drop(&mut self) {
        self.device.wait_idle("the test's images");
        let device = self.device.shared.device();
        // SAFETY: the device is idle, and nothing uses the objects any more.
        unsafe {
            for semaphore in self.timelines.drain(..) {
                device.destroy_semaphore(semaphore, None);
            }
            for (image, view, memory) in self.images.drain(..) {
                device.destroy_image_view(view, None);
                device.destroy_image(image, None);
                device.free_memory(memory, None);
            }
        }
    }
}

/// Begins a frame of `target` and clears its image in a pass, without
/// ending the frame.
fn draw_without_ending(target: &mut BorrowedTarget, gpu: &mut crate::VulkanGpu) {
    use mln_gfx::gpu::{ColorAttachment, DepthStencilAttachment, Gpu as _, LoadOp, PassDesc};
    let image = target.begin_frame(gpu).expect("a frame");
    let desc = PassDesc {
        label: "Unended",
        color: Some(ColorAttachment { texture: image.color, load: LoadOp::Clear(mln_util::Color::red()) }),
        depth_stencil: image.depth_stencil.map(|texture| DepthStencilAttachment {
            texture,
            depth: LoadOp::Clear(1.0),
            stencil: LoadOp::Clear(0),
        }),
    };
    gpu.begin_pass(&desc).unwrap();
    gpu.end_pass().unwrap();
}

/// A sync file that signals once a frame of `producer` is done.
#[cfg(unix)]
fn sync_file(producer: &mut BorrowedTarget, gpu: &mut crate::VulkanGpu) -> mln_gfx::ConsumerSync {
    use std::os::fd::BorrowedFd;
    draw_without_ending(producer, gpu);
    let mln_gfx::frame::Handoff::GivenBack { sync, .. } = producer.end_frame(gpu) else { panic!("a handed frame") };
    assert_eq!(SyncKind::SyncFd, sync.kind);
    // SAFETY: the producer keeps the file open until its slot is given back.
    let file = unsafe { BorrowedFd::borrow_raw(sync.handle as i32) }.try_clone_to_owned().unwrap();
    mln_gfx::ConsumerSync { sync, sync_file: Some(file) }
}

/// Waits until the work `in_flight` finishes on `gpu`.
fn wait_for(in_flight: &crate::InFlight, gpu: &mut crate::VulkanGpu) {
    assert!(finishes(in_flight, gpu), "the work never finished");
}

/// Whether `in_flight` finishes within ten seconds.
fn finishes(in_flight: &crate::InFlight, gpu: &mut crate::VulkanGpu) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !in_flight.finished(gpu).expect("the work failed") {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::yield_now();
    }
    true
}

// A frame begun and drawn into, but not ended, is submitted with what its
// slot waits for before the ring lets go of its images, and the semaphores
// imported for the waits outlive that submit: on replacement and on
// tear-down, for the host's sync files and timelines. Neither blocks: a host
// that signals its timeline only once the call returned gets the images
// back once the work that the call returned finishes. The host's reads of a
// slot that no frame drew into hold nothing up.
#[test]
fn a_frame_drawn_and_not_ended_is_submitted_with_its_waits() {
    let (device, mut backend) = backend();
    let mut owned = Owned::new(&device);
    let size = Size::new(8, 8);
    let (producers, consumers) = device.sync_kinds();
    let timelines = consumers.contains(SyncKind::VulkanTimelineSemaphore);
    let sync_files = cfg!(unix) && producers.contains(SyncKind::SyncFd) && consumers.contains(SyncKind::SyncFd);
    assert!(timelines || sync_files, "the device waits for neither timelines nor sync files");
    #[cfg(unix)]
    let mut producer = sync_files.then(|| {
        BorrowedTarget::new(&mut backend, owned.ring(1, size), SyncKind::SyncFd).expect("a sync file producer")
    });
    let mut target =
        BorrowedTarget::new(&mut backend, owned.ring(2, size), SyncKind::CpuComplete).expect("a ring of 8x8 images");
    let gpu = backend.gpu();
    let timeline = timelines.then(|| owned.timeline());

    // Replacement with a frame drawn at slot 0 that waits for a sync file,
    // where the device takes them.
    #[cfg(unix)]
    if let Some(producer) = &mut producer {
        let consumer = sync_file(producer, gpu);
        producer.release(gpu, 0, mln_gfx::ConsumerSync::of(mln_gfx::GpuSync::of(SyncKind::CpuComplete))).unwrap();
        target.release(gpu, 0, consumer).unwrap();
        target.select(0);
        draw_without_ending(&mut target, gpu);
        let in_flight = target.replace(gpu, owned.ring(2, size)).expect("new images");
        wait_for(&in_flight, gpu);
    }

    if let Some(timeline) = timeline {
        // Replacement with a frame drawn at slot 0 that waits for a timeline,
        // which the host signals only once the replacement returned.
        target.release(gpu, 0, timeline_sync(timeline, 1)).unwrap();
        target.select(0);
        draw_without_ending(&mut target, gpu);
        let in_flight = target.replace(gpu, owned.ring(2, size)).expect("new images");
        assert!(
            !in_flight.finished(gpu).expect("the work failed"),
            "the replacement's frame ran before the host's reads"
        );
        owned.signal(timeline, 1);
        wait_for(&in_flight, gpu);

        // Both slots go back with a value that the host never signals, and a
        // frame begins on slot 1 without drawing: the replacement waits for
        // neither.
        target.release(gpu, 0, timeline_sync(timeline, 10)).unwrap();
        target.release(gpu, 1, timeline_sync(timeline, 10)).unwrap();
        target.select(1);
        target.begin_frame(gpu).expect("a frame");
        let in_flight = target.replace(gpu, owned.ring(2, size)).expect("new images");
        wait_for(&in_flight, gpu);

        // Work of the host's queued after the ring's frame, on the queue
        // that they share, waits for a value that the host signals only once
        // the replacement completed: the replacement waits for the ring's
        // work alone.
        target.release(gpu, 0, timeline_sync(timeline, 2)).unwrap();
        target.select(0);
        draw_without_ending(&mut target, gpu);
        owned.signal(timeline, 2);
        mln_gfx::gpu::Gpu::flush(gpu, false).unwrap();
        owned.queue_host_work(timeline, 3);
        let in_flight = target.replace(gpu, owned.ring(2, size)).expect("new images");
        // The host's work waits on the queue until the signal, so a
        // replacement that waited for it fails here instead of hanging the
        // cleanup.
        let finished = finishes(&in_flight, gpu);
        owned.signal(timeline, 3);
        assert!(finished, "the replacement waited for the host's work");
    }

    // Tear-down with a frame drawn at slot 1, which waits for a timeline that
    // the host signals only once the tear-down returned, where the device
    // takes them, and otherwise a sync file.
    let signaled_after = if let Some(timeline) = timeline {
        target.release(gpu, 1, timeline_sync(timeline, 4)).unwrap();
        Some((timeline, 4))
    } else {
        #[cfg(unix)]
        if let Some(producer) = &mut producer {
            let consumer = sync_file(producer, gpu);
            producer.release(gpu, 0, mln_gfx::ConsumerSync::of(mln_gfx::GpuSync::of(SyncKind::CpuComplete))).unwrap();
            target.release(gpu, 1, consumer).unwrap();
        }
        None
    };
    target.select(1);
    draw_without_ending(&mut target, gpu);
    let in_flight = target.in_flight(gpu);
    target.tear_down(gpu);
    if let Some((timeline, value)) = signaled_after {
        assert!(
            !in_flight.finished(gpu).expect("the work failed"),
            "the tear-down's frame ran before the host's reads"
        );
        owned.signal(timeline, value);
    }
    wait_for(&in_flight, gpu);
    #[cfg(unix)]
    if let Some(mut producer) = producer {
        producer.tear_down(gpu);
    }
}
