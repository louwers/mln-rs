//! How long the GPU took, for the benchmarks (`tools/bench`) to measure:
//! the time GL took for the commands of each render pass, summed over every
//! pass, from `GL_TIME_ELAPSED` queries. Measured once a process calls
//! [`measure_render_passes`], on the backends made after, where the context
//! has the queries: desktop OpenGL 3.3, `ARB_timer_query`, or GL ES's
//! `EXT_disjoint_timer_query`, and on a GPU: Mesa's CPU rasterizers report
//! times that don't add up to the drawing they do. A build with the feature
//! in which no one calls it, such as a workspace's tests, measures nothing.
//! Built only with the `bench` feature, and hidden from the documentation:
//! it isn't API.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use glow::HasContext;
use parking_lot::Mutex;

use crate::types::Gl;

/// `GL_GPU_DISJOINT_EXT`: whether something, such as a change of the GPU's
/// clocks, made the times measured since it was last read meaningless.
const GPU_DISJOINT: u32 = 0x8FBB;

/// Whether the backends made now time their render passes.
static COUNTING: AtomicBool = AtomicBool::new(false);

/// Whether GL reported a disjoint operation, which leaves the time of the
/// passes it ran then unknown.
static DISJOINT: AtomicBool = AtomicBool::new(false);

/// The time of the passes the GPU has finished that nobody has taken yet;
/// `None` until a backend times its passes.
static TIME: Mutex<Option<Duration>> = Mutex::new(None);

/// Times the render passes of the backends made from now on, for
/// [`take_gpu_time`] to take after each frame.
pub fn measure_render_passes() {
    COUNTING.store(true, Ordering::Relaxed);
}

/// Leaves the render passes of the backends made from now on untimed,
/// where no one would read the time and the queries would only cost work.
pub fn leave_render_passes_untimed() {
    COUNTING.store(false, Ordering::Relaxed);
}

/// The time the GPU spent in the render passes it has finished since the
/// last call, once the backend has waited for them, or `None` where no
/// backend times its passes, and from when GL first reported a disjoint
/// operation on, such as a change of the GPU's clocks, which leaves the
/// time unknown.
#[must_use]
pub fn take_gpu_time() -> Option<Duration> {
    let mut time = TIME.lock();
    if DISJOINT.load(Ordering::Relaxed) {
        return None;
    }
    time.as_mut().map(std::mem::take)
}

/// A backend's time queries, one for each pass whose time hasn't come back.
pub(crate) struct Timer {
    /// Whether the context reports disjoint operations (GL ES).
    disjoint: bool,
    /// The query of the open pass.
    open: Option<glow::Query>,
    /// The queries of ended passes, in order.
    pending: Vec<glow::Query>,
    free: Vec<glow::Query>,
}

impl Timer {
    /// The timer of a backend on `gl`, which is current, if the process
    /// measures and the context has time queries, unless it draws on the
    /// CPU (`cpu_rasterizer`).
    pub(crate) fn new(gl: &Gl, cpu_rasterizer: bool) -> Option<Timer> {
        if !COUNTING.load(Ordering::Relaxed) || cpu_rasterizer {
            return None;
        }
        let version = gl.version();
        let extensions = gl.supported_extensions();
        let has = |name: &str| extensions.contains(name) || extensions.contains(&format!("GL_{name}"));
        let disjoint = has("EXT_disjoint_timer_query");
        let core = !version.is_embedded && (version.major, version.minor) >= (3, 3);
        if !core && !has("ARB_timer_query") && !disjoint {
            return None;
        }
        TIME.lock().get_or_insert_default();
        Some(Timer { disjoint, open: None, pending: Vec::new(), free: Vec::new() })
    }

    /// Starts timing a pass, which [`end`](Self::end) stops.
    pub(crate) fn begin(&mut self, gl: &Gl) {
        if self.open.is_some() {
            return;
        }
        // SAFETY: the context is current, and no time query is active.
        let query = self.free.pop().or_else(|| unsafe { gl.create_query() }.ok());
        if let Some(query) = query {
            // SAFETY: as above.
            unsafe { gl.begin_query(glow::TIME_ELAPSED, query) };
            self.open = Some(query);
        }
    }

    pub(crate) fn end(&mut self, gl: &Gl) {
        if let Some(query) = self.open.take() {
            // SAFETY: the context is current, and the query is active.
            unsafe { gl.end_query(glow::TIME_ELAPSED) };
            self.pending.push(query);
        }
    }

    /// Adds up the passes whose times have come back, which they all have
    /// once GL has finished the commands so far, as after a flush that waits
    /// or a readback.
    pub(crate) fn collect(&mut self, gl: &Gl) {
        let mut time = Duration::ZERO;
        // SAFETY: the context is current, and the queries have ended. A
        // query's result is available once those before it are.
        let ready = self
            .pending
            .iter()
            .take_while(|&&query| unsafe { gl.get_query_parameter_u32(query, glow::QUERY_RESULT_AVAILABLE) } != 0)
            .count();
        for query in self.pending.drain(..ready) {
            // SAFETY: as above.
            time += Duration::from_nanos(unsafe { gl.get_query_parameter_u64(query, glow::QUERY_RESULT) });
            self.free.push(query);
        }
        // SAFETY: the context is current, and has the parameter where it
        // reports disjoint operations. Reading it clears it, though queries
        // still pending may be affected too.
        if self.disjoint && unsafe { gl.get_parameter_i32(GPU_DISJOINT) } != 0 {
            DISJOINT.store(true, Ordering::Relaxed);
        }
        if let Some(total) = TIME.lock().as_mut() {
            *total += time;
        }
    }

    /// Deletes the queries, with the context current.
    pub(crate) fn destroy(&mut self, gl: &Gl) {
        if self.open.is_some() {
            self.end(gl);
        }
        for query in self.pending.drain(..).chain(self.free.drain(..)) {
            // SAFETY: the context is current, and nothing uses the query.
            unsafe { gl.delete_query(query) };
        }
    }
}
