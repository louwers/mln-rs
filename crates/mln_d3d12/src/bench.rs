//! How long the GPU took, for the benchmarks (`tools/bench`) to measure:
//! the time from each render pass's beginning to its end, summed over every
//! pass, from timestamp queries. Measured once a process calls
//! [`measure_render_passes`], on the backends made after. A build with the
//! feature in which no one calls it, such as a workspace's tests, measures
//! nothing. Built only with the `bench` feature, and hidden from the
//! documentation: it isn't API.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_HEAP_FLAG_NONE, D3D12_HEAP_PROPERTIES, D3D12_HEAP_TYPE_READBACK, D3D12_QUERY_HEAP_DESC,
    D3D12_QUERY_HEAP_TYPE_TIMESTAMP, D3D12_QUERY_TYPE_TIMESTAMP, D3D12_RANGE, D3D12_RESOURCE_DESC,
    D3D12_RESOURCE_DIMENSION_BUFFER, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
    ID3D12CommandQueue, ID3D12Device, ID3D12GraphicsCommandList, ID3D12QueryHeap, ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;

use crate::memory::NO_RANGE;

/// The passes one submission times at most; a pass past them goes untimed.
const PASSES: u32 = 1024;

/// Whether the backends made now time their render passes.
static COUNTING: AtomicBool = AtomicBool::new(false);

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
/// backend times its passes.
#[must_use]
pub fn take_gpu_time() -> Option<Duration> {
    TIME.lock().as_mut().map(std::mem::take)
}

/// A submission's queries: the beginning and end of each of its passes,
/// and the readback memory they resolve into.
struct Queries {
    heap: ID3D12QueryHeap,
    readback: ID3D12Resource,
}

/// A backend's timestamp queries: a set for each submission in flight.
pub(crate) struct Timer {
    /// The nanoseconds of one tick.
    period: f64,
    /// The queries of the batch being recorded, and the passes it has
    /// begun.
    recording: Option<(Queries, u32)>,
    /// The queries of submissions, by serial, and how many passes each
    /// timed.
    in_flight: Vec<(u64, Queries, u32)>,
    free: Vec<Queries>,
}

impl Timer {
    /// The timer of a backend on `queue`, if the process measures and the
    /// queue has a timestamp frequency.
    pub(crate) fn new(queue: &ID3D12CommandQueue) -> Option<Timer> {
        if !COUNTING.load(Ordering::Relaxed) {
            return None;
        }
        // SAFETY: a plain query.
        let frequency = unsafe { queue.GetTimestampFrequency() }.ok()?;
        if frequency == 0 {
            return None;
        }
        TIME.lock().get_or_insert_default();
        Some(Timer { period: 1e9 / frequency as f64, recording: None, in_flight: Vec::new(), free: Vec::new() })
    }

    /// Writes the beginning of a pass that `list` is about to begin, taking
    /// queries for the batch if it has none.
    pub(crate) fn begin(&mut self, device: &ID3D12Device, list: &ID3D12GraphicsCommandList) {
        if self.recording.is_none() {
            let queries = match self.free.pop() {
                Some(queries) => Some(queries),
                None => create_queries(device),
            };
            self.recording = queries.map(|queries| (queries, 0));
        }
        if let Some((queries, passes)) = self.recording.as_ref().filter(|(_, passes)| *passes < PASSES) {
            // SAFETY: the heap lives until the batch has finished.
            unsafe { list.EndQuery(&queries.heap, D3D12_QUERY_TYPE_TIMESTAMP, 2 * passes) };
        }
    }

    /// Writes the end of the pass that `list` ended last.
    pub(crate) fn end(&mut self, list: &ID3D12GraphicsCommandList) {
        if let Some((queries, passes)) = &mut self.recording
            && *passes < PASSES
        {
            // SAFETY: as in `begin`.
            unsafe { list.EndQuery(&queries.heap, D3D12_QUERY_TYPE_TIMESTAMP, 2 * *passes + 1) };
            *passes += 1;
        }
    }

    /// Resolves the batch's timestamps into its readback memory at the end
    /// of `list`, outside a pass, and holds them for the submit of
    /// `serial`.
    pub(crate) fn submit(&mut self, list: &ID3D12GraphicsCommandList, serial: u64) {
        if let Some((queries, passes)) = self.recording.take() {
            if passes > 0 {
                // SAFETY: each query was written in the batch, and the
                // readback memory holds them all.
                unsafe {
                    list.ResolveQueryData(
                        &queries.heap,
                        D3D12_QUERY_TYPE_TIMESTAMP,
                        0,
                        2 * passes,
                        &queries.readback,
                        0,
                    );
                }
            }
            self.in_flight.push((serial, queries, passes));
        }
    }

    /// Adds up the passes of the submits up to `serial`, which have
    /// finished.
    pub(crate) fn finished(&mut self, serial: u64) {
        let (done, waiting): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.in_flight).into_iter().partition(|(submitted, ..)| *submitted <= serial);
        self.in_flight = waiting;
        let mut time = Duration::ZERO;
        for (_, queries, passes) in done {
            let count = 2 * passes as usize;
            let range = D3D12_RANGE { Begin: 0, End: count * size_of::<u64>() };
            let mut data = std::ptr::null_mut();
            // SAFETY: the batch has finished, which resolved its timestamps
            // into the mapped range.
            unsafe {
                if count > 0 && queries.readback.Map(0, Some(&raw const range), Some(&raw mut data)).is_ok() {
                    let values = std::slice::from_raw_parts(data.cast::<u64>(), count);
                    let ticks: u64 =
                        values.as_chunks::<2>().0.iter().map(|[begin, end]| end.saturating_sub(*begin)).sum();
                    time += Duration::from_nanos((ticks as f64 * self.period).round() as u64);
                    queries.readback.Unmap(0, Some(&raw const NO_RANGE));
                }
            }
            self.free.push(queries);
        }
        if let Some(total) = TIME.lock().as_mut() {
            *total += time;
        }
    }
}

/// A query heap of a submission's timestamps, and readback memory for them.
fn create_queries(device: &ID3D12Device) -> Option<Queries> {
    let desc = D3D12_QUERY_HEAP_DESC { Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP, Count: 2 * PASSES, NodeMask: 0 };
    let mut heap: Option<ID3D12QueryHeap> = None;
    // SAFETY: the description outlives the call, which fills `heap`.
    unsafe { device.CreateQueryHeap(&raw const desc, &raw mut heap) }.ok()?;
    let properties = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_READBACK, ..Default::default() };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Alignment: 0,
        Width: u64::from(2 * PASSES) * size_of::<u64>() as u64,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    };
    let mut readback: Option<ID3D12Resource> = None;
    // SAFETY: as above.
    unsafe {
        device.CreateCommittedResource(
            &raw const properties,
            D3D12_HEAP_FLAG_NONE,
            &raw const desc,
            D3D12_RESOURCE_STATE_COPY_DEST,
            None,
            &raw mut readback,
        )
    }
    .ok()?;
    Some(Queries { heap: heap?, readback: readback? })
}
