// A frame whose symbols wait for a placement that isn't due yet asks for no
// update, and the runtime repaints the map once the placement is due.

use mln_map::{CameraOptions, CameraUpdate, RenderMode};
use mln_runtime::{AttachOptions, DemandMode, FrameDemand, FrameDisposition, FrameResult, RenderSession, RenderTarget};
use mln_style::bitmap::PremultipliedImage;
use mln_style::geo::LatLng;
use mln_style::size::Size;

use crate::support::{eventually, map, runtime, wait};

const STYLE: &str = r#"{"version":8,"sources":{"points":{"type":"geojson","data":{"type":"Feature","properties":{},"geometry":{"type":"Point","coordinates":[0,0]}}}},"layers":[{"id":"points","type":"symbol","source":"points","layout":{"icon-image":"marker"}}]}"#;

/// Demands a frame and waits for its result.
fn demand(session: &RenderSession, mode: DemandMode) -> FrameResult {
    session.demand_frame(FrameDemand::new(mode, 0)).unwrap();
    let mut results = Vec::new();
    assert!(eventually(|| {
        results.extend(session.drain_frame_results().unwrap());
        !results.is_empty()
    }));
    results[0]
}

#[test]
fn the_runtime_repaints_a_map_whose_symbols_wait_for_a_placement() {
    let runtime = runtime();
    let map = map(&runtime);
    wait(map.set_style_json(STYLE));
    let marker = PremultipliedImage::from_vec(Size::new(16, 16), vec![255; 16 * 16 * 4]);
    let marker = mln_style::image::Impl::new("marker".to_owned(), marker, 1.0, false, vec![], vec![], None, None, None);
    wait(map.set_style_image(marker.unwrap()));
    let target = RenderTarget::Offscreen { size: Size::new(64, 64), device: None };
    let (session, attached) = map.attach(target, AttachOptions::default()).unwrap();
    attached.wait().unwrap();

    // Each settled map has just placed its symbols, so a camera move that
    // follows at once waits for the next placement; a slow machine may
    // place at once instead, and tries again.
    let mut longitude = 0.0;
    let waited = (0..5).find_map(|_| {
        assert!(eventually(|| {
            let frame = demand(&session, DemandMode::Force);
            frame.mode == RenderMode::Full && !frame.needs_repaint
        }));
        longitude += 0.001;
        let center = LatLng::new(0.0, longitude).unwrap();
        wait(map.update_camera(CameraUpdate {
            camera: CameraOptions { center: Some(center), ..Default::default() },
            ..CameraUpdate::default()
        }));
        let moved = demand(&session, DemandMode::WaitForUpdate);
        moved.needs_repaint.then_some(moved)
    });
    let moved = waited.expect("a camera move whose symbols wait for a placement");

    // Nothing else changes the map: the runtime's repaint draws the
    // placement.
    let placed = demand(&session, DemandMode::WaitForUpdate);
    assert_eq!(placed.disposition, FrameDisposition::Rendered);
    assert!(placed.map_update_generation > moved.map_update_generation);
    assert!(!placed.needs_repaint);
}
