//! The players of a style's video sources: the map opens one for each
//! video source of its style, and drops it with the source.

use std::sync::Arc;

use mln_actor::{AsyncTask, RunLoop};
use mln_renderer::{RendererEvent, RendererEvents};
use mln_storage::FileSource;
use mln_style::Style;
use mln_style::source::Source;
use mln_style::sources::VideoSource;
use mln_util::error::Error;
use mln_util::hash::HashMap;
use mln_util::logging::{Event, Log};
use mln_video::{FrameSink, FrameSlot, OpenRequest, VideoPlayer, VideoPlayers};

/// What a video source's player does next.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VideoControl {
    Play,
    Pause,
    /// Moves to a time in seconds from the video's start.
    Seek(f64),
}

/// The players that the map opened, by source ID.
#[derive(Default)]
pub(crate) struct Videos {
    players: Option<Arc<dyn VideoPlayers>>,
    open: HashMap<String, OpenVideo>,
}

struct OpenVideo {
    frames: Arc<FrameSlot>,
    /// Runs on the map's loop for the player's frames and failures, which
    /// come from the player's threads.
    _wake: AsyncTask,
    /// `None` when it failed to open.
    player: Option<Box<dyn VideoPlayer>>,
    failure_reported: bool,
}

impl Drop for OpenVideo {
    fn drop(&mut self) {
        self.frames.set_waker(None);
    }
}

impl Videos {
    /// Opens videos with `players` from now on, and those open with the
    /// players before again.
    pub(crate) fn set_players(&mut self, players: Option<Arc<dyn VideoPlayers>>) {
        self.players = players;
        self.open.clear();
    }

    /// Opens a player for each video source of `style` that has none, and
    /// drops those of sources gone. A player's frames and failures push an
    /// invalidation to `events` on `run_loop`, the map's, which brings the
    /// map back here. Returns the failures of videos that no earlier call
    /// returned.
    pub(crate) fn sync(
        &mut self,
        style: &Style,
        file_source: Option<&Arc<dyn FileSource>>,
        events: &RendererEvents,
        run_loop: &RunLoop,
    ) -> Vec<Error> {
        let sources: Vec<(String, Arc<FrameSlot>, Vec<String>)> = style
            .get_sources()
            .into_iter()
            .filter_map(|source| {
                let video = source.downcast_ref::<VideoSource>()?;
                let impl_ = video.impl_();
                Some((video.get_id(), Arc::clone(impl_.frames()), impl_.get_urls().to_vec()))
            })
            .collect();
        self.open.retain(|id, open| {
            sources.iter().any(|(source, frames, _)| source == id && Arc::ptr_eq(frames, &open.frames))
        });
        for (id, frames, urls) in sources {
            if self.open.contains_key(&id) {
                continue;
            }
            let events = events.clone();
            let wake = AsyncTask::new(run_loop, move || events.push(RendererEvent::Invalidate));
            let sender = wake.sender();
            frames.set_waker(Some(Arc::new(move || sender.send())));
            let sink = FrameSink::new(Arc::clone(&frames));
            let request = OpenRequest { urls, file_source: file_source.cloned(), frames: sink.clone() };
            let player = if let Some(players) = &self.players {
                players.open(request).map_err(|error| sink.fail(error)).ok()
            } else {
                sink.fail("the map has no video players");
                None
            };
            self.open.insert(id, OpenVideo { frames, _wake: wake, player, failure_reported: false });
        }
        let mut failures = Vec::new();
        for (id, open) in &mut self.open {
            if open.failure_reported {
                continue;
            }
            if let Some(failure) = open.frames.failure() {
                open.failure_reported = true;
                let message = format!("Video source \"{id}\" doesn't play: {failure}");
                Log::warning(Event::Style, message.clone());
                failures.push(Error::native(message));
            }
        }
        failures
    }

    /// Steers the player of the video source `id`.
    pub(crate) fn control(&mut self, id: &str, control: VideoControl) -> Result<(), Error> {
        let open = self
            .open
            .get_mut(id)
            .ok_or_else(|| Error::invalid_argument(format!("no video source \"{id}\" is open")))?;
        if let Some(player) = &mut open.player {
            match control {
                VideoControl::Play => player.play(),
                VideoControl::Pause => player.pause(),
                VideoControl::Seek(seconds) => player.seek(seconds),
            }
        }
        Ok(())
    }
}
