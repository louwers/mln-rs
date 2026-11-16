// The platform's video player on Linux, GStreamer, through Map ->
// HeadlessFrontend: the map plays mln_video's red-then-green-then-blue VP9
// fixture over the whole view, and the still shows its first frame, red.
// Where GStreamer isn't installed the test passes, saying it was skipped.
// With a VA-API decoder the frames come as dmabufs, which the backends that
// import them draw without a copy through memory: with
// MLN_VIDEO_EXPECT_GPU_FRAMES set, the test fails if the renderer refused
// them.
#![cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android")))]

use std::sync::Arc;

use mln_actor::RunLoop;
use mln_headless::HeadlessFrontend;
use mln_map::{Map, MapMode, MapOptions};
use mln_storage::response::Reason;
use mln_storage::{FileSource, Resource, ResourceError, Responder, Response};
use mln_style::sources::VideoSource;
use mln_util::size::Size;

const VP9: &[u8] = include_bytes!("../../mln_video/tests/fixtures/colors-vp9.webm");

/// Serves the fixture at `fixture://colors.webm`, as a local file source
/// does: ranges padded with zeros, the size known.
struct Fixture;

impl FileSource for Fixture {
    fn load(&self, resource: &Resource, responder: Responder) {
        let response = match (resource.url.as_str(), resource.data_range) {
            ("fixture://colors.webm", Some((first, last))) => {
                let mut data: Vec<u8> =
                    VP9.iter().copied().skip(first as usize).take((last - first + 1) as usize).collect();
                data.resize((last - first + 1) as usize, 0);
                Response { data: Some(Arc::new(data)), total_size: Some(VP9.len() as u64), ..Response::default() }
            }
            _ => Response {
                error: Some(Box::new(ResourceError::new(Reason::NotFound, "not found"))),
                ..Response::default()
            },
        };
        std::thread::spawn(move || responder.send(response));
    }

    fn can_request(&self, _resource: &Resource) -> bool {
        true
    }
}

fn gstreamer_installed() -> bool {
    std::process::Command::new("gst-inspect-1.0").arg("--version").output().is_ok_and(|out| out.status.success())
}

fn va_decoder_installed() -> bool {
    std::process::Command::new("gst-inspect-1.0").arg("vavp9dec").output().is_ok_and(|out| out.status.success())
}

#[test]
fn gstreamer_plays_a_video_source() {
    if !gstreamer_installed() {
        eprintln!("skipped: GStreamer isn't installed");
        return;
    }
    let size = Size::new(512, 512);
    let run_loop = RunLoop::new();
    let mut frontend = HeadlessFrontend::new(size, 1.0, &run_loop).expect("a headless frontend");
    let mut map = Map::with_file_source(
        Arc::new(Fixture),
        MapOptions { map_mode: MapMode::Static, size, ..Default::default() },
        &run_loop,
    )
    .unwrap();
    map.set_video_players(Some(mln_video::platform_players()));
    frontend.attach(&map);
    map.style_mut().load_json(
        r#"{
          "version": 8,
          "sources": {
            "video": {
              "type": "video",
              "urls": ["fixture://colors.webm"],
              "coordinates": [[-180, 85], [180, 85], [180, -85], [-180, -85]]
            }
          },
          "layers": [{"id": "video", "type": "raster", "source": "video", "paint": {"raster-fade-duration": 0}}]
        }"#,
    );
    let camera = map.style().get_default_camera();
    map.jump_to(&camera);
    let image = frontend.render(&mut map).expect("render").image;
    let center = ((size.height / 2 * size.width + size.width / 2) * 4) as usize;
    let pixel = &image.data[center..center + 4];
    assert!(pixel[0] > 200 && pixel[1] < 60 && pixel[2] < 60 && pixel[3] == 255, "red, not {pixel:?}");

    let frames = map
        .style()
        .get_source("video")
        .and_then(|source| source.downcast_ref::<VideoSource>())
        .map(|video| Arc::clone(video.impl_().frames()));
    let refused = frames.is_some_and(|frames| frames.gpu_frames_refused());
    let backend = std::env::var("MLN_RENDER_BACKEND").unwrap_or_default();
    eprintln!("{backend}: GPU frames refused: {refused}");
    // A renderer on another GPU than the decoder's, such as lavapipe, may
    // not import its dmabufs and draws the frames from memory instead: the
    // check that it imports them is for a machine whose decoder and
    // renderer share a GPU.
    if std::env::var_os("MLN_VIDEO_EXPECT_GPU_FRAMES").is_some() && va_decoder_installed() {
        assert!(!refused, "{backend} imports VA-API's dmabufs");
    }
}
