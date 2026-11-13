// Devices that sessions share, devices that wrap the host's, and the host's
// own targets: rings of its textures, whose frames go back to it, and its
// surfaces. A backend that this build or machine lacks skips its cases.

#![allow(unsafe_code)]

mod support;

use mln_map::MapMode;
use mln_runtime::{
    Adapter, AttachOptions, BackendType, BorrowedRing, ConsumerSync, DemandMode, Device, DeviceOptions, ErrorKind,
    FrameDemand, FrameDisposition, GlApi, GpuSync, Map, MapOptions, RenderSession, RenderTarget, Runtime, SyncKind,
    SyncKinds,
};
use mln_util::size::Size;
use support::{eventually, runtime, wait};

const SIZE: Size = Size { width: 64, height: 64 };

fn style(color: &str) -> String {
    format!(
        r#"{{"version":8,"transition":{{"duration":0,"delay":0}},"sources":{{}},"layers":[{{"id":"bg","type":"background","paint":{{"background-color":"{color}"}}}}]}}"#
    )
}

fn map_of(runtime: &Runtime, color: &str) -> Map {
    let options = MapOptions {
        map: mln_map::MapOptions { size: SIZE, map_mode: MapMode::Continuous, ..Default::default() },
        ..Default::default()
    };
    let (map, created) = runtime.create_map(options).unwrap();
    created.wait().unwrap();
    wait(map.set_style_json(style(color)));
    map
}

fn attach(map: &Map, target: RenderTarget, producer_sync_kinds: SyncKinds) -> RenderSession {
    let options = AttachOptions { producer_sync_kinds, ..AttachOptions::default() };
    let (session, attached) = map.attach(target, options).unwrap();
    attached.wait().unwrap();
    session
}

/// Renders a forced frame and returns how it finished.
fn render(session: &RenderSession) -> FrameDisposition {
    session.demand_frame(FrameDemand::new(DemandMode::Force, 1)).unwrap();
    let mut results = Vec::new();
    assert!(eventually(|| {
        results.extend(session.drain_frame_results().unwrap());
        !results.is_empty()
    }));
    results[0].disposition
}

fn center_pixel(session: &RenderSession) -> [u8; 4] {
    let image = wait(session.read_texture());
    let offset = ((SIZE.height / 2 * SIZE.width + SIZE.width / 2) * 4) as usize;
    image.data[offset..offset + 4].try_into().unwrap()
}

/// A device of `backend`, or `None` where this build or machine has none.
fn device_of(backend: BackendType) -> Option<Device> {
    device_with(backend, DeviceOptions::default())
}

/// A device of `backend` made with `options`, or `None` where this build or
/// machine has none.
fn device_with(backend: BackendType, options: DeviceOptions) -> Option<Device> {
    match Device::with_backend(backend, options) {
        Ok(device) => Some(device),
        Err(error) if error.kind == ErrorKind::Unsupported => {
            eprintln!("skipped: no {backend:?} device with {options:?}: {error}");
            None
        }
        Err(error) => panic!("a {backend:?} device with {options:?}: {error}"),
    }
}

const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

/// Two maps' sessions on `device`, each of which renders its own color,
/// turn by turn.
fn sessions_of_two_maps_share(device: Device, what: &str) {
    let runtime = runtime();
    let (red, blue) = (map_of(&runtime, "#ff0000"), map_of(&runtime, "#0000ff"));
    let sessions: Vec<_> = [&red, &blue]
        .into_iter()
        .map(|map| {
            let target = RenderTarget::Offscreen { size: SIZE, device: Some(device.clone()) };
            attach(map, target, SyncKinds::NONE)
        })
        .collect();
    // The device lasts while its sessions use it.
    drop(device);
    for _ in 0..2 {
        for session in &sessions {
            assert_eq!(FrameDisposition::Rendered, render(session), "{what}");
        }
        assert_eq!(RED, center_pixel(&sessions[0]), "{what}");
        assert_eq!(BLUE, center_pixel(&sessions[1]), "{what}");
    }
}

#[test]
fn sessions_of_two_maps_share_each_backends_device() {
    for backend in [BackendType::Vulkan, BackendType::OpenGL, BackendType::WebGPU, BackendType::Metal] {
        let Some(device) = device_of(backend) else { continue };
        sessions_of_two_maps_share(device, &format!("{backend:?}"));
    }
}

/// An OpenGL device of either API, whose sessions have a context each or
/// take turns in one.
#[test]
fn sessions_of_two_maps_share_each_kind_of_open_gl_device() {
    for gl_api in [GlApi::Gles, GlApi::Gl] {
        for gl_single_context in [false, true] {
            let options = DeviceOptions { gl_api, gl_single_context, ..DeviceOptions::default() };
            let Some(device) = device_with(BackendType::OpenGL, options) else { continue };
            sessions_of_two_maps_share(device, &format!("{options:?}"));
        }
    }
}

/// OpenGL lists one GPU, the one its display renders on, so an OpenGL
/// device takes the first index and no other.
#[test]
fn an_open_gl_device_names_its_one_gpu_by_index() {
    let first = DeviceOptions { adapter: Some(Adapter::Index(0)), ..DeviceOptions::default() };
    let Some(device) = device_with(BackendType::OpenGL, first) else { return };
    sessions_of_two_maps_share(device, "the first OpenGL GPU");
    let second = DeviceOptions { adapter: Some(Adapter::Index(1)), ..DeviceOptions::default() };
    let error = Device::with_backend(BackendType::OpenGL, second).unwrap_err();
    assert_eq!(ErrorKind::NotFound, error.kind);
}

#[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
mod vulkan {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use mln_vulkan::ash::{self, vk};

    use super::*;

    /// A host's lock that counts the times sessions take it.
    #[derive(Default)]
    struct CountingLock {
        locks: AtomicUsize,
        unlocks: AtomicUsize,
    }

    impl mln_vulkan::QueueLock for CountingLock {
        fn lock(&self) -> bool {
            self.locks.fetch_add(1, Ordering::SeqCst);
            true
        }
        fn unlock(&self) {
            self.unlocks.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// The library's device, wrapped again as if it were the host's, and
    /// the library's device, which the wrap borrows.
    fn wrapped(lock: Option<Arc<dyn mln_vulkan::QueueLock>>) -> Option<(Device, Device)> {
        let Some(Device::Vulkan(library)) = device_of(BackendType::Vulkan) else { return None };
        let raw = library.raw();
        let host = mln_vulkan::HostDevice {
            get_instance_proc_addr: Some(raw.get_instance_proc_addr),
            get_device_proc_addr: Some(raw.get_device_proc_addr),
            instance: raw.instance,
            physical_device: raw.physical_device,
            device: raw.device,
            queue_family_index: raw.queue_family_index,
            queue: raw.queue,
            queue_lock: lock,
            enabled_extensions: library.enabled_extensions().to_vec(),
        };
        // SAFETY: the library device's live handles, which outlive the wrap:
        // the test drops the wrap's sessions first.
        let wrap = unsafe { mln_vulkan::Device::from_raw(host) }.unwrap();
        Some((Device::Vulkan(wrap), Device::Vulkan(library)))
    }

    #[test]
    fn a_wrapped_device_renders_under_the_hosts_queue_lock() {
        let lock = Arc::new(CountingLock::default());
        let Some((wrap, library)) = wrapped(Some(lock.clone())) else { return };
        let runtime = runtime();
        let (red, blue) = (map_of(&runtime, "#ff0000"), map_of(&runtime, "#0000ff"));
        // A session of the wrap and one of the library's device share the
        // queue.
        let on_wrap = attach(&red, RenderTarget::Offscreen { size: SIZE, device: Some(wrap) }, SyncKinds::NONE);
        let on_library = attach(&blue, RenderTarget::Offscreen { size: SIZE, device: Some(library) }, SyncKinds::NONE);
        assert_eq!(FrameDisposition::Rendered, render(&on_wrap));
        assert_eq!(FrameDisposition::Rendered, render(&on_library));
        assert_eq!(RED, center_pixel(&on_wrap));
        assert_eq!(BLUE, center_pixel(&on_library));
        wait(on_wrap.detach());
        wait(on_library.detach());
        let locks = lock.locks.load(Ordering::SeqCst);
        assert!(locks > 0);
        assert_eq!(locks, lock.unlocks.load(Ordering::SeqCst));
    }

    /// A queue lock that counts how many times it was dropped.
    struct DroppedLock(Arc<AtomicUsize>);

    impl mln_vulkan::QueueLock for DroppedLock {
        fn lock(&self) -> bool {
            true
        }
        fn unlock(&self) {}
    }

    impl Drop for DroppedLock {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    // A caller-driven session abandoned off its thread keeps its graphics
    // objects, which hold its device, but lets go of the host's queue lock as
    // the abandon returns: the session calls it no more.
    #[test]
    fn an_abandon_off_the_drivers_thread_lets_go_of_the_hosts_queue_lock() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let Some((wrap, library)) = wrapped(Some(Arc::new(DroppedLock(dropped.clone())))) else { return };
        let runtime = runtime();
        let map = map_of(&runtime, "#ff0000");
        let target = RenderTarget::Offscreen { size: SIZE, device: Some(wrap) };
        let options = AttachOptions { driver: mln_runtime::Driver::CallerThread, ..AttachOptions::default() };
        let (session, attached) = map.attach(target, options).unwrap();
        // The host's graphics thread, which renders a frame, then stops
        // servicing the session and keeps its driver work until the abandon.
        let (rendered_sender, rendered) = std::sync::mpsc::channel();
        let (abandoned_sender, abandoned) = std::sync::mpsc::channel::<()>();
        std::thread::scope(|scope| {
            // Dropped as the scope unwinds, before it joins the thread.
            let abandoned_sender = abandoned_sender;
            let (session, attached) = (&session, &attached);
            scope.spawn(move || {
                let driver = session.take_driver_work().unwrap();
                service_until(&driver, || attached.is_complete());
                session.demand_frame(FrameDemand::new(DemandMode::Force, 1)).unwrap();
                let mut results = Vec::new();
                service_until(&driver, || {
                    results.extend(session.drain_frame_results().unwrap());
                    !results.is_empty()
                });
                rendered_sender.send(results[0].disposition).unwrap();
                let _ = abandoned.recv();
            });
            assert_eq!(FrameDisposition::Rendered, rendered.recv().unwrap());
            assert_eq!(0, dropped.load(Ordering::SeqCst));
            assert_ne!(0, session.abandon().unwrap(), "the abandon kept no graphics objects");
            assert_eq!(1, dropped.load(Ordering::SeqCst));
            abandoned_sender.send(()).unwrap();
        });
        // The session's graphics objects stay on the device until the
        // process exits, so the host's device does too.
        std::mem::forget(library);
    }

    /// A host's lock on a queue that it shares with sessions.
    #[derive(Default)]
    struct HostQueueLock {
        locked: parking_lot::Mutex<bool>,
        unlocked: parking_lot::Condvar,
    }

    impl mln_vulkan::QueueLock for HostQueueLock {
        fn lock(&self) -> bool {
            let mut locked = self.locked.lock();
            while *locked {
                self.unlocked.wait(&mut locked);
            }
            *locked = true;
            true
        }
        fn unlock(&self) {
            *self.locked.lock() = false;
            self.unlocked.notify_one();
        }
    }

    /// The host's side of a Vulkan device: the images and the timeline that
    /// it makes, and the work it submits to the queue, under its lock.
    struct Host {
        instance: ash::Instance,
        device: ash::Device,
        raw: mln_vulkan::RawDevice,
        lock: Arc<HostQueueLock>,
        images: Vec<(vk::Image, vk::ImageView, vk::DeviceMemory)>,
        semaphores: Vec<vk::Semaphore>,
        buffers: Vec<(vk::Buffer, vk::DeviceMemory)>,
        pool: vk::CommandPool,
        fences: Vec<vk::Fence>,
    }

    impl Host {
        /// The library's Vulkan device, with the host's lock on its queue,
        /// and the host's side of it.
        fn new() -> Option<(Device, Host)> {
            let Some(Device::Vulkan(vulkan)) = device_of(BackendType::Vulkan) else { return None };
            let lock = Arc::new(HostQueueLock::default());
            let vulkan = vulkan.with_queue_lock(lock.clone()).unwrap();
            let raw = vulkan.raw();
            // SAFETY: the device's own loader function and handles.
            let (instance, device) = unsafe {
                let entry =
                    ash::Entry::from_static_fn(ash::StaticFn { get_instance_proc_addr: raw.get_instance_proc_addr });
                let instance = ash::Instance::load(entry.static_fn(), raw.instance);
                let device = ash::Device::load(instance.fp_v1_0(), raw.device);
                (instance, device)
            };
            let info = vk::CommandPoolCreateInfo::default().queue_family_index(raw.queue_family_index);
            // SAFETY: a valid create info.
            let pool = unsafe { device.create_command_pool(&info, None) }.unwrap();
            let host = Host {
                instance,
                device,
                raw,
                lock,
                images: Vec::new(),
                semaphores: Vec::new(),
                buffers: Vec::new(),
                pool,
                fences: Vec::new(),
            };
            Some((Device::Vulkan(vulkan), host))
        }

        fn memory(&self, requirements: vk::MemoryRequirements, flags: vk::MemoryPropertyFlags) -> vk::DeviceMemory {
            // SAFETY: a plain query of the GPU.
            let properties = unsafe { self.instance.get_physical_device_memory_properties(self.raw.physical_device) };
            let index = (0..properties.memory_type_count)
                .find(|&index| {
                    requirements.memory_type_bits & (1 << index) != 0
                        && properties.memory_types[index as usize].property_flags.contains(flags)
                })
                .unwrap();
            let info = vk::MemoryAllocateInfo::default().allocation_size(requirements.size).memory_type_index(index);
            // SAFETY: a valid allocate info.
            unsafe { self.device.allocate_memory(&info, None) }.unwrap()
        }

        /// An RGBA8 image of the ring's size that sessions render into and
        /// the host copies from.
        fn image(&mut self) -> mln_vulkan::BorrowedImage {
            let info = vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(vk::Format::R8G8B8A8_UNORM)
                .extent(vk::Extent3D { width: SIZE.width, height: SIZE.height, depth: 1 })
                .mip_levels(1)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC)
                .initial_layout(vk::ImageLayout::UNDEFINED);
            // SAFETY: valid create infos, and memory of the image's
            // requirements.
            unsafe {
                let image = self.device.create_image(&info, None).unwrap();
                let memory =
                    self.memory(self.device.get_image_memory_requirements(image), vk::MemoryPropertyFlags::empty());
                self.device.bind_image_memory(image, memory, 0).unwrap();
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
                let view = self.device.create_image_view(&view_info, None).unwrap();
                self.images.push((image, view, memory));
                mln_vulkan::BorrowedImage { image, view }
            }
        }

        fn ring(images: &[mln_vulkan::BorrowedImage]) -> BorrowedRing {
            BorrowedRing::vulkan(mln_vulkan::BorrowedImages {
                images: images.to_vec(),
                size: SIZE,
                format: vk::Format::R8G8B8A8_UNORM,
                final_layout: vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                owner: mln_vulkan::ImageOwner::GraphicsQueue,
                depth: None,
            })
        }

        /// A timeline semaphore of the host's, at zero.
        fn timeline(&mut self) -> vk::Semaphore {
            let mut type_info =
                vk::SemaphoreTypeCreateInfo::default().semaphore_type(vk::SemaphoreType::TIMELINE).initial_value(0);
            let info = vk::SemaphoreCreateInfo::default().push_next(&mut type_info);
            // SAFETY: a valid create info, on a device with timelines.
            let semaphore = unsafe { self.device.create_semaphore(&info, None) }.unwrap();
            self.semaphores.push(semaphore);
            semaphore
        }

        fn counter(&self, semaphore: vk::Semaphore) -> u64 {
            // SAFETY: a timeline of the device.
            unsafe { self.device.get_semaphore_counter_value(semaphore) }.unwrap()
        }

        fn signal(&self, semaphore: vk::Semaphore, value: u64) {
            let info = vk::SemaphoreSignalInfo::default().semaphore(semaphore).value(value);
            // SAFETY: a timeline of the device, below `value`.
            unsafe { self.device.signal_semaphore(&info) }.unwrap();
        }

        /// Submits a copy of `image`, in its final layout, to a buffer, to
        /// the queue that sessions share, without waiting for it.
        fn read(&mut self, image: vk::Image) {
            // SAFETY: valid create and allocate infos; the command buffer
            // records one copy of the image, in the layout the ring leaves
            // it in, and goes to the queue under the host's lock.
            unsafe {
                use mln_vulkan::QueueLock as _;
                let size = u64::from(SIZE.width * SIZE.height * 4);
                let info = vk::BufferCreateInfo::default().size(size).usage(vk::BufferUsageFlags::TRANSFER_DST);
                let buffer = self.device.create_buffer(&info, None).unwrap();
                let memory =
                    self.memory(self.device.get_buffer_memory_requirements(buffer), vk::MemoryPropertyFlags::empty());
                self.device.bind_buffer_memory(buffer, memory, 0).unwrap();
                self.buffers.push((buffer, memory));
                let info = vk::CommandBufferAllocateInfo::default()
                    .command_pool(self.pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1);
                let command_buffer = self.device.allocate_command_buffers(&info).unwrap()[0];
                self.device.begin_command_buffer(command_buffer, &vk::CommandBufferBeginInfo::default()).unwrap();
                // A frame in queue order is the host's to wait for.
                let after_frame = vk::MemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::MEMORY_WRITE)
                    .dst_access_mask(vk::AccessFlags::TRANSFER_READ);
                self.device.cmd_pipeline_barrier(
                    command_buffer,
                    vk::PipelineStageFlags::ALL_COMMANDS,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::DependencyFlags::empty(),
                    &[after_frame],
                    &[],
                    &[],
                );
                let region = vk::BufferImageCopy::default()
                    .image_subresource(vk::ImageSubresourceLayers {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        mip_level: 0,
                        base_array_layer: 0,
                        layer_count: 1,
                    })
                    .image_extent(vk::Extent3D { width: SIZE.width, height: SIZE.height, depth: 1 });
                self.device.cmd_copy_image_to_buffer(
                    command_buffer,
                    image,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    buffer,
                    &[region],
                );
                self.device.end_command_buffer(command_buffer).unwrap();
                let fence = self.device.create_fence(&vk::FenceCreateInfo::default(), None).unwrap();
                self.fences.push(fence);
                let command_buffers = [command_buffer];
                let submit = vk::SubmitInfo::default().command_buffers(&command_buffers);
                assert!(self.lock.lock());
                let submitted = self.device.queue_submit(self.raw.queue, &[submit], fence);
                self.lock.unlock();
                submitted.unwrap();
            }
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            // SAFETY: the host's objects, which no session uses any more,
            // once the host's own work is done.
            unsafe {
                if !self.fences.is_empty() {
                    let _ = self.device.wait_for_fences(&self.fences, true, u64::MAX);
                }
                for fence in self.fences.drain(..) {
                    self.device.destroy_fence(fence, None);
                }
                self.device.destroy_command_pool(self.pool, None);
                for (buffer, memory) in self.buffers.drain(..) {
                    self.device.destroy_buffer(buffer, None);
                    self.device.free_memory(memory, None);
                }
                for semaphore in self.semaphores.drain(..) {
                    self.device.destroy_semaphore(semaphore, None);
                }
                for (image, view, memory) in self.images.drain(..) {
                    self.device.destroy_image_view(view, None);
                    self.device.destroy_image(image, None);
                    self.device.free_memory(memory, None);
                }
            }
        }
    }

    fn timeline_sync(semaphore: vk::Semaphore, value: u64) -> ConsumerSync {
        use vk::Handle as _;
        ConsumerSync::of(GpuSync { kind: SyncKind::VulkanTimelineSemaphore, handle: semaphore.as_raw(), value })
    }

    /// The host and the library's device, with timelines, or `None` where
    /// there is none.
    fn with_timelines() -> Option<(Device, Host)> {
        let (device, host) = Host::new()?;
        let Device::Vulkan(vulkan) = &device else { unreachable!() };
        if !vulkan.sync_kinds().1.contains(SyncKind::VulkanTimelineSemaphore) {
            eprintln!("skipped: the Vulkan device has no timelines");
            return None;
        }
        Some((device, host))
    }

    // A frame waits for the host's timeline at zero, which it has reached
    // already, though no other semaphore of its submission has a value. The
    // validation layer checks the submission.
    #[test]
    fn a_ring_waits_for_the_hosts_timeline_at_zero() {
        let Some((device, mut host)) = with_timelines() else { return };
        let images = [host.image()];
        let timeline = host.timeline();
        let runtime = runtime();
        let map = map_of(&runtime, "#ff0000");
        let target = RenderTarget::Borrowed { size: SIZE, ring: Host::ring(&images), device: device.clone() };
        let session = attach(&map, target, SyncKinds::of(&[SyncKind::CpuComplete]));
        assert_eq!(FrameDisposition::Rendered, render(&session));
        let frame = session.acquire_frame().unwrap().expect("a frame");
        assert_eq!(SyncKind::CpuComplete, frame.producer_sync().kind);
        frame.release(timeline_sync(timeline, 0)).unwrap();
        assert_eq!(FrameDisposition::Rendered, render(&session));
        drop(session.acquire_frame().unwrap().expect("a frame"));
        wait(session.detach());
    }

    // The host's timeline wait holds a frame back, even when a new ring of
    // another size has the frame's image again.
    #[test]
    fn a_ring_waits_for_the_hosts_timeline_in_a_new_ring() {
        let Some((device, mut host)) = with_timelines() else { return };
        let images = [host.image()];
        let timeline = host.timeline();
        let runtime = runtime();
        let map = map_of(&runtime, "#ff0000");
        let target = RenderTarget::Borrowed { size: SIZE, ring: Host::ring(&images), device: device.clone() };
        let session = attach(&map, target, SyncKinds::of(&[SyncKind::VulkanTimelineSemaphore]));
        let produced = |frame: &mln_runtime::AcquiredFrame| {
            use vk::Handle as _;
            let sync = frame.producer_sync();
            assert_eq!(SyncKind::VulkanTimelineSemaphore, sync.kind);
            (vk::Semaphore::from_raw(sync.handle), sync.value)
        };

        assert_eq!(FrameDisposition::Rendered, render(&session));
        let frame = session.acquire_frame().unwrap().expect("a frame");
        let (producer, value) = produced(&frame);
        assert!(eventually(|| host.counter(producer) >= value));
        frame.release(timeline_sync(timeline, 1)).unwrap();
        wait(session.set_ring(Size::new(SIZE.width / 2, SIZE.height / 2), Host::ring(&images)));
        assert_eq!(FrameDisposition::Rendered, render(&session));
        let frame = session.acquire_frame().unwrap().expect("a frame");
        let (_, value) = produced(&frame);
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(host.counter(producer) < value, "the frame didn't wait for the host's timeline");
        host.signal(timeline, 1);
        assert!(eventually(|| host.counter(producer) >= value));
        drop(frame);
        wait(session.detach());
    }

    fn timeline_of(frame: &mln_runtime::AcquiredFrame) -> (vk::Semaphore, u64) {
        use vk::Handle as _;
        let sync = frame.producer_sync();
        assert_eq!(SyncKind::VulkanTimelineSemaphore, sync.kind);
        (vk::Semaphore::from_raw(sync.handle), sync.value)
    }

    // A release of the ring's images completes once the host's reads of
    // them end, so a new ring of the same image draws only after them. A
    // wait that runs out still completes the release, and quarantines
    // nothing.
    #[test]
    fn a_release_of_the_ring_waits_for_the_hosts_reads() {
        let Some((device, mut host)) = with_timelines() else { return };
        let images = [host.image()];
        let timeline = host.timeline();
        let runtime = runtime();
        let map = map_of(&runtime, "#ff0000");
        let target = RenderTarget::Borrowed { size: SIZE, ring: Host::ring(&images), device: device.clone() };
        let session = attach(&map, target, SyncKinds::of(&[SyncKind::VulkanTimelineSemaphore]));
        assert_eq!(FrameDisposition::Rendered, render(&session));
        let frame = session.acquire_frame().unwrap().expect("a frame");
        let (producer, value) = timeline_of(&frame);
        assert!(eventually(|| host.counter(producer) >= value));
        frame.release(timeline_sync(timeline, 1)).unwrap();

        let host = &host;
        std::thread::scope(|scope| {
            let released = session.release_target().unwrap();
            scope.spawn(|| {
                std::thread::sleep(std::time::Duration::from_millis(400));
                host.signal(timeline, 1);
            });
            released.wait().unwrap();
            wait(session.set_ring(SIZE, Host::ring(&images)));
            assert_eq!(FrameDisposition::Rendered, render(&session));
            let frame = session.acquire_frame().unwrap().expect("a frame");
            let (producer, value) = timeline_of(&frame);
            assert!(eventually(|| host.counter(producer) >= value));
            assert!(host.counter(timeline) >= 1, "the new ring drew into the image before the host's reads ended");
            // The host never signals 2.
            frame.release(timeline_sync(timeline, 2)).unwrap();
        });
        let started = std::time::Instant::now();
        wait(session.release_target());
        assert!(started.elapsed() >= std::time::Duration::from_millis(900));
        wait(session.set_ring(SIZE, Host::ring(&images)));
        assert_eq!(FrameDisposition::Rendered, render(&session));
        drop(session.acquire_frame().unwrap().expect("a frame"));
        wait(session.detach());
    }

    // A new ring that lets go of an image completes once the host's reads
    // of it end.
    #[test]
    fn a_new_ring_waits_for_the_hosts_reads_of_the_image_it_lets_go_of() {
        let Some((device, mut host)) = with_timelines() else { return };
        let (old, new) = ([host.image()], [host.image()]);
        let timeline = host.timeline();
        let runtime = runtime();
        let map = map_of(&runtime, "#ff0000");
        let target = RenderTarget::Borrowed { size: SIZE, ring: Host::ring(&old), device: device.clone() };
        let session = attach(&map, target, SyncKinds::of(&[SyncKind::VulkanTimelineSemaphore]));
        assert_eq!(FrameDisposition::Rendered, render(&session));
        let frame = session.acquire_frame().unwrap().expect("a frame");
        let (producer, value) = timeline_of(&frame);
        assert!(eventually(|| host.counter(producer) >= value));
        frame.release(timeline_sync(timeline, 1)).unwrap();

        let host = &host;
        std::thread::scope(|scope| {
            let replaced = session.set_ring(SIZE, Host::ring(&new)).unwrap();
            scope.spawn(|| {
                std::thread::sleep(std::time::Duration::from_millis(400));
                host.signal(timeline, 1);
            });
            replaced.wait().unwrap();
            assert!(host.counter(timeline) >= 1, "the new ring let go of the image before the host's reads ended");
        });
        wait(session.detach());
    }

    /// Services `driver` until `done` holds.
    fn service_until(driver: &mln_runtime::DriverWork, mut done: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + support::TIMEOUT;
        while !done() {
            assert!(std::time::Instant::now() < deadline, "the driver never got there");
            driver.service().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    // A caller-driven session's release waits for the host's reads on its
    // later services, which return at once: the host can signal from work
    // that it submits after a service.
    #[test]
    fn a_caller_driven_release_waits_for_the_hosts_reads_on_later_services() {
        let Some((device, mut host)) = with_timelines() else { return };
        let images = [host.image()];
        let timeline = host.timeline();
        let runtime = runtime();
        let map = map_of(&runtime, "#ff0000");
        let target = RenderTarget::Borrowed { size: SIZE, ring: Host::ring(&images), device: device.clone() };
        let options = AttachOptions {
            driver: mln_runtime::Driver::CallerThread,
            producer_sync_kinds: SyncKinds::of(&[SyncKind::VulkanTimelineSemaphore]),
            ..AttachOptions::default()
        };
        let (session, attached) = map.attach(target, options).unwrap();
        let driver = session.take_driver_work().unwrap();
        service_until(&driver, || attached.is_complete());
        attached.wait().unwrap();
        session.demand_frame(FrameDemand::new(DemandMode::Force, 1)).unwrap();
        let mut results = Vec::new();
        service_until(&driver, || {
            results.extend(session.drain_frame_results().unwrap());
            !results.is_empty()
        });
        assert_eq!(FrameDisposition::Rendered, results[0].disposition);
        let frame = session.acquire_frame().unwrap().expect("a frame");
        let (producer, value) = timeline_of(&frame);
        assert!(eventually(|| host.counter(producer) >= value));
        frame.release(timeline_sync(timeline, 1)).unwrap();

        let released = session.release_target().unwrap();
        let started = std::time::Instant::now();
        driver.service().unwrap();
        driver.service().unwrap();
        assert!(started.elapsed() < std::time::Duration::from_millis(500), "a service waited for the host's reads");
        assert!(!released.is_complete());
        host.signal(timeline, 1);
        service_until(&driver, || released.is_complete());
        released.wait().unwrap();
        assert!(started.elapsed() < std::time::Duration::from_millis(900), "the release waited out its timeout");
        let detached = session.detach().unwrap();
        service_until(&driver, || detached.is_complete());
        detached.wait().unwrap();
    }

    // An abandon doesn't wait for a release that waits for the host's reads,
    // which fails instead.
    #[test]
    fn an_abandon_ends_a_release_that_waits_for_the_hosts_reads() {
        let Some((device, mut host)) = with_timelines() else { return };
        let images = [host.image()];
        let timeline = host.timeline();
        let runtime = runtime();
        let map = map_of(&runtime, "#ff0000");
        let target = RenderTarget::Borrowed { size: SIZE, ring: Host::ring(&images), device: device.clone() };
        let session = attach(&map, target, SyncKinds::of(&[SyncKind::VulkanTimelineSemaphore]));
        assert_eq!(FrameDisposition::Rendered, render(&session));
        let frame = session.acquire_frame().unwrap().expect("a frame");
        let (producer, value) = timeline_of(&frame);
        assert!(eventually(|| host.counter(producer) >= value));
        frame.release(timeline_sync(timeline, 1)).unwrap();
        let released = session.release_target().unwrap();
        // The release runs on the session's thread before the abandon.
        assert!(eventually(|| !session.snapshot().unwrap().has_target));
        let started = std::time::Instant::now();
        session.abandon().unwrap();
        assert!(started.elapsed() < std::time::Duration::from_millis(500), "the abandon waited for the host's reads");
        assert_eq!(ErrorKind::TargetLost, released.wait().unwrap_err().kind);
    }

    // The host reads each frame on the queue that it shares with the
    // session, and gives it back in queue order: the next frame waits for
    // those reads before it clears the image. The validation layer checks
    // the frames.
    #[test]
    fn a_ring_waits_for_the_hosts_reads_in_queue_order() {
        let Some((device, mut host)) = Host::new() else { return };
        let images = [host.image()];
        let runtime = runtime();
        let map = map_of(&runtime, "#ff0000");
        let target = RenderTarget::Borrowed { size: SIZE, ring: Host::ring(&images), device: device.clone() };
        let session = attach(&map, target, SyncKinds::of(&[SyncKind::QueueOrder]));
        for _ in 0..3 {
            assert_eq!(FrameDisposition::Rendered, render(&session));
            let frame = session.acquire_frame().unwrap().expect("a frame");
            assert_eq!(SyncKind::QueueOrder, frame.producer_sync().kind);
            host.read(images[0].image);
            frame.release(ConsumerSync::of(GpuSync::of(SyncKind::QueueOrder))).unwrap();
        }
        wait(session.detach());
    }

    #[test]
    fn a_session_presents_to_the_hosts_surface() {
        use vk::Handle as _;
        let Some(device) = device_of(BackendType::Vulkan) else { return };
        let Device::Vulkan(vulkan) = &device else { unreachable!() };
        if !vulkan.presents() {
            eprintln!("skipped: the Vulkan device presents to no surface");
            return;
        }
        let raw = vulkan.raw();
        // SAFETY: the device's own loader function and instance.
        let (entry, instance) = unsafe {
            let entry =
                ash::Entry::from_static_fn(ash::StaticFn { get_instance_proc_addr: raw.get_instance_proc_addr });
            let instance = ash::Instance::load(entry.static_fn(), raw.instance);
            (entry, instance)
        };
        // The library's instance enables VK_EXT_headless_surface where the
        // loader has it, which its function tells: Wine's loader, for one,
        // has only the platform's surfaces.
        // SAFETY: the device's live instance, and a function name.
        let create = unsafe { (raw.get_instance_proc_addr)(raw.instance, c"vkCreateHeadlessSurfaceEXT".as_ptr()) };
        if create.is_none() {
            eprintln!("skipped: the instance has no VK_EXT_headless_surface");
            return;
        }
        let headless = ash::ext::headless_surface::Instance::new(&entry, &instance);
        // SAFETY: the instance has the extension, as above.
        let Ok(surface) =
            (unsafe { headless.create_headless_surface(&vk::HeadlessSurfaceCreateInfoEXT::default(), None) })
        else {
            eprintln!("skipped: no headless surface");
            return;
        };
        let runtime = runtime();
        let map = map_of(&runtime, "#ff0000");
        // SAFETY: a live surface of the device's instance, destroyed after
        // the session lets go of it.
        let host_surface = unsafe { mln_vulkan::HostSurface::from_raw(surface.as_raw()) };
        let target = RenderTarget::Surface {
            size: SIZE,
            surface: mln_runtime::Surface::vulkan(host_surface),
            device: Some(device.clone()),
        };
        let session = attach(&map, target, SyncKinds::NONE);
        assert_eq!(FrameDisposition::Rendered, render(&session));
        assert_eq!(FrameDisposition::Rendered, render(&session));
        wait(session.detach());
        // SAFETY: nothing uses the surface since the detach.
        unsafe { ash::khr::surface::Instance::new(&entry, &instance).destroy_surface(surface, None) };
    }
}

#[cfg(feature = "webgpu")]
#[test]
fn a_webgpu_ring_hands_each_frame_back() {
    let Some(device) = device_of(BackendType::WebGPU) else { return };
    let Device::WebGpu(webgpu) = &device else { unreachable!() };
    let (wgpu_device, queue) = (webgpu.device().clone(), webgpu.queue().clone());
    let texture = || {
        wgpu_device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: SIZE.width, height: SIZE.height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    };
    let read = |texture: &wgpu::Texture| -> [u8; 4] {
        let bytes_per_row = SIZE.width * 4;
        let buffer = wgpu_device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(bytes_per_row * SIZE.height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = wgpu_device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: None,
                },
            },
            texture.size(),
        );
        queue.submit([encoder.finish()]);
        buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        wgpu_device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
        let data = buffer.slice(..).get_mapped_range().unwrap();
        let offset = ((SIZE.height / 2 * SIZE.width + SIZE.width / 2) * 4) as usize;
        data[offset..offset + 4].try_into().unwrap()
    };
    let textures = vec![texture(), texture()];
    let ring = BorrowedRing::webgpu(mln_webgpu::BorrowedTextures {
        textures: textures.clone(),
        size: SIZE,
        depth_textures: Vec::new(),
    });
    let runtime = runtime();
    let map = map_of(&runtime, "#ff0000");
    let target = RenderTarget::Borrowed { size: SIZE, ring, device: device.clone() };
    let session = attach(&map, target, SyncKinds::of(&[SyncKind::QueueOrder]));
    let capabilities = session.capabilities().unwrap();
    assert_eq!(
        (2, true, false),
        (capabilities.texture_ring_depth, capabilities.frame_acquisition, capabilities.readback)
    );
    assert_eq!(Some(SyncKind::QueueOrder), capabilities.producer_sync_kind);

    assert_eq!(FrameDisposition::Rendered, render(&session));
    let first = session.acquire_frame().unwrap().expect("a frame");
    assert_eq!(SyncKind::QueueOrder, first.producer_sync().kind);
    assert_eq!(RED, read(&textures[first.slot()]));
    // The ring is sized by its owner, and the host reads its own textures.
    let extent = mln_runtime::LogicalExtent::new(SIZE, 1.0);
    assert_eq!(ErrorKind::Unsupported, session.resize(extent).unwrap_err().kind);
    assert_eq!(ErrorKind::Unsupported, session.read_texture().unwrap_err().kind);

    wait(map.set_style_json(style("#0000ff")));
    assert_eq!(FrameDisposition::Rendered, render(&session));
    let second = session.acquire_frame().unwrap().expect("a frame");
    assert_ne!(first.slot(), second.slot());
    assert_eq!(BLUE, read(&textures[second.slot()]));
    assert_eq!(RED, read(&textures[first.slot()]), "a held frame keeps its texture");
    // Both textures are held, so the next frame waits for one, until the
    // barrier ends it.
    session.demand_frame(FrameDemand::new(DemandMode::Force, 2)).unwrap();
    wait(session.barrier());
    let ended = session.drain_frame_results().unwrap();
    assert_eq!(
        vec![(Some(2), FrameDisposition::NoUpdate)],
        ended.iter().map(|r| (r.token, r.disposition)).collect::<Vec<_>>()
    );
    first.release(ConsumerSync::of(GpuSync::of(SyncKind::QueueOrder))).unwrap();
    second.release(ConsumerSync::of(GpuSync::of(SyncKind::CpuComplete))).unwrap();

    // New textures, as many as the ring has.
    let replacements = vec![texture(), texture()];
    let ring = BorrowedRing::webgpu(mln_webgpu::BorrowedTextures {
        textures: replacements.clone(),
        size: SIZE,
        depth_textures: Vec::new(),
    });
    wait(session.set_ring(SIZE, ring));
    assert_eq!(FrameDisposition::Rendered, render(&session));
    let frame = session.acquire_frame().unwrap().expect("a frame");
    assert_eq!(BLUE, read(&replacements[frame.slot()]));
    drop(frame);
    wait(session.detach());
}

/// A WebGPU device and `count` textures of it for a ring, or `None` where
/// this build or machine has no WebGPU device.
#[cfg(feature = "webgpu")]
fn webgpu_ring(count: usize) -> Option<(Device, Vec<wgpu::Texture>)> {
    let device = device_of(BackendType::WebGPU)?;
    let textures = (0..count).map(|_| webgpu_texture(&device)).collect();
    Some((device, textures))
}

/// A texture of a WebGPU device for a ring.
#[cfg(feature = "webgpu")]
fn webgpu_texture(device: &Device) -> wgpu::Texture {
    let Device::WebGpu(webgpu) = device else { unreachable!() };
    webgpu.device().create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: SIZE.width, height: SIZE.height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

#[cfg(feature = "webgpu")]
fn webgpu_session(map: &Map, device: &Device, textures: &[wgpu::Texture], options: AttachOptions) -> RenderSession {
    let ring = BorrowedRing::webgpu(mln_webgpu::BorrowedTextures {
        textures: textures.to_vec(),
        size: SIZE,
        depth_textures: Vec::new(),
    });
    let target = RenderTarget::Borrowed { size: SIZE, ring, device: device.clone() };
    let options = AttachOptions { producer_sync_kinds: SyncKinds::of(&[SyncKind::QueueOrder]), ..options };
    let (session, attached) = map.attach(target, options).unwrap();
    attached.wait().unwrap();
    session
}

// While the host holds the frame in a ring's only texture, no frame renders
// into it, however the host's acquires and the session's demands interleave.
#[cfg(feature = "webgpu")]
#[test]
fn no_frame_renders_into_a_texture_that_the_host_holds() {
    const DEMANDS: u64 = 10_000;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    let Some((device, textures)) = webgpu_ring(1) else { return };
    let runtime = runtime();
    let map = map_of(&runtime, "#ff0000");
    let session = webgpu_session(&map, &device, &textures, AttachOptions::default());
    let done = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let host = scope.spawn(|| {
            let mut held = 0u64;
            while !done.load(Ordering::SeqCst) {
                let Some(frame) = session.acquire_frame().unwrap() else {
                    std::hint::spin_loop();
                    continue;
                };
                // A frame that rendered meanwhile would count past it.
                let generation = frame.result().frame_generation;
                let until = Instant::now() + Duration::from_millis(1);
                while Instant::now() < until {
                    assert_eq!(
                        generation,
                        session.snapshot().unwrap().frame_generation,
                        "a frame rendered into the texture that the host holds"
                    );
                }
                frame.release(ConsumerSync::of(GpuSync::of(SyncKind::QueueOrder))).unwrap();
                held += 1;
            }
            held
        });
        for token in 0..DEMANDS {
            session.demand_frame(FrameDemand::new(DemandMode::Force, token)).unwrap();
        }
        let mut finished = 0;
        let deadline = Instant::now() + Duration::from_secs(600);
        while finished < DEMANDS && !host.is_finished() && Instant::now() < deadline {
            finished += session.drain_frame_results().unwrap().len() as u64;
            std::thread::sleep(Duration::from_millis(1));
        }
        done.store(true, Ordering::SeqCst);
        assert!(host.join().unwrap() > 0);
        assert_eq!(DEMANDS, finished);
    });
    wait(session.detach());
}

// A frame that the host acquires while a release of the ring's textures is
// on its way would name a texture that the release lets go of: none is
// acquired, and the session stays attached.
#[cfg(feature = "webgpu")]
#[test]
fn no_frame_is_acquired_while_a_release_of_the_ring_is_on_its_way() {
    use std::sync::{Arc, mpsc};

    let Some((device, textures)) = webgpu_ring(2) else { return };
    let runtime = runtime();
    let map = map_of(&runtime, "#ff0000");
    // The frame wake runs on the session's thread: blocking it holds the
    // release back.
    let (block, blocked) = mpsc::channel::<mpsc::Receiver<()>>();
    let blocked = parking_lot::Mutex::new(blocked);
    let frame_wake: mln_runtime::Wake = Arc::new(move || {
        if let Ok(unblock) = blocked.lock().try_recv() {
            let _ = unblock.recv();
        }
    });
    let options = AttachOptions { frame_wake: Some(frame_wake), ..AttachOptions::default() };
    let session = webgpu_session(&map, &device, &textures, options);
    let (unblock, unblocked) = mpsc::channel();
    block.send(unblocked).unwrap();
    session.demand_frame(FrameDemand::new(DemandMode::Force, 1)).unwrap();
    // The frame is in its texture once its result is, and the wake that
    // follows holds the session's thread.
    assert!(eventually(|| session.snapshot().unwrap().frame_generation == 1));
    let released = session.release_target().unwrap();
    assert!(session.acquire_frame().unwrap().is_none());
    unblock.send(()).unwrap();
    released.wait().unwrap();
    assert!(session.acquire_frame().unwrap().is_none());
    assert_eq!(mln_runtime::RenderSessionState::Attached, session.snapshot().unwrap().state);
    wait(session.detach());
}

// A texture that the host gave up stays quarantined when a new ring of
// another size has it again.
#[cfg(feature = "webgpu")]
#[test]
fn a_texture_that_the_host_gave_up_stays_quarantined_in_a_new_ring() {
    let Some((device, textures)) = webgpu_ring(1) else { return };
    let runtime = runtime();
    let map = map_of(&runtime, "#ff0000");
    let session = webgpu_session(&map, &device, &textures, AttachOptions::default());
    assert_eq!(FrameDisposition::Rendered, render(&session));
    session.acquire_frame().unwrap().expect("a frame").discard();
    let quarantined = |session: &RenderSession| {
        session
            .demand_frame(FrameDemand::new(DemandMode::Force, 2))
            .is_err_and(|error| error.kind == ErrorKind::InvalidState)
    };
    assert!(eventually(|| quarantined(&session)));

    let ring = BorrowedRing::webgpu(mln_webgpu::BorrowedTextures {
        textures: textures.clone(),
        size: SIZE,
        depth_textures: Vec::new(),
    });
    wait(session.set_ring(Size::new(SIZE.width / 2, SIZE.height / 2), ring));
    assert!(quarantined(&session));

    // A new texture renders.
    let ring = BorrowedRing::webgpu(mln_webgpu::BorrowedTextures {
        textures: vec![webgpu_texture(&device)],
        size: SIZE,
        depth_textures: Vec::new(),
    });
    wait(session.set_ring(SIZE, ring));
    assert_eq!(FrameDisposition::Rendered, render(&session));
    wait(session.detach());
}

// A ring's release forgets its textures: one that the host gave up starts
// fresh in a later ring, since the host may have destroyed it and made
// another with the same handle.
#[cfg(feature = "webgpu")]
#[test]
fn a_released_ring_forgets_the_textures_that_the_host_gave_up() {
    let Some((device, textures)) = webgpu_ring(1) else { return };
    let runtime = runtime();
    let map = map_of(&runtime, "#ff0000");
    let session = webgpu_session(&map, &device, &textures, AttachOptions::default());
    assert_eq!(FrameDisposition::Rendered, render(&session));
    session.acquire_frame().unwrap().expect("a frame").discard();
    assert!(eventually(|| {
        session
            .demand_frame(FrameDemand::new(DemandMode::Force, 2))
            .is_err_and(|error| error.kind == ErrorKind::InvalidState)
    }));

    wait(session.release_target());
    let ring = BorrowedRing::webgpu(mln_webgpu::BorrowedTextures {
        textures: textures.clone(),
        size: SIZE,
        depth_textures: Vec::new(),
    });
    wait(session.set_ring(SIZE, ring));
    assert_eq!(FrameDisposition::Rendered, render(&session));
    wait(session.detach());
}

#[cfg(all(feature = "gl", target_os = "linux"))]
mod gl {
    use std::ptr::NonNull;

    use glow::HasContext as _;
    use khronos_egl as egl;

    use super::*;

    type Egl = egl::DynamicInstance<egl::EGL1_4>;

    /// The host's context, of OpenGL ES 3 or of desktop OpenGL's
    /// compatibility profile, current on a pbuffer of the ring's size, which
    /// is also the surface a session presents to.
    struct Host {
        egl: Egl,
        display: egl::Display,
        config: egl::Config,
        context: egl::Context,
        pbuffer: egl::Surface,
        gl: glow::Context,
    }

    impl Host {
        /// A context of `api`: `egl::OPENGL_ES_API` or `egl::OPENGL_API`.
        fn new(api: egl::Enum) -> Option<Host> {
            const PLATFORM_SURFACELESS_MESA: egl::Enum = 0x31DD;
            // SAFETY: loading the system's EGL.
            let egl = unsafe { Egl::load_required() }.ok()?;
            // Mesa's surfaceless platform where there is one, as the library's
            // devices take.
            // SAFETY: the default native display is valid on every platform.
            let display = unsafe {
                egl.upcast::<egl::EGL1_5>()
                    .and_then(|egl| {
                        egl.get_platform_display(PLATFORM_SURFACELESS_MESA, egl::DEFAULT_DISPLAY, &[egl::ATTRIB_NONE])
                            .ok()
                    })
                    .or_else(|| egl.get_display(egl::DEFAULT_DISPLAY))
            }?;
            egl.initialize(display).ok()?;
            egl.bind_api(api).ok()?;
            let desktop = api == egl::OPENGL_API;
            let attributes = [
                egl::RENDERABLE_TYPE,
                if desktop { egl::OPENGL_BIT } else { egl::OPENGL_ES3_BIT },
                egl::SURFACE_TYPE,
                egl::PBUFFER_BIT,
                egl::RED_SIZE,
                8,
                egl::GREEN_SIZE,
                8,
                egl::BLUE_SIZE,
                8,
                egl::ALPHA_SIZE,
                8,
                egl::DEPTH_SIZE,
                24,
                egl::STENCIL_SIZE,
                8,
                egl::NONE,
            ];
            let config = egl.choose_first_config(display, &attributes).ok()??;
            let context_attributes: &[egl::Int] =
                if desktop { &[egl::NONE] } else { &[egl::CONTEXT_CLIENT_VERSION, 3, egl::NONE] };
            let context = egl.create_context(display, config, None, context_attributes).ok();
            egl.bind_api(egl::OPENGL_ES_API).ok()?;
            let context = context?;
            let size = [egl::WIDTH, SIZE.width as i32, egl::HEIGHT, SIZE.height as i32, egl::NONE];
            let pbuffer = egl.create_pbuffer_surface(display, config, &size).ok()?;
            egl.make_current(display, Some(pbuffer), Some(pbuffer), Some(context)).ok()?;
            // SAFETY: the context is current, and EGL gives its functions.
            let gl = unsafe {
                glow::Context::from_loader_function(|name| {
                    egl.get_proc_address(name).map_or(std::ptr::null(), |function| function as *const _)
                })
            };
            egl.make_current(display, None, None, None).ok()?;
            Some(Host { egl, display, config, context, pbuffer, gl })
        }

        fn current(&self) {
            self.egl.make_current(self.display, Some(self.pbuffer), Some(self.pbuffer), Some(self.context)).unwrap();
        }

        fn done(&self) {
            self.egl.make_current(self.display, None, None, None).unwrap();
        }

        /// A device whose share group is the host's.
        fn device(&self) -> Device {
            let pointer = |pointer: *mut std::ffi::c_void| NonNull::new(pointer).unwrap();
            // SAFETY: the host's live display, config and context, which
            // outlive the device's sessions.
            let device = unsafe {
                mln_gl::Device::from_raw(
                    pointer(self.display.as_ptr()),
                    pointer(self.config.as_ptr()),
                    pointer(self.context.as_ptr()),
                )
            };
            Device::Gl(device.unwrap())
        }

        /// A new RGBA8 texture of the ring's size.
        fn texture(&self) -> glow::Texture {
            self.current();
            // SAFETY: the host's context is current.
            let texture = unsafe {
                let texture = self.gl.create_texture().unwrap();
                self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                self.gl.tex_storage_2d(glow::TEXTURE_2D, 1, glow::RGBA8, SIZE.width as i32, SIZE.height as i32);
                self.gl.bind_texture(glow::TEXTURE_2D, None);
                self.gl.finish();
                texture
            };
            self.done();
            texture
        }

        /// The center pixel of `texture`, or of the pbuffer for `None`.
        fn read(&self, texture: Option<glow::Texture>) -> [u8; 4] {
            self.current();
            let mut pixel = [0u8; 4];
            // SAFETY: the host's context is current, and the texture its.
            unsafe {
                let framebuffer = texture.map(|texture| {
                    let framebuffer = self.gl.create_framebuffer().unwrap();
                    self.gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
                    self.gl.framebuffer_texture_2d(
                        glow::FRAMEBUFFER,
                        glow::COLOR_ATTACHMENT0,
                        glow::TEXTURE_2D,
                        Some(texture),
                        0,
                    );
                    framebuffer
                });
                self.gl.read_pixels(
                    SIZE.width as i32 / 2,
                    SIZE.height as i32 / 2,
                    1,
                    1,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelPackData::Slice(Some(&mut pixel)),
                );
                self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                if let Some(framebuffer) = framebuffer {
                    self.gl.delete_framebuffer(framebuffer);
                }
            }
            self.done();
            pixel
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            let _ = self.egl.destroy_surface(self.display, self.pbuffer);
            let _ = self.egl.destroy_context(self.display, self.context);
        }
    }

    // A view keeps its frame's fence through a dispose of the session, which
    // deletes it once the view ends.
    #[test]
    fn a_view_keeps_its_frames_fence_through_a_dispose() {
        let Some(host) = Host::new(egl::OPENGL_ES_API) else {
            eprintln!("skipped: no EGL context of OpenGL ES 3 with pbuffers");
            return;
        };
        let textures = [host.texture(), host.texture()];
        let ring = BorrowedRing::gl(mln_gl::BorrowedTextures {
            textures: textures.iter().map(|texture| texture.0.get()).collect(),
            size: SIZE,
            depth_textures: Vec::new(),
        });
        let runtime = runtime();
        let map = map_of(&runtime, "#ff0000");
        let target = RenderTarget::Borrowed { size: SIZE, ring, device: host.device() };
        let session = attach(&map, target, SyncKinds::of(&[SyncKind::OpenGlFence]));
        assert_eq!(FrameDisposition::Rendered, render(&session));
        let frame = session.acquire_frame().unwrap().expect("a frame");
        let sync = frame.producer_sync();
        assert_eq!(SyncKind::OpenGlFence, sync.kind);
        let is_fence = || {
            host.current();
            // SAFETY: the host's context is current; a name that is no
            // longer a fence's is no error.
            let fence = unsafe { host.gl.is_sync(glow::NativeFence(sync.handle as usize as *mut _)) };
            host.done();
            fence
        };
        let view = frame.view().unwrap();
        session.dispose().unwrap();
        drop(session);
        // The session ends once its thread let go of what it could.
        assert!(eventually(|| !frame.session_live()));
        assert!(is_fence(), "the dispose deleted the fence of a frame that a view holds");
        drop(view);
        assert!(eventually(|| !is_fence()));
        drop(frame);
    }

    #[test]
    fn a_ring_of_the_hosts_textures_hands_frames_back_with_fences() {
        a_ring_of_the_hosts_textures_hands_frames_back(egl::OPENGL_ES_API);
    }

    #[test]
    fn a_ring_of_a_desktop_hosts_textures_hands_frames_back_with_fences() {
        a_ring_of_the_hosts_textures_hands_frames_back(egl::OPENGL_API);
    }

    /// A ring of the textures of a host's context of `api`, and the
    /// context's surface, which a session presents to.
    fn a_ring_of_the_hosts_textures_hands_frames_back(api: egl::Enum) {
        let Some(host) = Host::new(api) else {
            eprintln!("skipped: no EGL context of {api:#x} with pbuffers");
            return;
        };
        let textures = [host.texture(), host.texture()];
        let ring = BorrowedRing::gl(mln_gl::BorrowedTextures {
            textures: textures.iter().map(|texture| texture.0.get()).collect(),
            size: SIZE,
            depth_textures: Vec::new(),
        });
        let device = host.device();
        let runtime = runtime();
        let map = map_of(&runtime, "#ff0000");
        let target = RenderTarget::Borrowed { size: SIZE, ring, device: device.clone() };
        let session = attach(&map, target, SyncKinds::of(&[SyncKind::OpenGlFence, SyncKind::CpuComplete]));
        assert_eq!(Some(SyncKind::OpenGlFence), session.capabilities().unwrap().producer_sync_kind);

        for (color, expected) in [("#ff0000", RED), ("#0000ff", BLUE)] {
            wait(map.set_style_json(style(color)));
            assert_eq!(FrameDisposition::Rendered, render(&session));
            let frame = session.acquire_frame().unwrap().expect("a frame");
            let sync = frame.producer_sync();
            assert_eq!(SyncKind::OpenGlFence, sync.kind);
            host.current();
            // SAFETY: the session's fence, in the host's share group, which
            // stays valid until the frame goes back.
            unsafe { host.gl.wait_sync(glow::NativeFence(sync.handle as usize as *mut _), 0, glow::TIMEOUT_IGNORED) };
            host.done();
            assert_eq!(expected, host.read(Some(textures[frame.slot()])));
            frame.release(ConsumerSync::of(GpuSync::of(SyncKind::CpuComplete))).unwrap();
        }
        wait(session.detach());

        // A session in the same share group presents to the host's surface.
        let target = RenderTarget::Surface {
            size: SIZE,
            // SAFETY: the host's pbuffer, which outlives the session.
            surface: mln_runtime::Surface::gl(unsafe {
                mln_gl::HostSurface::from_raw(NonNull::new(host.pbuffer.as_ptr()).unwrap())
            }),
            device: Some(device),
        };
        wait(map.set_style_json(style("#ff0000")));
        let session = attach(&map, target, SyncKinds::NONE);
        assert_eq!(FrameDisposition::Rendered, render(&session));
        wait(session.detach());
        assert_eq!(RED, host.read(None));
    }
}

#[cfg(all(feature = "metal", target_vendor = "apple", target_arch = "aarch64"))]
mod metal {
    use mln_mtl::objc2::rc::Retained;
    use mln_mtl::objc2::runtime::ProtocolObject;
    use mln_mtl::objc2_metal::{
        MTLDevice as _, MTLOrigin, MTLPixelFormat, MTLRegion, MTLSharedEvent, MTLSize, MTLStorageMode, MTLTexture,
        MTLTextureDescriptor, MTLTextureUsage,
    };

    use super::*;

    type Texture = Retained<ProtocolObject<dyn MTLTexture>>;

    /// The center pixel of `texture`, BGRA.
    fn read(texture: &Texture) -> [u8; 4] {
        let mut pixel = [0u8; 4];
        let region = MTLRegion {
            origin: MTLOrigin { x: SIZE.width as usize / 2, y: SIZE.height as usize / 2, z: 0 },
            size: MTLSize { width: 1, height: 1, depth: 1 },
        };
        // SAFETY: the pixel holds one BGRA8 pixel, and the texture is in
        // shared storage, which the frame's sync says the GPU is done with.
        unsafe {
            texture.getBytes_bytesPerRow_fromRegion_mipmapLevel(
                std::ptr::NonNull::new(pixel.as_mut_ptr().cast()).unwrap(),
                4,
                region,
                0,
            );
        };
        pixel
    }

    #[test]
    fn a_ring_of_the_hosts_textures_hands_frames_back_with_shared_events() {
        let Some(device) = device_of(BackendType::Metal) else { return };
        let Device::Metal(metal) = &device else { unreachable!() };
        let texture = || -> Texture {
            // SAFETY: a valid 2D format and a non-zero size.
            let descriptor = unsafe {
                MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                    MTLPixelFormat::BGRA8Unorm,
                    SIZE.width as usize,
                    SIZE.height as usize,
                    false,
                )
            };
            descriptor.setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
            descriptor.setStorageMode(MTLStorageMode::Shared);
            metal.device().newTextureWithDescriptor(&descriptor).unwrap()
        };
        let textures = vec![texture(), texture()];
        let ring = BorrowedRing::metal(mln_mtl::BorrowedTextures {
            textures: textures.clone(),
            size: SIZE,
            depth_textures: Vec::new(),
        });
        let runtime = runtime();
        let map = map_of(&runtime, "#ff0000");
        let target = RenderTarget::Borrowed { size: SIZE, ring, device: device.clone() };
        let session = attach(&map, target, SyncKinds::of(&[SyncKind::MetalSharedEvent, SyncKind::CpuComplete]));
        assert_eq!(Some(SyncKind::MetalSharedEvent), session.capabilities().unwrap().producer_sync_kind);

        for (color, expected) in [("#ff0000", [0, 0, 255, 255]), ("#0000ff", [255, 0, 0, 255])] {
            wait(map.set_style_json(style(color)));
            assert_eq!(FrameDisposition::Rendered, render(&session));
            let frame = session.acquire_frame().unwrap().expect("a frame");
            let sync = frame.producer_sync();
            assert_eq!(SyncKind::MetalSharedEvent, sync.kind);
            // SAFETY: the session's shared event, which stays valid until the
            // frame goes back.
            let event: Retained<ProtocolObject<dyn MTLSharedEvent>> =
                unsafe { Retained::retain(sync.handle as usize as *mut ProtocolObject<dyn MTLSharedEvent>) }.unwrap();
            assert!(eventually(|| event.signaledValue() >= sync.value));
            assert_eq!(expected, read(&textures[frame.slot()]));
            frame.release(ConsumerSync::of(GpuSync::of(SyncKind::CpuComplete))).unwrap();
        }
        wait(session.detach());
    }
}
