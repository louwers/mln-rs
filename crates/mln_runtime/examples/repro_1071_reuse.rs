//! Repro for maplibre/mln-rs#1071 as Martin renders: worker threads, each
//! with a device and one map per mode that it reuses for every image.
//! Renders each camera on several workers and rounds, and reports images of
//! the same camera that differ. Usage:
//! repro_1071_reuse <style.json> <metal|webgpu> <out dir> [workers] [rounds]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mln_engine::camera::{CameraOptions, CameraUpdate, CameraUpdateMode};
use mln_engine::map::MapMode;
use mln_runtime::*;
use mln_style::geo::LatLng;
use mln_style::size::Size;

#[derive(Clone, Copy)]
struct Shot {
    mode: MapMode,
    px: u32,
    lng: f64,
    lat: f64,
    zoom: f64,
}

fn shots() -> Vec<(String, Shot)> {
    let mut out = Vec::new();
    // Every tile of z0..=2, as the tile endpoint would draw them.
    for z in 0..=2u32 {
        let n = 1u32 << z;
        for x in 0..n {
            for y in 0..n {
                let lng = (x as f64 + 0.5) / n as f64 * 360.0 - 180.0;
                let t = std::f64::consts::PI * (1.0 - 2.0 * (y as f64 + 0.5) / n as f64);
                let lat = t.sinh().atan().to_degrees();
                out.push((format!("tile-{z}-{x}-{y}"), Shot { mode: MapMode::Tile, px: 512, lng, lat, zoom: z as f64 }));
            }
        }
    }
    for (name, lng, lat, zoom) in
        [("static-guinea-z2", 0.0, 0.0, 2.0), ("static-europe-z3", 10.0, 50.0, 3.0), ("static-world-z0", 0.0, 0.0, 0.0)]
    {
        out.push((name.into(), Shot { mode: MapMode::Static, px: 200, lng, lat, zoom }));
    }
    out
}

struct Renderer {
    map: Map,
    session: RenderSession,
    size: Size,
}

impl Renderer {
    fn new(runtime: &Runtime, device: &Device, style: &str, mode: MapMode, size: Size) -> Renderer {
        let mut options = MapOptions { system_fonts: false, ..Default::default() };
        options.map.map_mode = mode;
        options.map.size = size;
        let (map, created) = runtime.create_map(options).unwrap();
        created.wait().unwrap();
        let (session, attached) = map
            .attach(
                RenderTarget::Offscreen { size, device: Some(device.clone()) },
                AttachOptions { driver: Driver::CoreWorker, ..Default::default() },
            )
            .unwrap();
        attached.wait().unwrap();
        map.set_style_json(style.to_owned()).unwrap().wait().unwrap();
        Renderer { map, session, size }
    }

    fn render(&mut self, shot: Shot) -> Vec<u8> {
        let size = Size::new(shot.px, shot.px);
        if size != self.size {
            let scale = self.session.snapshot().unwrap().extent.scale_factor;
            self.session.resize(LogicalExtent::new(size, scale)).unwrap().wait().unwrap();
            self.size = size;
        }
        drop(self.map.update_camera(CameraUpdate {
            mode: CameraUpdateMode::Jump,
            camera: CameraOptions {
                center: Some(LatLng::new(shot.lat, shot.lng).unwrap()),
                zoom: Some(shot.zoom),
                ..Default::default()
            },
            ..Default::default()
        }));
        self.map.request_still_image().unwrap().wait().unwrap();
        drop(self.session.drain_frame_results());
        let image = self.session.read_texture().unwrap().wait().unwrap();
        mln_engine::image::encode_png(&image)
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let style = std::fs::read_to_string(&args[1]).unwrap();
    let backend = match args[2].as_str() {
        "metal" => BackendType::Metal,
        "webgpu" => BackendType::WebGPU,
        other => panic!("{other}"),
    };
    let out = std::path::PathBuf::from(&args[3]);
    let workers: usize = args.get(4).map_or(4, |a| a.parse().unwrap());
    let rounds: usize = args.get(5).map_or(2, |a| a.parse().unwrap());
    std::fs::create_dir_all(&out).unwrap();
    let runtime = Runtime::new(&RuntimeOptions::default()).unwrap();
    let shots = shots();
    let results: Arc<Mutex<HashMap<String, Vec<(usize, usize, Vec<u8>)>>>> = Arc::default();

    std::thread::scope(|scope| {
        for worker in 0..workers {
            let (runtime, style, shots, results) = (&runtime, &style, &shots, &results);
            scope.spawn(move || {
                let device = Device::with_backend(backend, DeviceOptions::default()).unwrap();
                let mut tile = Renderer::new(runtime, &device, style, MapMode::Tile, Size::new(512, 512));
                let mut still = Renderer::new(runtime, &device, style, MapMode::Static, Size::new(200, 200));
                for round in 0..rounds {
                    // Each worker walks the shots in its own order.
                    let mut order: Vec<usize> = (0..shots.len()).collect();
                    order.rotate_left((worker * 7 + round * 3) % shots.len());
                    for i in order {
                        let (name, shot) = &shots[i];
                        let renderer = if shot.mode == MapMode::Tile { &mut tile } else { &mut still };
                        let png = renderer.render(*shot);
                        results.lock().unwrap().entry(name.clone()).or_default().push((worker, round, png));
                    }
                }
            });
        }
    });

    let results = results.lock().unwrap();
    let mut names: Vec<_> = results.keys().cloned().collect();
    names.sort();
    let mut differing = 0;
    for name in names {
        let renders = &results[&name];
        let mut distinct: Vec<&Vec<u8>> = Vec::new();
        for (worker, round, png) in renders {
            if !distinct.contains(&png) {
                distinct.push(png);
                std::fs::write(out.join(format!("{name}-w{worker}-r{round}.png")), png).unwrap();
            }
        }
        if distinct.len() > 1 {
            differing += 1;
            println!("{name}: {} distinct renders of {}", distinct.len(), renders.len());
        }
    }
    println!("{differing} cameras with differing renders");
}
