// D3D12 through the C API: the library's device and one that wraps the host's,
// a ring of the host's textures that the host reads back on the device's
// queue, an exported ring of shared textures, a composition swap chain that a
// DirectComposition visual shows, and a custom layer's pass.

use core::ffi::c_void;

use mln_d3d12::windows::Win32::Foundation::{E_ACCESSDENIED, HANDLE};
use mln_d3d12::windows::Win32::Graphics::Direct3D12::{
    D3D12_COMMAND_LIST_TYPE_COMPUTE, D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_COMMAND_QUEUE_DESC, D3D12_FENCE_FLAG_NONE,
    D3D12_HEAP_FLAG_NONE, D3D12_HEAP_PROPERTIES, D3D12_HEAP_TYPE_DEFAULT, D3D12_HEAP_TYPE_READBACK,
    D3D12_PLACED_SUBRESOURCE_FOOTPRINT, D3D12_RESOURCE_DESC, D3D12_RESOURCE_DIMENSION_BUFFER,
    D3D12_RESOURCE_DIMENSION_TEXTURE2D, D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET, D3D12_RESOURCE_FLAG_NONE,
    D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATES, D3D12_SUBRESOURCE_FOOTPRINT,
    D3D12_TEXTURE_COPY_LOCATION, D3D12_TEXTURE_COPY_LOCATION_0, D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
    D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX, D3D12_TEXTURE_LAYOUT_ROW_MAJOR, D3D12_TEXTURE_LAYOUT_UNKNOWN,
    ID3D12CommandAllocator, ID3D12CommandQueue, ID3D12Device, ID3D12Fence, ID3D12GraphicsCommandList, ID3D12Resource,
};
use mln_d3d12::windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice2, IDCompositionDesktopDevice, IDCompositionVisual,
};
use mln_d3d12::windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
};
use mln_d3d12::windows::core::{IUnknown, Interface as _};
use parking_lot::Mutex;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, RegisterClassExW,
    SW_SHOWNOACTIVATE, ShowWindow, TranslateMessage, WNDCLASSEXW, WS_POPUP,
};

use crate::acquired_frame::{
    mln_acquired_frame_get_d3d12_texture, mln_acquired_frame_get_dxgi_shared_texture,
    mln_acquired_frame_get_producer_sync, mln_acquired_frame_get_vulkan_texture, mln_acquired_frame_release,
    mln_render_session_acquire_frame,
};
use crate::custom_layer::mln_map_add_custom_layer;
use crate::generated::types::{
    MLN_D3D12_COMPOSITION_KIND_SWAP_CHAIN_PANEL, MLN_D3D12_COMPOSITION_KIND_VISUAL,
    MLN_DEPTH_STENCIL_FORMAT_FLAG_DEPTH24_STENCIL8, MLN_DEPTH_STENCIL_FORMAT_FLAG_DEPTH32_FLOAT_STENCIL8,
    MLN_FRAME_DEMAND_FORCE, MLN_GPU_DEVICE_OPTION_BACKEND, MLN_GPU_EXPORT_KIND_FLAG_DXGI_SHARED_HANDLE,
    MLN_GPU_SYNC_CPU_COMPLETE, MLN_GPU_SYNC_D3D12_FENCE, MLN_GPU_SYNC_KIND_FLAG_D3D12_FENCE,
    MLN_GPU_SYNC_KIND_FLAG_QUEUE_ORDER, MLN_GPU_SYNC_QUEUE_ORDER, MLN_RENDER_BACKEND_D3D12,
    MLN_RENDER_BACKEND_FLAG_D3D12, MLN_RENDER_DRIVER_CALLER_GRAPHICS_THREAD, MLN_RENDER_RESULT_RENDERED,
    MLN_RENDER_TARGET_KIND_D3D12_BORROWED_TEXTURES, MLN_RENDER_TARGET_KIND_D3D12_COMPOSITION,
    MLN_RENDER_TARGET_KIND_EXPORTED_RING, MLN_RENDER_TARGET_KIND_FLAG_D3D12_BORROWED_TEXTURES,
    MLN_RENDER_TARGET_KIND_FLAG_D3D12_COMPOSITION, MLN_RENDER_TARGET_KIND_FLAG_EXPORTED_RING,
    MLN_RENDER_TARGET_KIND_FLAG_OFFSCREEN, MLN_RENDER_TARGET_KIND_FLAG_WIN32_WINDOW, MLN_STATUS_INVALID_ARGUMENT,
    MLN_STATUS_OK, MLN_STATUS_UNSUPPORTED, mln_custom_layer_host, mln_custom_layer_render_parameters,
    mln_d3d12_borrowed_texture, mln_d3d12_borrowed_textures_target, mln_d3d12_composition_target,
    mln_d3d12_device_descriptor, mln_d3d12_texture_frame, mln_diagnostic, mln_dxgi_shared_texture_frame,
    mln_exported_ring_target, mln_gpu_device, mln_gpu_device_capabilities, mln_gpu_device_d3d12_context_result,
    mln_gpu_device_options, mln_gpu_device_vulkan_context_result, mln_gpu_sync, mln_logical_size, mln_map,
    mln_render_target, mln_render_target_data, mln_vulkan_texture_frame,
};
use crate::gpu_device::{
    mln_gpu_device_create, mln_gpu_device_get_capabilities, mln_gpu_device_get_d3d12_context,
    mln_gpu_device_get_vulkan_context, mln_gpu_device_release, mln_gpu_device_wrap_d3d12,
};
use crate::map::mln_map_set_style_json;
use crate::map::tests::{create_map, options, release_map, view};
use crate::render_session::tests::{
    attach_options, demand, offscreen_target, read_pixel, release_session, sized, wait_for_result,
};
use crate::render_target::mln_map_attach;
use crate::runtime::tests::{Waiter, create, release};

const NO_DIAGNOSTIC: *mut mln_diagnostic = core::ptr::null_mut();
const RED: [u8; 4] = [255, 0, 0, 255];
/// The ring's size, in pixels at a scale factor of 1: 64 pixels of 4 bytes
/// make a row of D3D12's 256-byte row pitch.
const WIDTH: u32 = 64;
const HEIGHT: u32 = 48;
const STYLE: &str =
    r#"{"version":8,"sources":{},"layers":[{"id":"bg","type":"background","paint":{"background-color":"red"}}]}"#;

/// A library device on D3D12, or `None` where the machine has none.
fn d3d12_device() -> Option<mln_gpu_device> {
    let mut options = sized::<mln_gpu_device_options>();
    options.fields = MLN_GPU_DEVICE_OPTION_BACKEND;
    options.backend = MLN_RENDER_BACKEND_D3D12;
    let mut device = 0;
    // SAFETY: local arguments.
    let status = unsafe { mln_gpu_device_create(&raw const options, &raw mut device, NO_DIAGNOSTIC) };
    if status != MLN_STATUS_OK {
        eprintln!("skipped: no D3D12 device");
        return None;
    }
    Some(device)
}

/// The device's D3D12 device and queue.
fn context(device: mln_gpu_device) -> (ID3D12Device, ID3D12CommandQueue) {
    let mut result = sized::<mln_gpu_device_d3d12_context_result>();
    // SAFETY: local arguments.
    assert_eq!(unsafe { mln_gpu_device_get_d3d12_context(device, &raw mut result, NO_DIAGNOSTIC) }, MLN_STATUS_OK);
    // SAFETY: the device's live objects; the clones add references.
    unsafe {
        (
            ID3D12Device::from_raw_borrowed(&result.device).unwrap().clone(),
            ID3D12CommandQueue::from_raw_borrowed(&result.command_queue).unwrap().clone(),
        )
    }
}

fn red_map(runtime: crate::generated::types::mln_runtime) -> mln_map {
    let map = create_map(runtime, &options(1.0));
    let styled = Waiter::new();
    // SAFETY: local arguments.
    unsafe { mln_map_set_style_json(map, view(STYLE), &raw const styled.descriptor, NO_DIAGNOSTIC) };
    assert_eq!(styled.status(), MLN_STATUS_OK);
    map
}

/// Attaches `target` to `map` on `device`, handing frames over with
/// `producer` sync.
fn attach(map: mln_map, target: &mln_render_target, device: mln_gpu_device, producer: u32) -> (mln_status, u64) {
    let (mut session, attached) = (0, Waiter::new());
    let mut options = attach_options();
    options.producer_sync_kinds = producer;
    // SAFETY: local arguments.
    let status = unsafe {
        mln_map_attach(
            map,
            target,
            device,
            &raw const options,
            &raw mut session,
            &raw const attached.descriptor,
            NO_DIAGNOSTIC,
        )
    };
    if status != MLN_STATUS_OK {
        return (status, 0);
    }
    (attached.status(), session)
}

use crate::generated::types::mln_status;

/// A committed resource of `desc` in a heap of `heap`, made in `state`.
fn resource(
    device: &ID3D12Device,
    heap: mln_d3d12::windows::Win32::Graphics::Direct3D12::D3D12_HEAP_TYPE,
    desc: &D3D12_RESOURCE_DESC,
    state: D3D12_RESOURCE_STATES,
) -> ID3D12Resource {
    let heap = D3D12_HEAP_PROPERTIES { Type: heap, ..Default::default() };
    let mut resource: Option<ID3D12Resource> = None;
    // SAFETY: the descriptions outlive the call, which fills `resource`.
    unsafe {
        device.CreateCommittedResource(&raw const heap, D3D12_HEAP_FLAG_NONE, desc, state, None, &raw mut resource)
    }
    .unwrap();
    resource.unwrap()
}

/// A render target texture of the ring's size, in the common state.
fn texture(device: &ID3D12Device) -> ID3D12Resource {
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Width: u64::from(WIDTH),
        Height: HEIGHT,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
        Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
        Alignment: 0,
    };
    resource(device, D3D12_HEAP_TYPE_DEFAULT, &desc, D3D12_RESOURCE_STATE_COMMON)
}

/// The center pixel of `texture`, which the host copies on `queue` once the
/// queue reaches `wait`, if any, and the CPU reads.
fn center_pixel(
    device: &ID3D12Device,
    queue: &ID3D12CommandQueue,
    texture: &ID3D12Resource,
    wait: Option<(&ID3D12Fence, u64)>,
) -> [u8; 4] {
    let row_pitch = WIDTH * 4;
    let readback = resource(
        device,
        D3D12_HEAP_TYPE_READBACK,
        &D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: u64::from(row_pitch * HEIGHT),
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_UNKNOWN,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            Flags: D3D12_RESOURCE_FLAG_NONE,
            Alignment: 0,
        },
        D3D12_RESOURCE_STATE_COPY_DEST,
    );
    let dst = D3D12_TEXTURE_COPY_LOCATION {
        // SAFETY: copies of the pointers without references, which the
        // locations' ManuallyDrop never releases.
        pResource: unsafe { std::mem::transmute_copy(&readback) },
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                Offset: 0,
                Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                    Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                    Width: WIDTH,
                    Height: HEIGHT,
                    Depth: 1,
                    RowPitch: row_pitch,
                },
            },
        },
    };
    let src = D3D12_TEXTURE_COPY_LOCATION {
        // SAFETY: as above.
        pResource: unsafe { std::mem::transmute_copy(texture) },
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
    };
    // SAFETY: the host's live objects; the texture rests in the common state,
    // which a copy promotes from and decays back to, and the CPU waits for the
    // copy before it reads the buffer.
    unsafe {
        let allocator: ID3D12CommandAllocator = device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT).unwrap();
        let list: ID3D12GraphicsCommandList =
            device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None).unwrap();
        list.CopyTextureRegion(&raw const dst, 0, 0, 0, &raw const src, None);
        list.Close().unwrap();
        if let Some((fence, value)) = wait {
            queue.Wait(fence, value).unwrap();
        }
        queue.ExecuteCommandLists(&[Some(list.cast().unwrap())]);
        let done: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap();
        queue.Signal(&done, 1).unwrap();
        done.SetEventOnCompletion(1, HANDLE::default()).unwrap();
        let mut data = core::ptr::null_mut();
        readback.Map(0, None, Some(&raw mut data)).unwrap();
        let offset = ((HEIGHT / 2) * row_pitch + WIDTH / 2 * 4) as usize;
        let mut pixel = [0; 4];
        core::ptr::copy_nonoverlapping(data.cast::<u8>().add(offset), pixel.as_mut_ptr(), 4);
        readback.Unmap(0, None);
        pixel
    }
}

/// Renders a forced frame of `session` and acquires it.
fn render_and_acquire(session: u64, token: u64) -> u64 {
    assert_eq!(demand(session, MLN_FRAME_DEMAND_FORCE, token), MLN_STATUS_OK);
    assert_eq!(wait_for_result(session).disposition, MLN_RENDER_RESULT_RENDERED);
    let mut frame = 0;
    // SAFETY: local outputs.
    assert_eq!(unsafe { mln_render_session_acquire_frame(session, &raw mut frame, NO_DIAGNOSTIC) }, MLN_STATUS_OK);
    frame
}

// A library D3D12 device reports its backend, targets and sync kinds, and
// its D3D12 objects, but no Vulkan ones.
#[test]
fn a_d3d12_device_reports_its_targets_and_objects() {
    assert_ne!(0, crate::base::mln_supported_render_backend_mask() & MLN_RENDER_BACKEND_FLAG_D3D12);
    let Some(device) = d3d12_device() else { return };
    let mut capabilities = sized::<mln_gpu_device_capabilities>();
    // SAFETY: local arguments.
    unsafe { mln_gpu_device_get_capabilities(device, &raw mut capabilities, NO_DIAGNOSTIC) };
    assert_eq!(MLN_RENDER_BACKEND_D3D12, capabilities.backend);
    for kind in [
        MLN_RENDER_TARGET_KIND_FLAG_OFFSCREEN,
        MLN_RENDER_TARGET_KIND_FLAG_WIN32_WINDOW,
        MLN_RENDER_TARGET_KIND_FLAG_D3D12_BORROWED_TEXTURES,
        MLN_RENDER_TARGET_KIND_FLAG_D3D12_COMPOSITION,
        MLN_RENDER_TARGET_KIND_FLAG_EXPORTED_RING,
    ] {
        assert_ne!(0, capabilities.target_kinds & kind, "{kind:#x} of {:#x}", capabilities.target_kinds);
    }
    assert_eq!(MLN_GPU_EXPORT_KIND_FLAG_DXGI_SHARED_HANDLE, capabilities.export_kinds);
    let fences = MLN_GPU_SYNC_KIND_FLAG_D3D12_FENCE | MLN_GPU_SYNC_KIND_FLAG_QUEUE_ORDER;
    assert_eq!(fences, capabilities.sync_kinds & fences);
    assert_eq!(
        MLN_DEPTH_STENCIL_FORMAT_FLAG_DEPTH24_STENCIL8 | MLN_DEPTH_STENCIL_FORMAT_FLAG_DEPTH32_FLOAT_STENCIL8,
        capabilities.depth_stencil_formats
    );
    let (d3d, queue) = context(device);
    // SAFETY: a plain query of the device's queue.
    assert_eq!(D3D12_COMMAND_LIST_TYPE_DIRECT, unsafe { queue.GetDesc() }.Type);
    drop(d3d);
    let mut vulkan = sized::<mln_gpu_device_vulkan_context_result>();
    // SAFETY: local arguments.
    let status = unsafe { mln_gpu_device_get_vulkan_context(device, &raw mut vulkan, NO_DIAGNOSTIC) };
    assert_eq!(MLN_STATUS_UNSUPPORTED, status);
    mln_gpu_device_release(device);
}

// A device that wraps the host's D3D12 device and direct queue renders on
// them; a queue that isn't a direct one, or no queue, wraps nothing.
#[test]
fn a_wrapped_d3d12_device_renders_on_the_hosts_queue() {
    let Some(library) = d3d12_device() else { return };
    let (d3d, queue) = context(library);
    mln_gpu_device_release(library);
    let wrap = |device: *mut c_void, command_queue: *mut c_void| {
        let descriptor = mln_d3d12_device_descriptor { device, command_queue, ..sized() };
        let mut wrapped = 0;
        // SAFETY: local arguments naming live objects or null.
        let status = unsafe { mln_gpu_device_wrap_d3d12(&raw const descriptor, &raw mut wrapped, NO_DIAGNOSTIC) };
        (status, wrapped)
    };
    assert_eq!(MLN_STATUS_INVALID_ARGUMENT, wrap(d3d.as_raw(), core::ptr::null_mut()).0);
    let compute_desc = D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_COMPUTE, ..Default::default() };
    // SAFETY: a plain call of the live device.
    let compute: ID3D12CommandQueue = unsafe { d3d.CreateCommandQueue(&raw const compute_desc) }.unwrap();
    assert_eq!(MLN_STATUS_UNSUPPORTED, wrap(d3d.as_raw(), compute.as_raw()).0);
    // A queue that is no D3D12 queue at all.
    assert_eq!(MLN_STATUS_INVALID_ARGUMENT, wrap(d3d.as_raw(), d3d.as_raw()).0);

    let (status, device) = wrap(d3d.as_raw(), queue.as_raw());
    assert_eq!(MLN_STATUS_OK, status);
    let (wrapped_device, wrapped_queue) = context(device);
    assert_eq!((&d3d, &queue), (&wrapped_device, &wrapped_queue));
    let runtime = create();
    let map = red_map(runtime);
    let (status, session) = attach(map, &offscreen_target(WIDTH, HEIGHT), device, 0);
    mln_gpu_device_release(device);
    assert_eq!(MLN_STATUS_OK, status);
    assert_eq!(demand(session, MLN_FRAME_DEMAND_FORCE, 1), MLN_STATUS_OK);
    assert_eq!(wait_for_result(session).disposition, MLN_RENDER_RESULT_RENDERED);
    assert_eq!(read_pixel(session), (MLN_STATUS_OK, Some((WIDTH, HEIGHT, RED))));
    release_session(session);
    release_map(map);
    release(runtime);
}

// A ring of the host's D3D12 textures hands each frame over with the
// session's shared fence, which the host's queue waits for before it copies
// the texture, and names the texture through the D3D12 getter alone. A ring
// of textures in queue order needs no fence.
#[test]
fn a_d3d12_ring_hands_frames_to_the_host() {
    let Some(device) = d3d12_device() else { return };
    let (d3d, queue) = context(device);
    let textures = [texture(&d3d), texture(&d3d)];
    let entries: Vec<mln_d3d12_borrowed_texture> =
        textures.iter().map(|texture| mln_d3d12_borrowed_texture { resource: texture.as_raw() }).collect();
    let ring = mln_d3d12_borrowed_textures_target {
        logical_size: mln_logical_size { width: WIDTH, height: HEIGHT },
        physical_width: WIDTH,
        physical_height: HEIGHT,
        textures: entries.as_ptr(),
        texture_count: entries.len(),
        state: D3D12_RESOURCE_STATE_COMMON.0 as u32,
        depth_textures: core::ptr::null(),
        depth_texture_count: 0,
        depth_state: 0,
    };
    let target = mln_render_target {
        kind: MLN_RENDER_TARGET_KIND_D3D12_BORROWED_TEXTURES,
        data: mln_render_target_data { d3d12_borrowed_textures: ring },
        ..sized()
    };
    let runtime = create();
    let map = red_map(runtime);
    // A ring renders on the device of its textures.
    assert_eq!(MLN_STATUS_UNSUPPORTED, attach(map, &target, 0, 0).0);
    for producer in [MLN_GPU_SYNC_KIND_FLAG_D3D12_FENCE, MLN_GPU_SYNC_KIND_FLAG_QUEUE_ORDER] {
        let (status, session) = attach(map, &target, device, producer);
        assert_eq!(MLN_STATUS_OK, status, "{producer:#x}");
        for token in 1..=3 {
            let frame = render_and_acquire(session, token);
            let mut texture = sized::<mln_d3d12_texture_frame>();
            // SAFETY: local outputs.
            assert_eq!(MLN_STATUS_OK, unsafe {
                mln_acquired_frame_get_d3d12_texture(frame, &raw mut texture, NO_DIAGNOSTIC)
            });
            assert_eq!((WIDTH, HEIGHT), (texture.width, texture.height));
            assert_eq!(textures[texture.slot as usize].as_raw(), texture.resource);
            assert_eq!(d3d.as_raw(), texture.device);
            assert_eq!(DXGI_FORMAT_R8G8B8A8_UNORM.0 as u32, texture.format);
            assert_eq!(D3D12_RESOURCE_STATE_COMMON.0 as u32, texture.state);
            assert!(texture.depth_resource.is_null());
            let mut vulkan = sized::<mln_vulkan_texture_frame>();
            // SAFETY: local outputs.
            assert_eq!(MLN_STATUS_UNSUPPORTED, unsafe {
                mln_acquired_frame_get_vulkan_texture(frame, &raw mut vulkan, NO_DIAGNOSTIC)
            });
            let mut sync = sized::<mln_gpu_sync>();
            // SAFETY: local outputs.
            assert_eq!(MLN_STATUS_OK, unsafe {
                mln_acquired_frame_get_producer_sync(frame, &raw mut sync, NO_DIAGNOSTIC)
            });
            let resource = &textures[texture.slot as usize];
            let pixel = if producer == MLN_GPU_SYNC_KIND_FLAG_D3D12_FENCE {
                assert_eq!(MLN_GPU_SYNC_D3D12_FENCE, sync.kind);
                let mut fence: Option<ID3D12Fence> = None;
                // SAFETY: the session's live NT handle, which the call
                // doesn't take.
                unsafe { d3d.OpenSharedHandle(HANDLE(sync.handle as usize as *mut c_void), &raw mut fence) }.unwrap();
                center_pixel(&d3d, &queue, resource, Some((&fence.unwrap(), sync.value)))
            } else {
                assert_eq!(MLN_GPU_SYNC_QUEUE_ORDER, sync.kind);
                center_pixel(&d3d, &queue, resource, None)
            };
            assert_eq!(RED, pixel);
            let done = mln_gpu_sync { kind: MLN_GPU_SYNC_CPU_COMPLETE, ..sized() };
            // SAFETY: the live frame, and a local sync.
            assert_eq!(unsafe { mln_acquired_frame_release(frame, &raw const done, NO_DIAGNOSTIC) }, MLN_STATUS_OK);
        }
        release_session(session);
    }
    mln_gpu_device_release(device);
    release_map(map);
    release(runtime);
}

// An exported ring on a D3D12 device shares its textures through DXGI shared
// handles, and names each on the device through the D3D12 getter.
#[test]
fn a_d3d12_exported_ring_frame_has_its_shared_texture_and_resource() {
    let Some(device) = d3d12_device() else { return };
    let ring = mln_exported_ring_target {
        logical_size: mln_logical_size { width: 32, height: 16 },
        texture_count: 3,
        export_kinds: MLN_GPU_EXPORT_KIND_FLAG_DXGI_SHARED_HANDLE,
        dma_buf_formats: core::ptr::null(),
        dma_buf_format_count: 0,
        android_hardware_buffer_usage: 0,
    };
    let target = mln_render_target {
        kind: MLN_RENDER_TARGET_KIND_EXPORTED_RING,
        data: mln_render_target_data { exported_ring: ring },
        ..sized()
    };
    let runtime = create();
    let map = red_map(runtime);
    let (status, session) = attach(map, &target, device, MLN_GPU_SYNC_KIND_FLAG_D3D12_FENCE);
    mln_gpu_device_release(device);
    assert_eq!(MLN_STATUS_OK, status);
    for token in 1..=4 {
        let frame = render_and_acquire(session, token);
        let mut shared = sized::<mln_dxgi_shared_texture_frame>();
        // SAFETY: local outputs.
        assert_eq!(MLN_STATUS_OK, unsafe {
            mln_acquired_frame_get_dxgi_shared_texture(frame, &raw mut shared, NO_DIAGNOSTIC)
        });
        assert_eq!((32, 16, DXGI_FORMAT_B8G8R8A8_UNORM.0 as u32), (shared.width, shared.height, shared.format));
        assert!(!shared.handle.is_null());
        let mut texture = sized::<mln_d3d12_texture_frame>();
        // SAFETY: local outputs.
        assert_eq!(MLN_STATUS_OK, unsafe {
            mln_acquired_frame_get_d3d12_texture(frame, &raw mut texture, NO_DIAGNOSTIC)
        });
        assert_eq!(shared.slot, texture.slot);
        assert!(!texture.resource.is_null() && !texture.device.is_null());
        assert_eq!(DXGI_FORMAT_B8G8R8A8_UNORM.0 as u32, texture.format);
        assert_eq!(D3D12_RESOURCE_STATE_COMMON.0 as u32, texture.state);
        let mut sync = sized::<mln_gpu_sync>();
        // SAFETY: local outputs.
        unsafe { mln_acquired_frame_get_producer_sync(frame, &raw mut sync, NO_DIAGNOSTIC) };
        assert_eq!(MLN_GPU_SYNC_D3D12_FENCE, sync.kind);
        let done = mln_gpu_sync { kind: MLN_GPU_SYNC_CPU_COMPLETE, ..sized() };
        // SAFETY: the live frame, and a local sync.
        assert_eq!(unsafe { mln_acquired_frame_release(frame, &raw const done, NO_DIAGNOSTIC) }, MLN_STATUS_OK);
    }
    release_session(session);
    release_map(map);
    release(runtime);
}

/// `"mln_c_d3d12_test"`, as UTF-16.
const CLASS: &[u16] = &[109, 108, 110, 95, 99, 95, 100, 51, 100, 49, 50, 95, 116, 101, 115, 116, 0];

unsafe extern "system" fn window_procedure(window: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: the system's arguments.
    unsafe { DefWindowProcW(window, message, w, l) }
}

/// A shown window of the host's, which the test's thread handles the
/// messages of.
struct HostWindow(HWND);

impl HostWindow {
    fn new() -> HostWindow {
        // SAFETY: the test's executable.
        let instance = unsafe { GetModuleHandleW(core::ptr::null()) };
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(window_procedure),
            hInstance: instance,
            lpszClassName: CLASS.as_ptr(),
            ..WNDCLASSEXW::default()
        };
        // SAFETY: a valid class, which may already be registered.
        unsafe { RegisterClassExW(&raw const class) };
        // SAFETY: the class is registered.
        let window = unsafe {
            CreateWindowExW(
                0,
                CLASS.as_ptr(),
                CLASS.as_ptr(),
                WS_POPUP,
                0,
                0,
                WIDTH as i32,
                HEIGHT as i32,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                instance,
                core::ptr::null(),
            )
        };
        assert!(!window.is_null(), "a window");
        // SAFETY: the live window.
        unsafe { ShowWindow(window, SW_SHOWNOACTIVATE) };
        pump();
        HostWindow(window)
    }
}

impl Drop for HostWindow {
    fn drop(&mut self) {
        // SAFETY: the test's window, made on this thread.
        unsafe { DestroyWindow(self.0) };
        pump();
    }
}

/// Handles the thread's window messages.
fn pump() {
    // SAFETY: a local message.
    unsafe {
        let mut message: MSG = core::mem::zeroed();
        while PeekMessageW(&raw mut message, core::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&raw const message);
            DispatchMessageW(&raw const message);
        }
    }
}

// A composition target presents through a swap chain that the host's
// DirectComposition visual shows, on a D3D12 device that the session makes
// without one, and on no device of another backend. It takes a known kind
// and an object only.
#[test]
fn a_composition_target_presents_to_a_direct_composition_visual() {
    let Some(device) = d3d12_device() else { return };
    let window = HostWindow::new();
    // SAFETY: plain calls, of a device without a rendering device, on the
    // live window. A non-interactive Windows session has no desktop
    // compositor and denies DirectComposition device creation.
    let (composition, _target, visual) = unsafe {
        let composition: IDCompositionDesktopDevice = match DCompositionCreateDevice2(None::<&IUnknown>) {
            Ok(composition) => composition,
            Err(error) if error.code() == E_ACCESSDENIED => return,
            Err(error) => panic!("DCompositionCreateDevice2 failed: {error}"),
        };
        let target =
            composition.CreateTargetForHwnd(mln_d3d12::windows::Win32::Foundation::HWND(window.0), true).unwrap();
        let visual: IDCompositionVisual = composition.CreateVisual().unwrap().cast().unwrap();
        target.SetRoot(&visual).unwrap();
        (composition, target, visual)
    };
    let target = |kind, object: *mut c_void| mln_render_target {
        kind: MLN_RENDER_TARGET_KIND_D3D12_COMPOSITION,
        data: mln_render_target_data {
            d3d12_composition: mln_d3d12_composition_target {
                logical_size: mln_logical_size { width: WIDTH, height: HEIGHT },
                kind,
                object,
            },
        },
        ..sized()
    };
    let runtime = create();
    let map = red_map(runtime);
    assert_eq!(MLN_STATUS_INVALID_ARGUMENT, attach(map, &target(7, visual.as_raw()), 0, 0).0);
    assert_eq!(
        MLN_STATUS_INVALID_ARGUMENT,
        attach(map, &target(MLN_D3D12_COMPOSITION_KIND_VISUAL, core::ptr::null_mut()), 0, 0).0
    );
    // A visual is no SwapChainPanel.
    assert_eq!(
        MLN_STATUS_INVALID_ARGUMENT,
        attach(map, &target(MLN_D3D12_COMPOSITION_KIND_SWAP_CHAIN_PANEL, visual.as_raw()), device, 0).0
    );
    // The session presents from its core worker.
    let mut caller = attach_options();
    caller.driver = MLN_RENDER_DRIVER_CALLER_GRAPHICS_THREAD;
    let (mut refused, waiter) = (0, Waiter::new());
    let visual_target = target(MLN_D3D12_COMPOSITION_KIND_VISUAL, visual.as_raw());
    // SAFETY: local arguments.
    let status = unsafe {
        mln_map_attach(
            map,
            &raw const visual_target,
            device,
            &raw const caller,
            &raw mut refused,
            &raw const waiter.descriptor,
            NO_DIAGNOSTIC,
        )
    };
    assert_eq!(MLN_STATUS_UNSUPPORTED, status);
    for device in [device, 0] {
        let (status, session) = attach(map, &target(MLN_D3D12_COMPOSITION_KIND_VISUAL, visual.as_raw()), device, 0);
        assert_eq!(MLN_STATUS_OK, status);
        // SAFETY: a plain call.
        unsafe { composition.Commit() }.unwrap();
        for token in 1..=2 {
            pump();
            assert_eq!(demand(session, MLN_FRAME_DEMAND_FORCE, token), MLN_STATUS_OK);
            assert_eq!(wait_for_result(session).disposition, MLN_RENDER_RESULT_RENDERED);
        }
        release_session(session);
    }
    mln_gpu_device_release(device);
    release_map(map);
    release(runtime);
    drop(window);
}

/// What the custom layer's render callback saw: the backend, and whether
/// the pass named its objects.
static PASSES: Mutex<Vec<(u32, bool)>> = Mutex::new(Vec::new());

unsafe extern "C" fn render(_: *mut c_void, parameters: *const mln_custom_layer_render_parameters) {
    // SAFETY: the library's parameters, valid during the call.
    let parameters = unsafe { &*parameters };
    // SAFETY: the variant that `backend` names, when it names D3D12.
    let named = parameters.backend == MLN_RENDER_BACKEND_D3D12
        && unsafe {
            let pass = parameters.pass.d3d12;
            !pass.device.is_null()
                && !pass.command_list.is_null()
                && pass.render_target_view != 0
                && pass.color_format == DXGI_FORMAT_R8G8B8A8_UNORM.0 as u32
                && (pass.depth_stencil_view == 0) == (pass.depth_stencil_format == DXGI_FORMAT_UNKNOWN.0 as u32)
        };
    PASSES.lock().push((parameters.backend, named));
}

// A custom layer's render callback gets the D3D12 pass: its device,
// command list, views and formats.
#[test]
fn a_custom_layer_gets_the_d3d12_pass() {
    let Some(device) = d3d12_device() else { return };
    let runtime = create();
    let map = red_map(runtime);
    let host = mln_custom_layer_host { render: Some(render), ..sized() };
    let added = Waiter::new();
    // SAFETY: local arguments.
    let status = unsafe {
        mln_map_add_custom_layer(
            map,
            view("quad"),
            view(""),
            &raw const host,
            &raw const added.descriptor,
            NO_DIAGNOSTIC,
        )
    };
    assert_eq!((status, added.status()), (MLN_STATUS_OK, MLN_STATUS_OK));
    let (status, session) = attach(map, &offscreen_target(WIDTH, HEIGHT), device, 0);
    mln_gpu_device_release(device);
    assert_eq!(MLN_STATUS_OK, status);
    assert_eq!(demand(session, MLN_FRAME_DEMAND_FORCE, 1), MLN_STATUS_OK);
    assert_eq!(wait_for_result(session).disposition, MLN_RENDER_RESULT_RENDERED);
    release_session(session);
    release_map(map);
    release(runtime);
    let passes = PASSES.lock();
    assert!(!passes.is_empty(), "the layer rendered");
    assert!(passes.iter().all(|&(backend, named)| backend == MLN_RENDER_BACKEND_D3D12 && named), "{passes:?}");
}
