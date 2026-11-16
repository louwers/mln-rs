//! The runner's video player, which shows a video source's first URL as a
//! still image: the suite's video tests compare a frame of the video, which
//! the runner has as an image.

use std::sync::Arc;

use mln_storage::{Request, Resource};
use mln_video::{CpuPlane, FramePixels, OpenRequest, VideoFrame, VideoPlayer, VideoPlayers};

/// Opens each video as its first frame, which never changes.
#[derive(Debug)]
pub(crate) struct StillVideos;

/// A still video, until its image arrives.
struct Still {
    _request: Request,
}

impl VideoPlayer for Still {
    fn play(&mut self) {}
    fn pause(&mut self) {}
    fn seek(&mut self, _seconds: f64) {}
}

/// The URL of the image that is the frame a test compares: `video/default`
/// names `local://video/0.png`, which the suite's cache lacks. It is the
/// still of `local://image/0.png`, which the image tests draw.
fn still_url(url: &str) -> &str {
    if url == "local://video/0.png" { "local://image/0.png" } else { url }
}

impl VideoPlayers for StillVideos {
    fn open(&self, request: OpenRequest) -> Result<Box<dyn VideoPlayer>, String> {
        let file_source = request.file_source.ok_or("the map has no file source")?;
        let url = request.urls.first().ok_or("the video has no URLs")?;
        let frames = request.frames;
        let (pending, responder) = Request::direct_once(move |response| {
            let image = match (response.error, response.data) {
                (Some(error), _) => Err(error.message),
                (None, Some(data)) => mln_image::decode_image(&data).map_err(|error| error.message),
                (None, None) => Err("an empty response".to_owned()),
            };
            match image {
                Ok(image) => frames.push(VideoFrame {
                    size: image.size,
                    pixels: FramePixels::Rgba(CpuPlane { stride: image.size.width as usize * 4, data: image.data }),
                    time: Some(0.0),
                }),
                Err(error) => frames.fail(error),
            }
        });
        file_source.load(&Resource::image(still_url(url)), responder);
        Ok(Box::new(Still { _request: pending }))
    }
}

/// The players of the runner's maps.
pub(crate) fn players() -> Arc<dyn VideoPlayers> {
    Arc::new(StillVideos)
}
