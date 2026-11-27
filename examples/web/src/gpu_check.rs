//! The check of the browser's GPU that check/check.mjs runs:
//! `mln_builtins`' conformance cases of writes from JS arrays
//! (`conformance::host_writes`) and of writes while the instance's memory grows
//! (`conformance::memory_growth`), on a `Gpu` of the map's backend made for
//! the check alone, since the map's own is its render session's. The cases
//! test how the backend takes bytes, not the map.

use std::cell::RefCell;
use std::future::Future;
use std::rc::Rc;

use mln_builtins::conformance;
use mln_gfx::gpu::{Gpu as _, ReadbackId, ResourceId, destroy_unused};
use mln_gfx::{Error, RendererBackend as _};
use wasm_bindgen::{JsCast as _, JsValue};

/// Where the check is.
#[derive(Default)]
enum State {
    #[default]
    Idle,
    /// The backend is on its way.
    Starting,
    /// `case` ran; its frame is on its way back.
    Running {
        backend: Box<Backend>,
        case: Case,
        readback: ReadbackId,
    },
    Failed(String),
}

/// The cases, in the order they run.
#[derive(Clone, Copy)]
enum Case {
    HostWrites,
    MemoryGrowth,
}

/// The check, which a task of its own starts.
#[derive(Clone, Default)]
pub(crate) struct GpuCheck(Rc<RefCell<State>>);

impl GpuCheck {
    /// Makes the backend and runs the first case on it, in a task of its
    /// own. Restarts a check that ran already.
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
                    let host = |bytes: &[u8]| js_sys::Uint8Array::from(bytes);
                    let ran = conformance::host_writes(&mut *backend.gpu(), host);
                    match ran {
                        Ok(readback) => State::Running { backend: Box::new(backend), case: Case::HostWrites, readback },
                        Err(error) => State::Failed(format!("the host writes case failed: {error}")),
                    }
                }
                Err(error) => State::Failed(format!("no GPU for the check: {error}")),
            };
            *check.0.borrow_mut() = state;
        });
    }

    /// Whether every case's frame came back and passed its checks; `false`
    /// until the last comes. Fails when no check started or the check
    /// failed, and panics where one of a frame's checks doesn't hold.
    pub(crate) fn take(&self) -> Result<bool, String> {
        let mut state = self.0.borrow_mut();
        match &mut *state {
            State::Idle => Err("no GPU check started".to_owned()),
            State::Starting => Ok(false),
            State::Failed(error) => Err(error.clone()),
            State::Running { backend, case, readback } => {
                let Some(pixels) = backend.gpu().take_readback(*readback, false).map_err(|e| e.to_string())? else {
                    return Ok(false);
                };
                match case {
                    Case::HostWrites => {
                        conformance::check_host_writes(&pixels);
                        match memory_growth(backend) {
                            Ok(next) => (*case, *readback) = (Case::MemoryGrowth, next),
                            Err(error) => *state = State::Failed(format!("the memory growth case failed: {error}")),
                        }
                        Ok(false)
                    }
                    Case::MemoryGrowth => {
                        conformance::check_memory_growth(&pixels);
                        *state = State::Idle;
                        Ok(true)
                    }
                }
            }
        }
    }

    fn drop_readback(&self) {
        if let State::Running { mut backend, readback, .. } = std::mem::take(&mut *self.0.borrow_mut()) {
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
    use mln_gfx::Size;
    let canvas = web_sys::OffscreenCanvas::new(1, 1).map_err(|error| Error::native(format!("{error:?}")))?;
    let (backend, _target) = mln_gl::CanvasTarget::with_backend(canvas, Size::new(1, 1))?;
    Ok(backend)
}

/// Runs `conformance::memory_growth` on `backend` with a [`Trap`] armed
/// before each of its calls, and returns the readback of its frame. Fails
/// when a call does, and when a trap wasn't one: the case then tested
/// nothing.
fn memory_growth(backend: &mut Backend) -> Result<ReadbackId, String> {
    let mut trap = Trap::new()?;
    let mut failed = None;
    let readback = conformance::memory_growth(&mut *backend.gpu(), |spare| {
        if failed.is_none() {
            failed = trap.arm(spare).err();
        }
    })
    .map_err(|error| error.to_string())?;
    trap.settle()?;
    failed.map_or(Ok(readback), Err)
}

/// The fewest slots the table has when a trap is armed: the vector that
/// grows with it is then larger than the heap's free memory can be after
/// [`Trap::arm`] filled it, which is less than [`FILL`] in one piece and
/// about a page of the memory at its end.
const TABLE: u32 = 1 << 14;

/// The size of the pieces of memory that fill the heap.
const FILL: usize = 16 << 10;

/// What makes the instance's memory grow as the instance takes a JS value.
///
/// wasm-bindgen keeps the instance's JS values in a table, and the table's
/// free slots in a vector that it grows with the table. A full table whose
/// vector the heap has no room to grow therefore grows the memory with the
/// next JS value, which detaches every view of the memory made before. A
/// typed array that code takes as a JS value and hands to the browser in a
/// later call is such a view.
struct Trap {
    table: js_sys::WebAssembly::Table,
    /// The JS values and the memory that fill the table and the heap.
    values: Vec<JsValue>,
    memory: Vec<Vec<u8>>,
    /// The slots the table had for JS values, free ones and taken, when it
    /// was this long, once that is known: from the first time it grew.
    slots: Option<(u32, u32)>,
    /// While armed: the table's length and the memory's pages as it was
    /// armed.
    armed: Option<(u32, usize)>,
}

impl Trap {
    /// Fails where the instance has no table of JS values: wasm-bindgen
    /// then keeps them in JavaScript, and taking one grows no memory.
    fn new() -> Result<Trap, String> {
        let table = js_sys::Reflect::get(&wasm_bindgen::exports(), &JsValue::from_str("__wbindgen_externrefs"))
            .ok()
            .and_then(|table| table.dyn_into().ok())
            .ok_or("the instance has no table of JS values to fill")?;
        Ok(Trap { table, values: Vec::new(), memory: Vec::new(), slots: None, armed: None })
    }

    /// How many JS values the table takes before it grows, once known.
    fn free(&self) -> Option<u32> {
        let (slots, length) = self.slots?;
        Some(slots + (self.table.length() - length) - wasm_bindgen::externref_heap_live_count())
    }

    /// Arms the trap: the instance's memory grows as it takes its next JS
    /// value after `spare` more. Fails as [`settle`](Self::settle) does for
    /// the trap armed before.
    fn arm(&mut self, spare: usize) -> Result<(), String> {
        self.settle()?;
        // The table, but for `spare` slots.
        while self.free() != u32::try_from(spare).ok() || self.table.length() < TABLE {
            let length = self.table.length();
            self.values.push(js_sys::Object::new().into());
            let grew = self.table.length() - length;
            if self.slots.is_none() && grew > 0 {
                // The value that grew the table took the first new slot.
                let slots = wasm_bindgen::externref_heap_live_count() + grew - 1;
                self.slots = Some((slots, self.table.length()));
            }
        }
        // The heap, until a piece grows the memory.
        loop {
            let pages = core::arch::wasm32::memory_size(0);
            self.memory.push(Vec::with_capacity(FILL));
            if core::arch::wasm32::memory_size(0) > pages {
                break;
            }
        }
        self.armed = Some((self.table.length(), core::arch::wasm32::memory_size(0)));
        Ok(())
    }

    /// Checks that the armed trap was one: that the memory grows with the
    /// table. Where the calls since it was armed took too few JS values to
    /// grow the table, or dropped ones they had, this takes them. Fails
    /// otherwise.
    fn settle(&mut self) -> Result<(), String> {
        let Some((length, pages)) = self.armed.take() else {
            return Ok(());
        };
        let mut taken = Vec::new();
        while self.table.length() == length {
            if taken.len() as u32 > TABLE {
                return Err("the table of JS values doesn't grow".to_owned());
            }
            taken.push(JsValue::from(js_sys::Object::new()));
        }
        if core::arch::wasm32::memory_size(0) == pages {
            return Err("the table of JS values grew without the memory: the heap had room".to_owned());
        }
        Ok(())
    }
}
