//! Where a player's frames wait for the renderers that draw them.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mln_gfx::video::VideoFrame;
use mln_util::event_queue::Waker;
use parking_lot::Mutex;

/// The newest frame of one video source, and how its player fares.
///
/// A player pushes frames through its [`FrameSink`]; each replaces the one
/// before, which renderers that didn't draw it skip. Renderers read the
/// newest frame, which stays in the slot, so a renderer made later still
/// draws a paused video. The slot's waker runs on the player's thread for
/// each frame and failure, to ask the map for a new frame.
#[derive(Default)]
pub struct FrameSlot {
    state: Mutex<State>,
    waker: Mutex<Option<Waker>>,
    gpu_frames_refused: AtomicBool,
}

#[derive(Default)]
struct State {
    frame: Option<Arc<VideoFrame>>,
    /// Counts the frames pushed.
    generation: u64,
    failure: Option<String>,
}

impl std::fmt::Debug for FrameSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.state.lock();
        f.debug_struct("FrameSlot")
            .field("generation", &state.generation)
            .field("failure", &state.failure)
            .finish_non_exhaustive()
    }
}

/// The newest frame of a slot, and the count of frames before and with it.
#[derive(Clone, Debug)]
pub struct Newest {
    pub frame: Arc<VideoFrame>,
    pub generation: u64,
}

impl FrameSlot {
    #[must_use]
    pub fn new() -> Arc<FrameSlot> {
        Arc::new(FrameSlot::default())
    }

    /// Sets what runs when a frame or a failure arrives.
    pub fn set_waker(&self, waker: Option<Waker>) {
        *self.waker.lock() = waker;
    }

    /// The newest frame, if it isn't the one of `seen`, a generation that
    /// [`newest`](Self::newest) returned before.
    #[must_use]
    pub fn newer_than(&self, seen: u64) -> Option<Newest> {
        let state = self.state.lock();
        let frame = state.frame.as_ref().filter(|_| state.generation != seen)?;
        Some(Newest { frame: Arc::clone(frame), generation: state.generation })
    }

    /// The newest frame, if a frame came.
    #[must_use]
    pub fn newest(&self) -> Option<Newest> {
        self.newer_than(0)
    }

    /// Why the video doesn't play, once its player failed.
    #[must_use]
    pub fn failure(&self) -> Option<String> {
        self.state.lock().failure.clone()
    }

    /// Whether a frame or a failure came: a still render waits until then.
    #[must_use]
    pub fn is_settled(&self) -> bool {
        let state = self.state.lock();
        state.frame.is_some() || state.failure.is_some()
    }

    /// Tells the player to send frames in memory from now on: a renderer
    /// couldn't import one of its GPU frames.
    pub fn refuse_gpu_frames(&self) {
        if !self.gpu_frames_refused.swap(true, Ordering::Relaxed) {
            self.wake();
        }
    }

    /// Whether a renderer refused the player's GPU frames.
    #[must_use]
    pub fn gpu_frames_refused(&self) -> bool {
        self.gpu_frames_refused.load(Ordering::Relaxed)
    }

    fn push(&self, frame: VideoFrame) {
        {
            let mut state = self.state.lock();
            state.frame = Some(Arc::new(frame));
            state.generation += 1;
        }
        self.wake();
    }

    fn fail(&self, message: String) {
        {
            let mut state = self.state.lock();
            if state.failure.is_some() {
                return;
            }
            state.failure = Some(message);
        }
        self.wake();
    }

    fn wake(&self) {
        let waker = self.waker.lock().clone();
        if let Some(waker) = waker {
            waker();
        }
    }
}

/// A player's handle on its source's [`FrameSlot`].
#[derive(Clone, Debug)]
pub struct FrameSink {
    slot: Arc<FrameSlot>,
}

impl FrameSink {
    #[must_use]
    pub fn new(slot: Arc<FrameSlot>) -> FrameSink {
        FrameSink { slot }
    }

    /// Hands the map the frame that the video shows now.
    pub fn push(&self, frame: VideoFrame) {
        self.slot.push(frame);
    }

    /// Tells the map that the video can't play, and why: none of its URLs
    /// plays, or the player broke. The source draws nothing from then on,
    /// and only the first failure counts.
    pub fn fail(&self, message: impl Into<String>) {
        self.slot.fail(message.into());
    }

    /// Whether the player should send frames in memory: a renderer couldn't
    /// import its GPU frames.
    #[must_use]
    pub fn gpu_frames_refused(&self) -> bool {
        self.slot.gpu_frames_refused()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use mln_gfx::video::{CpuPlane, FramePixels};
    use mln_util::size::Size;

    use super::*;

    fn frame() -> VideoFrame {
        VideoFrame {
            size: Size::new(1, 1),
            pixels: FramePixels::Rgba(CpuPlane { data: vec![0; 4], stride: 4 }),
            time: None,
        }
    }

    #[test]
    fn readers_see_each_new_frame_once_and_frames_wake_the_map() {
        let slot = FrameSlot::new();
        let wakes = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&wakes);
        slot.set_waker(Some(Arc::new(move || {
            counted.fetch_add(1, Ordering::Relaxed);
        })));
        let sink = FrameSink::new(Arc::clone(&slot));
        assert!(slot.newest().is_none());
        assert!(!slot.is_settled());

        sink.push(frame());
        sink.push(frame());
        let newest = slot.newest().unwrap();
        assert_eq!(2, newest.generation);
        assert!(slot.newer_than(newest.generation).is_none());
        assert!(slot.is_settled());
        assert_eq!(2, wakes.load(Ordering::Relaxed));
    }

    #[test]
    fn the_first_failure_counts() {
        let slot = FrameSlot::new();
        let sink = FrameSink::new(Arc::clone(&slot));
        sink.fail("no codec");
        sink.fail("later");
        assert_eq!(Some("no codec".to_owned()), slot.failure());
        assert!(slot.is_settled());
    }
}
