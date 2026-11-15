//! The check of the browser's GPU that check/check.mjs runs: `mln_gfx`'s
//! conformance case of writes from JS arrays (`conformance::host_writes`),
//! on a `Gpu` of the map's backend made for the check alone, since the map's
//! own is its render session's. The case tests how the backend takes host
//! bytes, not the map.

use std::cell::RefCell;
use std::future::Future;
use std::rc::Rc;

use mln_gfx::RendererBackend;
use mln_gfx::gpu::{ReadbackId, ResourceId, conformance};
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
        backend: Rc<dyn RendererBackend>,
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
                Ok(backend) => {
                    let host = |bytes: &[u8]| mln_util::host_bytes::HostBytes::new(js_sys::Uint8Array::from(bytes));
                    let ran = conformance::host_writes(&mut *backend.gpu(), host);
                    match ran {
                        Ok(readback) => State::Running { backend, readback },
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
        match &*state {
            State::Idle => Err("no GPU check started".to_owned()),
            State::Starting => Ok(false),
            State::Failed(error) => Err(error.clone()),
            State::Running { backend, readback } => {
                let Some(pixels) = backend.gpu().take_readback(*readback, false).map_err(|e| e.to_string())? else {
                    return Ok(false);
                };
                conformance::check_host_writes(&pixels);
                *state = State::Idle;
                Ok(true)
            }
        }
    }

    fn drop_readback(&self) {
        if let State::Running { backend, readback } = std::mem::take(&mut *self.0.borrow_mut()) {
            backend.gpu().destroy(ResourceId::Readback(readback));
        }
    }
}

/// A backend of the kind the map's render session draws with: the
/// browser's WebGPU where the build has it, and WebGL2, on a canvas of its
/// own, otherwise.
fn backend() -> impl Future<Output = Result<Rc<dyn RendererBackend>, Error>> {
    #[cfg(feature = "webgpu")]
    return async {
        let device = mln_webgpu::Device::request().await?;
        let backend: Rc<dyn RendererBackend> = Rc::new(mln_webgpu::RendererBackend::new(&device));
        Ok(backend)
    };
    #[cfg(not(feature = "webgpu"))]
    std::future::ready(webgl_backend())
}

#[cfg(not(feature = "webgpu"))]
fn webgl_backend() -> Result<Rc<dyn RendererBackend>, Error> {
    use mln_util::size::Size;
    let canvas = web_sys::OffscreenCanvas::new(1, 1).map_err(|error| Error::native(format!("{error:?}")))?;
    let (backend, _target) = mln_gl::CanvasTarget::with_backend(canvas, Size::new(1, 1))?;
    Ok(backend)
}
