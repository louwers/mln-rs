//! Repro for maplibre/mln-rs#1071: renders a style at the issue's cameras on
//! one backend. Usage: repro_1071 <style.json> <metal|gl|vulkan|webgpu> <out dir>

use mln_engine::camera::{CameraOptions, CameraUpdate};
use mln_engine::map::MapMode;
use mln_runtime::*;
use mln_style::geo::{LatLng, LatLngBounds};
use mln_style::size::Size;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let style = std::fs::read_to_string(&args[1]).unwrap();
    let backend = match args[2].as_str() {
        "metal" => BackendType::Metal,
        "gl" => BackendType::OpenGL,
        "vulkan" => BackendType::Vulkan,
        "webgpu" => BackendType::WebGPU,
        other => panic!("{other}"),
    };
    let out = std::path::PathBuf::from(&args[3]);
    std::fs::create_dir_all(&out).unwrap();
    let runtime = Runtime::new(&RuntimeOptions::default()).unwrap();

    enum Cam {
        Center(f64, f64, f64),
        Bounds(f64, f64, f64, f64),
    }
    let cases = [
        ("world-z0", MapMode::Tile, 512, Cam::Center(0.0, 0.0, 0.0)),
        ("europe-bbox", MapMode::Static, 200, Cam::Bounds(-10.0, 40.0, 30.0, 60.0)),
        ("gulf-of-guinea-z2", MapMode::Static, 200, Cam::Center(0.0, 0.0, 2.0)),
    ];
    for (name, mode, px, cam) in cases {
        let size = Size::new(px, px);
        let options =
            MapOptions { map: mln_engine::map::MapOptions { size, map_mode: mode, ..Default::default() }, ..Default::default() };
        let (map, created) = runtime.create_map(options).unwrap();
        created.wait().unwrap();
        map.set_style_json(style.clone()).unwrap().wait().unwrap();
        match cam {
            Cam::Center(lng, lat, zoom) => {
                map.update_camera(CameraUpdate {
                    camera: CameraOptions {
                        center: Some(LatLng::new(lat, lng).unwrap()),
                        zoom: Some(zoom),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .unwrap()
                .wait()
                .unwrap();
            }
            Cam::Bounds(w, s, e, n) => {
                let bounds = LatLngBounds::hull(LatLng::new(s, w).unwrap(), LatLng::new(n, e).unwrap());
                map.fit_camera(CameraFitArea::Bounds(bounds), CameraFitUpdate::default()).unwrap().wait().unwrap();
            }
        }
        let device = Device::with_backend(backend, DeviceOptions::default()).unwrap();
        let (session, attached) =
            map.attach(RenderTarget::Offscreen { size, device: Some(device) }, AttachOptions::default()).unwrap();
        attached.wait().unwrap();
        map.request_still_image().unwrap().wait().unwrap();
        let image = session.read_texture().unwrap().wait().unwrap();
        let path = out.join(format!("{name}-{}.png", args[2]));
        std::fs::write(&path, mln_engine::image::encode_png(&image)).unwrap();
        println!("{}", path.display());
        session.release().unwrap().wait().unwrap();
        map.release().unwrap().wait().unwrap();
    }
}
