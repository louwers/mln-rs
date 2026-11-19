//! The check of the browser's GPU that check/check.mjs runs: `mln_gfx`'s
//! conformance case of writes from JS arrays (`conformance::host_writes`),
//! on a `Gpu` of the map's backend made for the check alone, since the map's
//! own is its render session's. The case tests how the backend takes host
//! bytes, not the map.

use std::cell::RefCell;
use std::future::Future;
use std::rc::Rc;

use mln_gfx::RendererBackend as _;
use mln_gfx::gpu::{Gpu as _, ReadbackId, ResourceId, conformance, destroy_unused};
use mln_util::error::Error;

/// Where the check is.
#[derive(Default)]
enum State {
    #[default]
    Idle,
    /// The backend is on its way.
    Starting,
    /// The case ran; its frame is on its way back.
    Running {
        backend: Box<Backend>,
        readback: ReadbackId,
    },
    Failed(String),
}

/// The check, which a task of its own starts.
#[derive(Clone, Default)]
pub(crate) struct GpuCheck(Rc<RefCell<State>>);

impl GpuCheck {
    /// Makes the backend and runs the case on it, in a task of its own.
    /// Restarts a check that ran already.
    pub(crate) fn start(&self) {
        if matches!(*self.0.borrow(), State::Starting) {
            return;
        }
        self.drop_readback();
        *self.0.borrow_mut() = State::Starting;
        let check = self.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let state = match backend().await {
                Ok(mut backend) => {
                    let host = |bytes: &[u8]| mln_util::host_bytes::HostBytes::new(js_sys::Uint8Array::from(bytes));
                    let ran = conformance::host_writes(&mut *backend.gpu(), host);
                    match ran {
                        Ok(readback) => State::Running { backend: Box::new(backend), readback },
                        Err(error) => State::Failed(format!("the host writes case failed: {error}")),
                    }
                }
                Err(error) => State::Failed(format!("no GPU for the check: {error}")),
            };
            *check.0.borrow_mut() = state;
        });
    }

    /// Whether the case's frame came back and passed its checks; `false`
    /// until it comes. Fails when no check started or the check failed, and
    /// panics where one of the frame's checks doesn't hold.
    pub(crate) fn take(&self) -> Result<bool, String> {
        let mut state = self.0.borrow_mut();
        match &mut *state {
            State::Idle => Err("no GPU check started".to_owned()),
            State::Starting => Ok(false),
            State::Failed(error) => Err(error.clone()),
            State::Running { backend, readback } => {
                let readback = *readback;
                let Some(pixels) = backend.gpu().take_readback(readback, false).map_err(|e| e.to_string())? else {
                    return Ok(false);
                };
                conformance::check_host_writes(&pixels);
                *state = State::Idle;
                Ok(true)
            }
        }
    }

    fn drop_readback(&self) {
        if let State::Running { mut backend, readback } = std::mem::take(&mut *self.0.borrow_mut()) {
            destroy_unused(&mut *backend.gpu(), [ResourceId::Readback(readback)]);
        }
    }
}

/// The backend of the kind the map's render session draws with: the
/// browser's WebGPU where the build has it, and WebGL2 otherwise.
#[cfg(feature = "webgpu")]
type Backend = mln_webgpu::RendererBackend;
#[cfg(not(feature = "webgpu"))]
type Backend = mln_gl::RendererBackend;

/// A [`Backend`] of the check's own: on the browser's WebGPU, or on a WebGL2
/// canvas of its own.
fn backend() -> impl Future<Output = Result<Backend, Error>> {
    #[cfg(feature = "webgpu")]
    return async {
        let device = mln_webgpu::Device::request().await?;
        Ok(mln_webgpu::RendererBackend::new(&device))
    };
    #[cfg(not(feature = "webgpu"))]
    std::future::ready(webgl_backend())
}

#[cfg(not(feature = "webgpu"))]
fn webgl_backend() -> Result<Backend, Error> {
    use mln_util::size::Size;
    let canvas = web_sys::OffscreenCanvas::new(1, 1).map_err(|error| Error::native(format!("{error:?}")))?;
    let (backend, _target) = mln_gl::CanvasTarget::with_backend(canvas, Size::new(1, 1))?;
    Ok(backend)
}
