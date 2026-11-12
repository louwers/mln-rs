// Rings of images that a session allocates and shares with the host: which
// kind a ring picks, and its frames as a host reads them. A backend that
// this build or machine lacks, or a device that exports no kind, skips its
// cases: Mesa's software drivers export dmabufs only through the kernel's
// udmabuf module.

#![allow(unsafe_code)]

mod support;

use mln_map::MapMode;
use mln_runtime::{
    AcquiredFrame, AttachOptions, BackendType, ConsumerSync, DemandMode, Device, DeviceOptions, DmaBufFormat,
    ErrorKind, ExportKind, ExportKinds, ExportedImage, ExportedRing, FrameDemand, FrameDisposition, FrameTextureKind,
    GpuSync, LogicalExtent, Map, MapOptions, RenderSession, RenderTarget, Runtime, SyncKind, SyncKinds,
};
use mln_util::size::Size;
use support::{eventually, runtime, wait};

const SIZE: Size = Size { width: 64, height: 48 };
const DEPTH: usize = 3;
const ABGR8888: u32 = u32::from_le_bytes(*b"AB24");
const LINEAR: u64 = 0;
const EVERY_KIND: [ExportKind; 5] = ExportKind::BY_PREFERENCE;

fn style(color: &str) -> String {
    format!(
        r#"{{"version":8,"transition":{{"duration":0,"delay":0}},"sources":{{}},"layers":[{{"id":"bg","type":"background","paint":{{"background-color":"{color}"}}}}]}}"#
    )
}

fn red_map(runtime: &Runtime) -> Map {
    let options = MapOptions::new(mln_map::MapOptions::new().with_size(SIZE).with_map_mode(MapMode::Continuous));
    let (map, created) = runtime.create_map(options).unwrap();
    created.wait().unwrap();
    wait(map.set_style_json(style("#ff0000")));
    map
}

fn ring(kinds: &[ExportKind]) -> ExportedRing {
    ExportedRing {
        depth: DEPTH,
        kinds: ExportKinds::of(kinds),
        dma_buf_formats: vec![DmaBufFormat { fourcc: ABGR8888, modifier: LINEAR }],
        android_hardware_buffer_usage: 0,
    }
}

fn attach(
    map: &Map,
    ring: ExportedRing,
    device: Option<Device>,
    producer_sync_kinds: SyncKinds,
) -> Result<RenderSession, mln_runtime::Error> {
    let options = AttachOptions { producer_sync_kinds, ..AttachOptions::default() };
    let (session, attached) = map.attach(RenderTarget::Exported { size: SIZE, ring, device }, options)?;
    attached.wait()?;
    Ok(session)
}

/// Renders a forced frame and acquires it.
fn render_and_acquire(session: &RenderSession, token: u64) -> AcquiredFrame {
    session.demand_frame(FrameDemand::new(DemandMode::Force, token)).unwrap();
    let mut results = Vec::new();
    assert!(eventually(|| {
        results.extend(session.drain_frame_results().unwrap());
        results.iter().any(|result| result.token == Some(token))
    }));
    let result = results.iter().find(|result| result.token == Some(token)).unwrap();
    assert_eq!(FrameDisposition::Rendered, result.disposition);
    session.acquire_frame().unwrap().expect("the rendered frame waits for the host")
}

/// Detaches `session`, so that its map takes another.
#[expect(clippy::needless_pass_by_value, reason = "a detached session is done with")]
fn detach(session: RenderSession) {
    wait(session.detach());
}

/// A device of `backend`, or `None` where this build or machine has none.
fn device_of(backend: BackendType) -> Option<Device> {
    match Device::with_backend(backend, DeviceOptions::default()) {
        Ok(device) => Some(device),
        Err(error) if error.kind == ErrorKind::Unsupported => {
            eprintln!("skipped: no {backend:?} device: {error}");
            None
        }
        Err(error) => panic!("a {backend:?} device: {error}"),
    }
}

const BACKENDS: [BackendType; 4] = [BackendType::Vulkan, BackendType::OpenGL, BackendType::Metal, BackendType::WebGPU];

fn exported_image(frame: &AcquiredFrame) -> (Size, ExportedImage) {
    let texture = frame.texture().expect("an exported frame has its image");
    match &texture.kind {
        FrameTextureKind::Exported(image) => (texture.size, image.clone()),
        other => panic!("an exported frame has no {other:?}"),
    }
}

#[test]
fn a_ring_takes_the_first_kind_that_its_device_exports() {
    for backend in BACKENDS {
        let Some(device) = device_of(backend) else { continue };
        let exported = device.export_kinds();
        let runtime = runtime();
        let map = red_map(&runtime);
        let Some(best) = exported.iter().next() else {
            let error = attach(&map, ring(&EVERY_KIND), Some(device), SyncKinds::NONE).unwrap_err();
            assert_eq!(ErrorKind::Unsupported, error.kind, "{backend:?}");
            continue;
        };
        let session = attach(&map, ring(&EVERY_KIND), Some(device.clone()), SyncKinds::NONE).unwrap();
        let capabilities = session.capabilities().unwrap();
        assert_eq!(Some(best), capabilities.export_kind, "{backend:?}");
        assert_eq!(DEPTH as u32, capabilities.texture_ring_depth);
        assert!(capabilities.frame_acquisition && !capabilities.readback && !capabilities.presentation);
        detach(session);
        // Each kind alone, and the host's order of them doesn't matter.
        for kind in exported.iter() {
            let session = attach(&map, ring(&[kind]), Some(device.clone()), SyncKinds::NONE).unwrap();
            assert_eq!(Some(kind), session.capabilities().unwrap().export_kind, "{backend:?}");
            detach(session);
        }
        let mut reversed = EVERY_KIND;
        reversed.reverse();
        let session = attach(&map, ring(&reversed), Some(device.clone()), SyncKinds::NONE).unwrap();
        assert_eq!(Some(best), session.capabilities().unwrap().export_kind, "{backend:?}");
        detach(session);
        // No kind that the device exports.
        let others: Vec<ExportKind> = EVERY_KIND.into_iter().filter(|kind| !exported.contains(*kind)).collect();
        let error = attach(&map, ring(&others), Some(device), SyncKinds::NONE).unwrap_err();
        assert_eq!(ErrorKind::Unsupported, error.kind, "{backend:?}");
    }
}

#[test]
fn a_ring_without_a_device_renders_on_one_that_exports_its_kind() {
    let runtime = runtime();
    let map = red_map(&runtime);
    let exporting = BACKENDS
        .into_iter()
        .filter_map(device_of)
        .find_map(|device| device.export_kinds().iter().next().map(|kind| (device.backend(), kind)));
    if let Some((_, kind)) = exporting {
        let session = attach(&map, ring(&[kind]), None, SyncKinds::NONE).unwrap();
        assert_eq!(Some(kind), session.capabilities().unwrap().export_kind);
        let frame = render_and_acquire(&session, 1);
        assert_eq!(SIZE, exported_image(&frame).0);
    } else {
        let error = attach(&map, ring(&EVERY_KIND), None, SyncKinds::NONE).unwrap_err();
        assert_eq!(ErrorKind::Unsupported, error.kind);
    }
}

#[test]
fn an_exported_ring_rejects_what_it_cannot_take() {
    let runtime = runtime();
    let map = red_map(&runtime);
    let mut empty = ring(&EVERY_KIND);
    empty.depth = 0;
    assert_eq!(ErrorKind::InvalidArgument, attach(&map, empty, None, SyncKinds::NONE).unwrap_err().kind);
    assert_eq!(ErrorKind::InvalidArgument, attach(&map, ring(&[]), None, SyncKinds::NONE).unwrap_err().kind);
    let Some((device, kind)) = BACKENDS
        .into_iter()
        .filter_map(device_of)
        .find_map(|device| device.export_kinds().iter().next().map(|kind| (device, kind)))
    else {
        eprintln!("skipped: no device exports a ring");
        return;
    };
    let session = attach(&map, ring(&[kind]), Some(device), SyncKinds::NONE).unwrap();
    assert_eq!(ErrorKind::Unsupported, session.release_target().unwrap_err().kind);
}

/// Sessions of red maps whose devices export `kind`, one for each backend
/// that does.
fn sessions_exporting(runtime: &Runtime, kind: ExportKind, producer: SyncKinds) -> Vec<(BackendType, RenderSession)> {
    BACKENDS
        .into_iter()
        .filter_map(device_of)
        .filter(|device| {
            let exports = device.export_kinds().contains(kind);
            if !exports {
                eprintln!("skipped: the {:?} device exports no {kind:?}", device.backend());
            }
            exports
        })
        .map(|device| (device.backend(), attach(&red_map(runtime), ring(&[kind]), Some(device), producer).unwrap()))
        .collect()
}

/// Resizes the session's ring to `size` logical pixels.
fn resize(session: &RenderSession, size: Size) {
    let scale_factor = session.snapshot().unwrap().extent.scale_factor;
    wait(session.resize(LogicalExtent { size, scale_factor }));
}

#[test]
fn a_held_frame_keeps_its_image_across_a_resize() {
    let runtime = runtime();
    let swapped = Size::new(SIZE.height, SIZE.width);
    for kind in EVERY_KIND {
        for (backend, session) in sessions_exporting(&runtime, kind, SyncKinds::NONE) {
            resize(&session, SIZE);
            let held = render_and_acquire(&session, 1);
            let (size, image) = exported_image(&held);
            assert_eq!(SIZE, size, "{backend:?} {kind:?}");
            resize(&session, swapped);
            // The other slots render at the new size while the host holds
            // the first.
            for token in 2..2 + DEPTH as u64 {
                let next = render_and_acquire(&session, token);
                assert_ne!(held.slot(), next.slot());
                assert_eq!(swapped, exported_image(&next).0, "{backend:?} {kind:?}");
                drop(next);
            }
            assert_eq!((SIZE, image), exported_image(&held), "{backend:?} {kind:?}");
            #[cfg(target_os = "linux")]
            if kind == ExportKind::DmaBuf {
                assert_eq!(linux::RED, linux::dma_buf_center(&held), "{backend:?}");
            }
            // Dropping the frame gives it back: its slot renders at the new
            // size.
            let slot = held.slot();
            drop(held);
            let mut reached = false;
            for token in 10..10 + 2 * DEPTH as u64 {
                let next = render_and_acquire(&session, token);
                assert_eq!(swapped, exported_image(&next).0);
                reached |= next.slot() == slot;
            }
            assert!(reached, "{backend:?} {kind:?}: the given back slot renders again");
        }
    }
}

#[test]
fn dropped_and_released_frames_free_their_slots() {
    let runtime = runtime();
    for kind in EVERY_KIND {
        for (backend, session) in sessions_exporting(&runtime, kind, SyncKinds::NONE) {
            let capabilities = session.capabilities().unwrap();
            assert_eq!(Some(SyncKind::CpuComplete), capabilities.producer_sync_kind, "{backend:?} {kind:?}");
            assert!(capabilities.consumer_sync_kinds.contains(SyncKind::CpuComplete));
            for token in 1..=3 * DEPTH as u64 {
                let frame = render_and_acquire(&session, token);
                assert_eq!(GpuSync::of(SyncKind::CpuComplete), frame.producer_sync());
                if token % 2 == 0 {
                    drop(frame);
                } else {
                    frame.release(ConsumerSync::of(GpuSync::of(SyncKind::CpuComplete))).unwrap();
                }
            }
        }
    }
}

/// A resize to the size the ring has keeps its images, so a slot whose
/// frame the host discarded stays out of use: the host may still read it.
#[test]
fn a_resize_to_the_same_size_keeps_a_discarded_slot_out_of_use() {
    let runtime = runtime();
    for kind in EVERY_KIND {
        for (backend, session) in sessions_exporting(&runtime, kind, SyncKinds::NONE) {
            let discarded = render_and_acquire(&session, 1);
            let slot = discarded.slot();
            discarded.discard();
            resize(&session, SIZE);
            for token in 2..2 + 2 * DEPTH as u64 {
                let frame = render_and_acquire(&session, token);
                assert_ne!(slot, frame.slot(), "{backend:?} {kind:?}: the discarded slot renders again");
            }
        }
    }
}

/// Frames that the host held across a resize are of the old images: giving
/// them up quarantines none of the new ones.
#[test]
fn discarding_frames_held_across_a_resize_frees_their_slots() {
    let runtime = runtime();
    let swapped = Size::new(SIZE.height, SIZE.width);
    for kind in EVERY_KIND {
        for (backend, session) in sessions_exporting(&runtime, kind, SyncKinds::NONE) {
            let held: Vec<AcquiredFrame> =
                (1..=DEPTH as u64).map(|token| render_and_acquire(&session, token)).collect();
            resize(&session, swapped);
            for frame in held {
                frame.discard();
            }
            // The host keeps each new frame, so each renders into another
            // slot.
            let kept: Vec<AcquiredFrame> =
                (10..10 + DEPTH as u64).map(|token| render_and_acquire(&session, token)).collect();
            let mut slots: Vec<usize> = kept.iter().map(AcquiredFrame::slot).collect();
            assert!(kept.iter().all(|frame| exported_image(frame).0 == swapped), "{backend:?} {kind:?}");
            slots.sort_unstable();
            assert_eq!((0..DEPTH).collect::<Vec<_>>(), slots, "{backend:?} {kind:?}: every slot renders again");
        }
    }
}

/// A ring that can't make images of a new size fails its session, rather
/// than drawing the resized map into the images of the old size. Mesa's
/// software drivers share dmabufs through udmabuf, which takes 64 MiB at
/// most by default, so a large resize fails there; elsewhere it may not.
#[test]
fn a_resize_that_makes_no_images_fails_the_session() {
    let runtime = runtime();
    let huge = Size::new(6000, 6000);
    for (backend, session) in sessions_exporting(&runtime, ExportKind::DmaBuf, SyncKinds::NONE) {
        if backend != BackendType::OpenGL {
            continue;
        }
        let scale_factor = session.snapshot().unwrap().extent.scale_factor;
        let Err(error) = session.resize(LogicalExtent { size: huge, scale_factor }).unwrap().wait() else {
            eprintln!("skipped: the {backend:?} ring makes buffers of {huge:?}");
            continue;
        };
        eprintln!("the resize failed: {error}");
        let snapshot = session.snapshot().unwrap();
        assert_eq!(mln_runtime::RenderSessionState::Failed, snapshot.state, "{backend:?}");
        assert!(snapshot.failure.is_some());
    }
}

/// A wrap of the host's device offers sync files, D3D12 fences and exports
/// only through the extensions that the host lists, since Vulkan can't say
/// which it enabled.
#[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
#[test]
fn a_wrapped_device_exports_through_the_extensions_that_the_host_lists() {
    let Some(Device::Vulkan(library)) = device_of(BackendType::Vulkan) else { return };
    let wrap = |enabled_extensions: Vec<std::ffi::CString>| {
        let raw = library.raw();
        let host = mln_vulkan::HostDevice {
            get_instance_proc_addr: Some(raw.get_instance_proc_addr),
            get_device_proc_addr: Some(raw.get_device_proc_addr),
            instance: raw.instance,
            physical_device: raw.physical_device,
            device: raw.device,
            queue_family_index: raw.queue_family_index,
            queue: raw.queue,
            queue_lock: None,
            enabled_extensions,
        };
        // SAFETY: the library device's live handles, which outlive the wrap.
        unsafe { mln_vulkan::Device::from_raw(host) }.unwrap()
    };
    let unlisted = wrap(Vec::new());
    assert_eq!(ExportKinds::NONE, unlisted.export_kinds());
    let (producer, consumer) = unlisted.sync_kinds();
    for kind in [SyncKind::SyncFd, SyncKind::D3d12Fence] {
        assert!(!producer.contains(kind) && !consumer.contains(kind), "{kind:?}");
    }
    let listed = wrap(library.enabled_extensions().to_vec());
    assert_eq!(library.export_kinds(), listed.export_kinds());
    assert_eq!(library.sync_kinds(), listed.sync_kinds());
}

#[cfg(target_os = "linux")]
mod linux {
    use std::os::fd::{AsRawFd as _, BorrowedFd};

    use super::*;

    pub(super) const RED: [u8; 4] = [255, 0, 0, 255];

    /// The center pixel of a frame's linear ABGR8888 dmabuf, which it maps.
    pub(super) fn dma_buf_center(frame: &AcquiredFrame) -> [u8; 4] {
        let (size, image) = exported_image(frame);
        let ExportedImage::DmaBuf { fourcc, modifier, planes } = image else { panic!("the frame has no dmabuf") };
        assert_eq!((ABGR8888, LINEAR, 1), (fourcc, modifier, planes.len()));
        let plane = planes[0];
        assert!(plane.stride >= size.width * 4);
        let length = plane.offset as usize + plane.stride as usize * size.height as usize;
        // SAFETY: a read-only shared mapping of the session's live dmabuf,
        // which the frame keeps, unmapped below.
        unsafe {
            let data = libc::mmap(std::ptr::null_mut(), length, libc::PROT_READ, libc::MAP_SHARED, plane.fd, 0);
            assert_ne!(libc::MAP_FAILED, data, "the dmabuf maps");
            let offset = plane.offset as usize
                + (size.height / 2) as usize * plane.stride as usize
                + (size.width / 2) as usize * 4;
            let mut pixel = [0u8; 4];
            std::ptr::copy_nonoverlapping(data.cast::<u8>().add(offset), pixel.as_mut_ptr(), 4);
            libc::munmap(data, length);
            pixel
        }
    }

    /// Waits for a frame's producer sync file, if it has one.
    fn wait_for_producer(frame: &AcquiredFrame) {
        let sync = frame.producer_sync();
        if sync.kind != SyncKind::SyncFd || (sync.handle as i64) < 0 {
            return;
        }
        let mut waiting = libc::pollfd { fd: sync.handle as i32, events: libc::POLLIN, revents: 0 };
        // SAFETY: a poll of the frame's live sync file.
        assert_eq!(1, unsafe { libc::poll(&raw mut waiting, 1, 10_000) }, "the frame's sync file signals");
    }

    #[test]
    fn a_dma_buf_ring_renders_into_buffers_that_the_host_maps() {
        let runtime = runtime();
        let producer = SyncKinds::of(&[SyncKind::SyncFd, SyncKind::CpuComplete]);
        for (backend, session) in sessions_exporting(&runtime, ExportKind::DmaBuf, producer) {
            let capabilities = session.capabilities().unwrap();
            assert_eq!(Some(ExportKind::DmaBuf), capabilities.export_kind);
            for token in 1..=2 * DEPTH as u64 {
                let frame = render_and_acquire(&session, token);
                assert_eq!(capabilities.producer_sync_kind, Some(frame.producer_sync().kind));
                wait_for_producer(&frame);
                assert_eq!(RED, dma_buf_center(&frame), "{backend:?}");
                // A host that takes sync files hands back one of its own:
                // here a copy of the frame's, which has signaled.
                let sync = frame.producer_sync();
                if frame.accepts(SyncKind::SyncFd) && sync.kind == SyncKind::SyncFd && (sync.handle as i64) >= 0 {
                    // SAFETY: the frame's live sync file, which the copy
                    // outlives.
                    let copy = unsafe { BorrowedFd::borrow_raw(sync.handle as i32) }.try_clone_to_owned().unwrap();
                    let handle = copy.as_raw_fd() as u64;
                    let consumer = ConsumerSync {
                        sync: GpuSync { kind: SyncKind::SyncFd, handle, value: 0 },
                        sync_file: Some(copy),
                    };
                    frame.release(consumer).map_err(|(_, _, error)| error).unwrap();
                } else {
                    frame.release(ConsumerSync::of(GpuSync::of(SyncKind::CpuComplete))).unwrap();
                }
            }
        }
    }

    /// The pixel of a frame's linear ABGR8888 dmabuf at `x`, `y` from its
    /// first row.
    fn dma_buf_pixel(frame: &AcquiredFrame, x: u32, y: u32) -> [u8; 4] {
        let (size, image) = exported_image(frame);
        let ExportedImage::DmaBuf { planes, .. } = image else { panic!("the frame has no dmabuf") };
        let plane = planes[0];
        let length = plane.offset as usize + plane.stride as usize * size.height as usize;
        // SAFETY: as in dma_buf_center.
        unsafe {
            let data = libc::mmap(std::ptr::null_mut(), length, libc::PROT_READ, libc::MAP_SHARED, plane.fd, 0);
            assert_ne!(libc::MAP_FAILED, data, "the dmabuf maps");
            let offset = plane.offset as usize + y as usize * plane.stride as usize + x as usize * 4;
            let mut pixel = [0u8; 4];
            std::ptr::copy_nonoverlapping(data.cast::<u8>().add(offset), pixel.as_mut_ptr(), 4);
            libc::munmap(data, length);
            pixel
        }
    }

    /// A dmabuf's readers, such as a compositor, take its first row as the
    /// top of the image: the map's north.
    #[test]
    fn a_dma_buf_starts_at_the_top_of_the_map() {
        let runtime = runtime();
        let north = r##"{"version":8,"transition":{"duration":0,"delay":0},"sources":{"north":{"type":"geojson","data":{"type":"Polygon","coordinates":[[[-180,0],[180,0],[180,85],[-180,85],[-180,0]]]}}},"layers":[{"id":"bg","type":"background","paint":{"background-color":"#ff0000"}},{"id":"north","type":"fill","source":"north","paint":{"fill-color":"#0000ff","fill-antialias":false}}]}"##;
        for (backend, session) in sessions_exporting(&runtime, ExportKind::DmaBuf, SyncKinds::NONE) {
            let map = session_map(&runtime, north);
            drop(session);
            let device = device_of(backend).unwrap();
            let session = attach(&map, ring(&[ExportKind::DmaBuf]), Some(device), SyncKinds::NONE).unwrap();
            let mut frame = render_and_acquire(&session, 1);
            // The fill loads after the first frame.
            for token in 2..50 {
                if dma_buf_pixel(&frame, SIZE.width / 2, SIZE.height / 4) != RED {
                    break;
                }
                drop(frame);
                std::thread::sleep(std::time::Duration::from_millis(20));
                frame = render_and_acquire(&session, token);
            }
            assert_eq!([0, 0, 255, 255], dma_buf_pixel(&frame, SIZE.width / 2, SIZE.height / 4), "{backend:?}");
            assert_eq!(RED, dma_buf_pixel(&frame, SIZE.width / 2, SIZE.height * 3 / 4), "{backend:?}");
        }
    }

    fn session_map(runtime: &Runtime, style: &str) -> Map {
        let options = MapOptions::new(mln_map::MapOptions::new().with_size(SIZE).with_map_mode(MapMode::Continuous));
        let (map, created) = runtime.create_map(options).unwrap();
        created.wait().unwrap();
        wait(map.set_style_json(style.to_owned()));
        map
    }

    #[cfg(feature = "vulkan")]
    #[test]
    fn an_opaque_fd_imports_into_another_device_of_the_driver() {
        use mln_vulkan::ash::{self, vk};

        let runtime = runtime();
        let map = red_map(&runtime);
        let Some(Device::Vulkan(library)) = device_of(BackendType::Vulkan) else { return };
        if !library.export_kinds().contains(ExportKind::OpaqueFd) {
            eprintln!("skipped: the Vulkan device exports no opaque file descriptor");
            return;
        }
        let raw = library.raw();
        let session =
            attach(&map, ring(&[ExportKind::OpaqueFd]), Some(Device::Vulkan(library.clone())), SyncKinds::NONE)
                .unwrap();
        let frame = render_and_acquire(&session, 1);
        let (size, image) = exported_image(&frame);
        let ExportedImage::OpaqueFd { fd, allocation_size, dedicated, format } = image else {
            panic!("the frame has no opaque file descriptor")
        };
        assert_eq!(vk::Format::R8G8B8A8_UNORM.as_raw() as u32, format);

        // SAFETY: a second device on the library device's GPU, made, used
        // and destroyed here, which imports a copy of the frame's memory
        // while the frame keeps it.
        unsafe {
            let entry =
                ash::Entry::from_static_fn(ash::StaticFn { get_instance_proc_addr: raw.get_instance_proc_addr });
            let instance = ash::Instance::load(entry.static_fn(), raw.instance);
            let family = raw.queue_family_index;
            let priorities = [1.0];
            let queues =
                [vk::DeviceQueueCreateInfo::default().queue_family_index(family).queue_priorities(&priorities)];
            let extensions = [ash::khr::external_memory_fd::NAME.as_ptr()];
            let device = instance
                .create_device(
                    raw.physical_device,
                    &vk::DeviceCreateInfo::default().queue_create_infos(&queues).enabled_extension_names(&extensions),
                    None,
                )
                .unwrap();
            let queue = device.get_device_queue(family, 0);
            let mut external =
                vk::ExternalMemoryImageCreateInfo::default().handle_types(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
            let image = device
                .create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(vk::Format::R8G8B8A8_UNORM)
                        .extent(vk::Extent3D { width: size.width, height: size.height, depth: 1 })
                        .mip_levels(1)
                        .array_layers(1)
                        .samples(vk::SampleCountFlags::TYPE_1)
                        .tiling(vk::ImageTiling::OPTIMAL)
                        .usage(
                            vk::ImageUsageFlags::COLOR_ATTACHMENT
                                | vk::ImageUsageFlags::SAMPLED
                                | vk::ImageUsageFlags::TRANSFER_SRC,
                        )
                        .sharing_mode(vk::SharingMode::EXCLUSIVE)
                        .initial_layout(vk::ImageLayout::UNDEFINED)
                        .push_next(&mut external),
                    None,
                )
                .unwrap();
            let requirements = device.get_image_memory_requirements(image);
            assert!(requirements.size <= allocation_size);
            let types = instance.get_physical_device_memory_properties(raw.physical_device);
            let pick = |wanted: vk::MemoryPropertyFlags, bits: u32| {
                (0..types.memory_type_count)
                    .find(|&index| {
                        bits & (1 << index) != 0 && types.memory_types[index as usize].property_flags.contains(wanted)
                    })
                    .unwrap()
            };
            // An import takes the descriptor, so it takes a copy.
            let copy = BorrowedFd::borrow_raw(fd).try_clone_to_owned().unwrap();
            let mut import = vk::ImportMemoryFdInfoKHR::default()
                .handle_type(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD)
                .fd(copy.as_raw_fd());
            let mut own = vk::MemoryDedicatedAllocateInfo::default().image(image);
            let mut allocate = vk::MemoryAllocateInfo::default()
                .allocation_size(allocation_size)
                .memory_type_index(pick(vk::MemoryPropertyFlags::empty(), requirements.memory_type_bits))
                .push_next(&mut import);
            if dedicated {
                allocate = allocate.push_next(&mut own);
            }
            let memory = device.allocate_memory(&allocate, None).unwrap();
            // Vulkan owns the descriptor once the import succeeds.
            std::mem::forget(copy);
            device.bind_image_memory(image, memory, 0).unwrap();

            let bytes = u64::from(size.width) * u64::from(size.height) * 4;
            let buffer = device
                .create_buffer(
                    &vk::BufferCreateInfo::default().size(bytes).usage(vk::BufferUsageFlags::TRANSFER_DST),
                    None,
                )
                .unwrap();
            let buffer_requirements = device.get_buffer_memory_requirements(buffer);
            let buffer_memory = device
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default().allocation_size(buffer_requirements.size).memory_type_index(
                        pick(
                            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
                            buffer_requirements.memory_type_bits,
                        ),
                    ),
                    None,
                )
                .unwrap();
            device.bind_buffer_memory(buffer, buffer_memory, 0).unwrap();

            let pool = device
                .create_command_pool(&vk::CommandPoolCreateInfo::default().queue_family_index(family), None)
                .unwrap();
            let commands = device
                .allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default().command_pool(pool).command_buffer_count(1),
                )
                .unwrap()[0];
            device.begin_command_buffer(commands, &vk::CommandBufferBeginInfo::default()).unwrap();
            let color = vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            };
            // The frame left the image in GENERAL, released to the external
            // queue family: acquire it here.
            let acquire = vk::ImageMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::empty())
                .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
                .old_layout(vk::ImageLayout::GENERAL)
                .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_EXTERNAL)
                .dst_queue_family_index(family)
                .image(image)
                .subresource_range(color);
            device.cmd_pipeline_barrier(
                commands,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[acquire],
            );
            let region = vk::BufferImageCopy::default()
                .image_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .image_extent(vk::Extent3D { width: size.width, height: size.height, depth: 1 });
            device.cmd_copy_image_to_buffer(commands, image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, buffer, &[region]);
            device.end_command_buffer(commands).unwrap();
            let submits = [vk::SubmitInfo::default().command_buffers(std::slice::from_ref(&commands))];
            device.queue_submit(queue, &submits, vk::Fence::null()).unwrap();
            device.queue_wait_idle(queue).unwrap();

            let data = device.map_memory(buffer_memory, 0, bytes, vk::MemoryMapFlags::empty()).unwrap().cast::<u8>();
            let offset = ((size.height / 2 * size.width + size.width / 2) * 4) as usize;
            let mut pixel = [0u8; 4];
            std::ptr::copy_nonoverlapping(data.add(offset), pixel.as_mut_ptr(), 4);
            device.unmap_memory(buffer_memory);
            assert_eq!(RED, pixel);

            device.destroy_command_pool(pool, None);
            device.destroy_buffer(buffer, None);
            device.free_memory(buffer_memory, None);
            device.destroy_image(image, None);
            device.free_memory(memory, None);
            device.destroy_device(None);
        }
        drop(frame);
    }
}

#[cfg(all(feature = "vulkan", windows))]
mod win32 {
    use mln_vulkan::ash::{self, vk};
    use windows::Win32::Foundation::{CloseHandle, GENERIC_ALL, HANDLE, HMODULE, LUID};
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
    use windows::Win32::Graphics::Direct3D11::{
        D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_FLAG, D3D11_FENCE_FLAG_SHARED, D3D11_MAP_READ,
        D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, D3D11CreateDevice,
        ID3D11Device, ID3D11Device5, ID3D11DeviceContext, ID3D11DeviceContext4, ID3D11Fence, ID3D11Texture2D,
    };
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter, IDXGIFactory4};
    use windows::core::{Interface as _, PCWSTR};

    use super::*;

    /// Red, as a B8G8R8A8 texture holds it.
    const BGRA_RED: [u8; 4] = [0, 0, 255, 255];

    /// The Vulkan device of the library's, if it exports D3D11 textures.
    fn dxgi_device() -> Option<mln_vulkan::Device> {
        let Some(Device::Vulkan(library)) = device_of(BackendType::Vulkan) else { return None };
        if !library.export_kinds().contains(ExportKind::DxgiSharedHandle) {
            eprintln!("skipped: the Vulkan device shares no D3D11 textures");
            return None;
        }
        Some(library)
    }

    /// A D3D11 device on the adapter of `library`'s GPU, as a host makes one
    /// to open the ring's textures.
    fn host_device(library: &mln_vulkan::Device) -> (ID3D11Device5, ID3D11DeviceContext4) {
        let raw = library.raw();
        let mut ids = vk::PhysicalDeviceIDProperties::default();
        // SAFETY: plain queries of the library device's live GPU, and D3D11
        // calls whose out pointers they fill.
        unsafe {
            let entry =
                ash::Entry::from_static_fn(ash::StaticFn { get_instance_proc_addr: raw.get_instance_proc_addr });
            let instance = ash::Instance::load(entry.static_fn(), raw.instance);
            let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut ids);
            instance.get_physical_device_properties2(raw.physical_device, &mut properties);
            assert_eq!(vk::TRUE, ids.device_luid_valid);
            let [l0, l1, l2, l3, h0, h1, h2, h3] = ids.device_luid;
            let luid =
                LUID { LowPart: u32::from_ne_bytes([l0, l1, l2, l3]), HighPart: i32::from_ne_bytes([h0, h1, h2, h3]) };
            let factory: IDXGIFactory4 = CreateDXGIFactory1().unwrap();
            let adapter: IDXGIAdapter = factory.EnumAdapterByLuid(luid).unwrap();
            let (mut device, mut context): (Option<ID3D11Device>, Option<ID3D11DeviceContext>) = (None, None);
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_FLAG(0),
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .unwrap();
            (device.unwrap().cast().unwrap(), context.unwrap().cast().unwrap())
        }
    }

    /// The center pixel of the frame's texture, which the host's device
    /// opens and copies to a texture that the CPU maps.
    fn texture_pixel(device: &ID3D11Device5, context: &ID3D11DeviceContext4, frame: &AcquiredFrame) -> [u8; 4] {
        let (size, image) = exported_image(frame);
        let ExportedImage::DxgiSharedTexture { handle, format } = image else {
            panic!("the frame has no DXGI shared texture")
        };
        assert_eq!(DXGI_FORMAT_B8G8R8A8_UNORM.0 as u32, format);
        // SAFETY: the frame's live NT handle, which the session keeps, and
        // D3D11 calls of the host's live device.
        unsafe {
            let texture: ID3D11Texture2D = device.OpenSharedResource1(HANDLE(handle as usize as *mut _)).unwrap();
            let mut description = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut description);
            assert_eq!((size.width, size.height), (description.Width, description.Height));
            assert_eq!(DXGI_FORMAT_B8G8R8A8_UNORM, description.Format);
            let staging_description = D3D11_TEXTURE2D_DESC {
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
                ..description
            };
            let mut staging: Option<ID3D11Texture2D> = None;
            device.CreateTexture2D(&staging_description, None, Some(&mut staging)).unwrap();
            let staging = staging.unwrap();
            context.CopyResource(&staging, &texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).unwrap();
            let (x, y) = (size.width as usize / 2, size.height as usize / 2);
            let mut pixel = [0u8; 4];
            std::ptr::copy_nonoverlapping(
                mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize + x * 4),
                pixel.as_mut_ptr(),
                4,
            );
            context.Unmap(&staging, 0);
            pixel
        }
    }

    #[test]
    fn a_dxgi_ring_renders_into_d3d11_textures_that_the_host_opens() {
        let Some(library) = dxgi_device() else { return };
        let (device, context) = host_device(&library);
        let runtime = runtime();
        let map = red_map(&runtime);
        let session =
            attach(&map, ring(&[ExportKind::DxgiSharedHandle]), Some(Device::Vulkan(library)), SyncKinds::NONE)
                .unwrap();
        let capabilities = session.capabilities().unwrap();
        assert_eq!(Some(ExportKind::DxgiSharedHandle), capabilities.export_kind);
        assert_eq!(Some(SyncKind::CpuComplete), capabilities.producer_sync_kind);
        for token in 1..=2 * DEPTH as u64 {
            let frame = render_and_acquire(&session, token);
            assert_eq!(GpuSync::of(SyncKind::CpuComplete), frame.producer_sync());
            assert_eq!(BGRA_RED, texture_pixel(&device, &context, &frame));
            frame.release(ConsumerSync::of(GpuSync::of(SyncKind::CpuComplete))).unwrap();
        }
    }

    /// Each frame hands over with the session's D3D12 fence, which the host
    /// waits for on its D3D11 device, and comes back with the host's, which
    /// the next frame into the texture waits for.
    #[test]
    fn a_dxgi_ring_hands_frames_over_with_d3d12_fences() {
        let Some(library) = dxgi_device() else { return };
        let (producer, consumer) = mln_vulkan::exported_target::sync_kinds(&library);
        if !producer.contains(SyncKind::D3d12Fence) || !consumer.contains(SyncKind::D3d12Fence) {
            eprintln!("skipped: the Vulkan device shares no D3D12 fences");
            return;
        }
        let (device, context) = host_device(&library);
        let runtime = runtime();
        let map = red_map(&runtime);
        let fences = SyncKinds::of(&[SyncKind::D3d12Fence]);
        let session =
            attach(&map, ring(&[ExportKind::DxgiSharedHandle]), Some(Device::Vulkan(library)), fences).unwrap();
        let capabilities = session.capabilities().unwrap();
        assert_eq!(Some(SyncKind::D3d12Fence), capabilities.producer_sync_kind);
        assert!(capabilities.consumer_sync_kinds.contains(SyncKind::D3d12Fence));
        // SAFETY: D3D11 calls of the host's live device, whose fence handle
        // the test closes once the session is gone.
        let (reads, reads_handle): (ID3D11Fence, HANDLE) = unsafe {
            let mut fence: Option<ID3D11Fence> = None;
            device.CreateFence(0, D3D11_FENCE_FLAG_SHARED, &mut fence).unwrap();
            let fence = fence.unwrap();
            let handle = fence.CreateSharedHandle(None, GENERIC_ALL.0, PCWSTR::null()).unwrap();
            (fence, handle)
        };
        let mut last_value = 0;
        for token in 1..=2 * DEPTH as u64 {
            let frame = render_and_acquire(&session, token);
            let sync = frame.producer_sync();
            assert_eq!(SyncKind::D3d12Fence, sync.kind);
            assert_ne!(0, sync.handle);
            assert!(sync.value > last_value, "the session's fence counts up");
            last_value = sync.value;
            // SAFETY: the session's live fence handle, and D3D11 calls of the
            // host's live device.
            unsafe {
                let mut frames: Option<ID3D11Fence> = None;
                device.OpenSharedFence(HANDLE(sync.handle as usize as *mut _), &mut frames).unwrap();
                context.Wait(&frames.unwrap(), sync.value).unwrap();
            }
            assert_eq!(BGRA_RED, texture_pixel(&device, &context, &frame));
            // SAFETY: as above.
            unsafe {
                context.Signal(&reads, token).unwrap();
                context.Flush();
            }
            let done = GpuSync { kind: SyncKind::D3d12Fence, handle: reads_handle.0 as usize as u64, value: token };
            frame.release(ConsumerSync::of(done)).unwrap();
        }
        detach(session);
        // SAFETY: the test's handle, which the detached session let go of.
        unsafe { CloseHandle(reads_handle).unwrap() };
    }
}

#[cfg(all(feature = "metal", target_vendor = "apple"))]
mod apple {
    use std::ffi::c_void;

    use super::*;

    #[link(name = "IOSurface", kind = "framework")]
    unsafe extern "C" {
        fn IOSurfaceLock(surface: *mut c_void, options: u32, seed: *mut u32) -> i32;
        fn IOSurfaceUnlock(surface: *mut c_void, options: u32, seed: *mut u32) -> i32;
        fn IOSurfaceGetBaseAddress(surface: *mut c_void) -> *const u8;
        fn IOSurfaceGetBytesPerRow(surface: *mut c_void) -> usize;
        fn IOSurfaceGetWidth(surface: *mut c_void) -> usize;
        fn IOSurfaceGetHeight(surface: *mut c_void) -> usize;
        fn IOSurfaceGetPixelFormat(surface: *mut c_void) -> u32;
    }

    const READ_ONLY: u32 = 1;

    /// A pixel of a frame's IOSurface, as BGRA, which it locks to read on the
    /// CPU.
    fn surface_pixel(frame: &AcquiredFrame, x: usize, y: usize) -> [u8; 4] {
        let (size, image) = exported_image(frame);
        let ExportedImage::IoSurface { surface } = image else { panic!("the frame has no IOSurface") };
        let surface = surface as usize as *mut c_void;
        // SAFETY: the frame's live surface, locked around the read.
        unsafe {
            assert_eq!(
                (size.width as usize, size.height as usize),
                (IOSurfaceGetWidth(surface), IOSurfaceGetHeight(surface))
            );
            // kCVPixelFormatType_32BGRA.
            assert_eq!(0x4247_5241, IOSurfaceGetPixelFormat(surface));
            assert_eq!(0, IOSurfaceLock(surface, READ_ONLY, std::ptr::null_mut()));
            let row = IOSurfaceGetBytesPerRow(surface);
            let data = IOSurfaceGetBaseAddress(surface);
            let mut pixel = [0u8; 4];
            std::ptr::copy_nonoverlapping(data.add(y * row + x * 4), pixel.as_mut_ptr(), 4);
            assert_eq!(0, IOSurfaceUnlock(surface, READ_ONLY, std::ptr::null_mut()));
            pixel
        }
    }

    #[test]
    fn an_io_surface_ring_renders_into_surfaces_that_the_host_reads() {
        let runtime = runtime();
        for (backend, session) in sessions_exporting(&runtime, ExportKind::IoSurface, SyncKinds::NONE) {
            assert_eq!(Some(ExportKind::IoSurface), session.capabilities().unwrap().export_kind);
            for token in 1..=DEPTH as u64 + 1 {
                let frame = render_and_acquire(&session, token);
                let center = surface_pixel(&frame, SIZE.width as usize / 2, SIZE.height as usize / 2);
                assert_eq!([0, 0, 255, 255], center, "{backend:?}");
                frame.release(ConsumerSync::of(GpuSync::of(SyncKind::CpuComplete))).unwrap();
            }
        }
    }
}

#[cfg(target_os = "android")]
mod android {
    use std::ffi::{c_int, c_void};

    use super::*;

    /// `AHardwareBuffer_Desc`.
    #[repr(C)]
    #[derive(Default)]
    struct Desc {
        width: u32,
        height: u32,
        layers: u32,
        format: u32,
        usage: u64,
        stride: u32,
        rfu0: u32,
        rfu1: u64,
    }

    #[link(name = "nativewindow")]
    unsafe extern "C" {
        fn AHardwareBuffer_describe(buffer: *const c_void, desc: *mut Desc);
        fn AHardwareBuffer_lock(
            buffer: *mut c_void,
            usage: u64,
            fence: c_int,
            rect: *const c_void,
            address: *mut *mut c_void,
        ) -> c_int;
        fn AHardwareBuffer_unlock(buffer: *mut c_void, fence: *mut c_int) -> c_int;
    }

    const RED: [u8; 4] = [255, 0, 0, 255];
    const FORMAT_R8G8B8A8_UNORM: u32 = 1;
    const USAGE_CPU_READ_OFTEN: u64 = 3;
    const USAGE_GPU_SAMPLED_IMAGE: u64 = 1 << 8;
    const USAGE_GPU_COLOR_OUTPUT: u64 = 1 << 9;

    /// The backends whose devices export hardware buffers on Android.
    const EXPORTING: [BackendType; 2] = [BackendType::Vulkan, BackendType::OpenGL];

    /// A session of a map of `style` on a device of `backend`, whose ring of
    /// hardware buffers the CPU reads, or `None` where the device exports
    /// none.
    fn session(runtime: &Runtime, backend: BackendType, style: &str, producer: SyncKinds) -> Option<RenderSession> {
        let device = device_of(backend)?;
        if !device.export_kinds().contains(ExportKind::AndroidHardwareBuffer) {
            eprintln!("skipped: the {backend:?} device exports no hardware buffers");
            return None;
        }
        let options = MapOptions::new(mln_map::MapOptions::new().with_size(SIZE).with_map_mode(MapMode::Continuous));
        let (map, created) = runtime.create_map(options).unwrap();
        created.wait().unwrap();
        wait(map.set_style_json(style.to_owned()));
        let ring = ExportedRing {
            android_hardware_buffer_usage: USAGE_CPU_READ_OFTEN,
            ..ring(&[ExportKind::AndroidHardwareBuffer])
        };
        Some(attach(&map, ring, Some(device), producer).unwrap())
    }

    /// Waits for a frame's producer sync file, if it has one.
    fn wait_for_producer(frame: &AcquiredFrame) {
        let sync = frame.producer_sync();
        if sync.kind != SyncKind::SyncFd || (sync.handle as i64) < 0 {
            return;
        }
        let mut waiting = libc::pollfd { fd: sync.handle as i32, events: libc::POLLIN, revents: 0 };
        // SAFETY: a poll of the frame's live sync file.
        assert_eq!(1, unsafe { libc::poll(&raw mut waiting, 1, 10_000) }, "the frame's sync file signals");
    }

    /// The pixel at `x`, `y` from the first row of a frame's hardware
    /// buffer, which the CPU locks to read once the frame is done.
    fn pixel(frame: &AcquiredFrame, x: u32, y: u32) -> [u8; 4] {
        wait_for_producer(frame);
        let (size, image) = exported_image(frame);
        let ExportedImage::AndroidHardwareBuffer { buffer } = image else { panic!("the frame has no hardware buffer") };
        let buffer = buffer as usize as *mut c_void;
        let mut desc = Desc::default();
        // SAFETY: the frame's live buffer, locked around the read.
        unsafe {
            AHardwareBuffer_describe(buffer, &raw mut desc);
            assert_eq!((size.width, size.height, 1), (desc.width, desc.height, desc.layers));
            assert_eq!(FORMAT_R8G8B8A8_UNORM, desc.format);
            let usage = USAGE_GPU_COLOR_OUTPUT | USAGE_GPU_SAMPLED_IMAGE | USAGE_CPU_READ_OFTEN;
            assert_eq!(usage, desc.usage & usage);
            let mut data = std::ptr::null_mut();
            assert_eq!(0, AHardwareBuffer_lock(buffer, USAGE_CPU_READ_OFTEN, -1, std::ptr::null(), &raw mut data));
            let offset = (y as usize * desc.stride as usize + x as usize) * 4;
            let mut pixel = [0u8; 4];
            std::ptr::copy_nonoverlapping(data.cast::<u8>().add(offset), pixel.as_mut_ptr(), 4);
            assert_eq!(0, AHardwareBuffer_unlock(buffer, std::ptr::null_mut()));
            pixel
        }
    }

    /// What the host's reads of `frame` end with: a sync file where the
    /// session waits for one, here a duplicate of the frame's own, which
    /// has signaled, and a finished read otherwise.
    fn consumer_of(frame: &AcquiredFrame, consumers: SyncKinds) -> ConsumerSync {
        use std::os::fd::{AsRawFd as _, BorrowedFd};
        let sync = frame.producer_sync();
        if !consumers.contains(SyncKind::SyncFd) || sync.kind != SyncKind::SyncFd || (sync.handle as i64) < 0 {
            return ConsumerSync::of(GpuSync::of(SyncKind::CpuComplete));
        }
        // SAFETY: the frame's live sync file, which the frame keeps.
        let file = unsafe { BorrowedFd::borrow_raw(sync.handle as i32) }.try_clone_to_owned().unwrap();
        let handle = file.as_raw_fd() as u64;
        ConsumerSync { sync: GpuSync { kind: SyncKind::SyncFd, handle, value: 0 }, sync_file: Some(file) }
    }

    #[test]
    fn a_hardware_buffer_ring_renders_into_buffers_that_the_cpu_reads() {
        for backend in EXPORTING {
            let runtime = runtime();
            let producer = SyncKinds::of(&[SyncKind::SyncFd, SyncKind::CpuComplete]);
            let Some(session) = session(&runtime, backend, &style("#ff0000"), producer) else { continue };
            let capabilities = session.capabilities().unwrap();
            assert_eq!(Some(ExportKind::AndroidHardwareBuffer), capabilities.export_kind, "{backend:?}");
            eprintln!(
                "{backend:?}: producer {:?}, consumers {:?}",
                capabilities.producer_sync_kind,
                capabilities.consumer_sync_kinds.iter().collect::<Vec<_>>()
            );
            for token in 1..=2 * DEPTH as u64 {
                let frame = render_and_acquire(&session, token);
                assert_eq!(capabilities.producer_sync_kind, Some(frame.producer_sync().kind), "{backend:?}");
                assert_eq!(RED, pixel(&frame, SIZE.width / 2, SIZE.height / 2), "{backend:?}");
                let consumer = consumer_of(&frame, capabilities.consumer_sync_kinds);
                frame.release(consumer).unwrap();
            }
            detach(session);
        }
    }

    /// A hardware buffer's readers, such as a texture or a `SurfaceFlinger`
    /// layer, take its first row as the top of the image: the map's north.
    #[test]
    fn a_hardware_buffer_starts_at_the_top_of_the_map() {
        let north = r##"{"version":8,"transition":{"duration":0,"delay":0},"sources":{"north":{"type":"geojson","data":{"type":"Polygon","coordinates":[[[-180,0],[180,0],[180,85],[-180,85],[-180,0]]]}}},"layers":[{"id":"bg","type":"background","paint":{"background-color":"#ff0000"}},{"id":"north","type":"fill","source":"north","paint":{"fill-color":"#0000ff","fill-antialias":false}}]}"##;
        for backend in EXPORTING {
            let runtime = runtime();
            let Some(session) = session(&runtime, backend, north, SyncKinds::NONE) else { continue };
            let mut frame = render_and_acquire(&session, 1);
            // The fill loads after the first frame.
            for token in 2..50 {
                if pixel(&frame, SIZE.width / 2, SIZE.height / 4) != RED {
                    break;
                }
                drop(frame);
                std::thread::sleep(std::time::Duration::from_millis(20));
                frame = render_and_acquire(&session, token);
            }
            assert_eq!([0, 0, 255, 255], pixel(&frame, SIZE.width / 2, SIZE.height / 4), "{backend:?}");
            assert_eq!(RED, pixel(&frame, SIZE.width / 2, SIZE.height * 3 / 4), "{backend:?}");
            drop(frame);
            detach(session);
        }
    }
}
