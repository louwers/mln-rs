// The window's CAMetalLayer, which both backends present to: made in the
// window's view on the main thread, which AppKit requires, once, and handed
// to the render thread for each backend it makes. A layer per backend would
// leave the last one's frame in the view under the next.

// The layer comes from raw-window-metal as a pointer; taking it is the FFI
// here.
#![allow(unsafe_code)]

use objc2::rc::Retained;
use objc2_quartz_core::CAMetalLayer;
use raw_window_metal::Layer;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

/// A layer in the window's view, sized and scaled with it. Clones share it.
#[derive(Clone)]
pub(crate) struct MetalLayer(Layer);

impl MetalLayer {
    /// Makes the window's view layer-backed with a `CAMetalLayer`. Call on the
    /// main thread.
    pub(crate) fn new(window: &Window) -> Result<MetalLayer, String> {
        let handle = window.window_handle().map_err(|e| format!("The window has no handle: {e}"))?;
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return Err("The window is not an AppKit window".to_owned());
        };
        // SAFETY: the handle's view is the window's NSView, and this runs on
        // the main thread with the window alive.
        Ok(MetalLayer(unsafe { Layer::from_ns_view(handle.ns_view) }))
    }

    /// A wgpu surface presenting to the layer.
    pub(crate) fn create_surface(&self, instance: &wgpu::Instance) -> Result<wgpu::Surface<'static>, String> {
        let target = wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(self.0.as_ptr().as_ptr());
        // SAFETY: the pointer is a CAMetalLayer, and the window keeps it until
        // it closes, after the render thread has dropped its surfaces.
        unsafe { instance.create_surface_unsafe(target) }
            .map_err(|e| format!("Failed to create a surface for the window: {e}"))
    }

    pub(crate) fn into_layer(self) -> Retained<CAMetalLayer> {
        let layer = self.0.into_raw().as_ptr().cast::<CAMetalLayer>();
        // SAFETY: `into_raw` gives up a +1 retained, non-null CAMetalLayer.
        unsafe { Retained::from_raw(layer) }.expect("non-null layer")
    }
}
