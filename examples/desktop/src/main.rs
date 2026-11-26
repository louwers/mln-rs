// The desktop example app: a map in a winit window (OpenFreeMap Liberty, or
// `--style`, or MLN_STYLE_URL), with the web demo's controls over it in egui.
//
// - host.rs: the window and its event loop on the main thread, which only
//   turns window events into messages for the render thread.
// - view.rs: the render thread, which drives the map through its runtime
//   (mln_runtime) and the render session that presents it to the window
//   (backend.rs), applies the messages and draws.
// - ui.rs: the controls, which view.rs lays out and an overlay of the window's
//   backend paints over the map (overlay.rs, metal_overlay.rs,
//   vulkan_overlay.rs, d3d12_overlay.rs, gl_overlay.rs).
// - benchmark.rs: the benchmark mode, a camera path of the benchmarks drawn
//   in the window, timed.
//
// Command line: -h/--help, -f/--fullscreen, -b/--benchmark[=PATH], --laps,
// -o/--offline, --network NETWORK, --disable-cache,
// -s/--style URL, -c/--cache FILE, -x/--lon, -y/--lat, -a/--alt, --fov,
// -z/--zoom, --bearing, -p/--pitch, -r/--roll,
// -P/--maxPitch, --backend (metal, on macOS, vulkan, elsewhere than on
// Apple's platforms, d3d12, on Windows, opengl: OpenGL ES, on Linux, or
// webgpu: wgpu; by default the library's pick, see backend.rs), and for checking
// the app:
// --readback FILE.png (write the map once it is idle, without the controls,
// see view.rs), --quit-after SECONDS, --gesture-test (scripted input through
// the view's handlers) and --no-system-fonts: without it, the map falls back
// to system fonts for the scripts that its glyph PBFs can't shape, for the
// glyphs its PBFs lack, and for CJK (mln_engine::text::system_fonts).

mod backend;
mod benchmark;
#[cfg(windows)]
mod d3d12_overlay;
#[cfg(target_os = "linux")]
mod gl_overlay;
mod host;
#[cfg(target_os = "macos")]
mod metal_layer;
#[cfg(target_os = "macos")]
mod metal_overlay;
mod network;
mod overlay;
mod pacing;
mod settings;
mod styles;
mod ui;
mod view;
#[cfg(not(target_vendor = "apple"))]
mod vulkan_overlay;
#[cfg(windows)]
mod win32_window;

use crate::settings::Settings;

const USAGE: &str = "  mln-desktop {OPTIONS}

    The mln-rs desktop example

  OPTIONS:

      -h, --help                        Display this help menu
      -f, --fullscreen                  Toggle fullscreen
      -b, --benchmark[=path]            Draw a benchmark camera path (street, the
                                        default, overzoom, globe, antimeridian,
                                        earthquakes, terrain, terrain-3d or labels) as
                                        fast as the window takes frames, print their
                                        times, quit
      --laps=[number]                   Laps of the benchmark's path (10; 0: until
                                        the window closes)
      -o, --offline                     Toggle offline
      --network=[network]               Throttle the network: online, fast-4g, slow-4g,
                                        3g or offline
      --disable-cache                   Don't keep what the map loads in the cache
      -s[STYLE], --style=[STYLE]        Map stylesheet (catalog key, URL, or file)
      -c[file], --cache=[file]          Cache database file name
      -x[degrees], --lon=[degrees]      Longitude
      -y[degrees], --lat=[degrees]      Latitude
      -a[degrees], --alt=[degrees]      Altitude
      --fov=[degrees]                   FOV
      -z[number], --zoom=[number]       Zoom level
      --bearing=[degrees]               Bearing
      -p[degrees], --pitch=[degrees]    Pitch
      -r[degrees], --roll=[degrees]     Roll
      -P[degrees], --maxPitch=[degrees] Max Pitch
      --backend=[backend]               Rendering backend: metal (macOS), vulkan
                                        (not macOS), d3d12 (Windows), opengl
                                        (Linux) or webgpu; by default the
                                        library's pick
      --readback=[file.png]             Write the map once it is idle
      --quit-after=[seconds]            Close the window after this time
      --gesture-test                    Replay scripted input, log the camera, quit
      --free-bearing                    Let drags and zooms on the globe turn the bearing
      --no-system-fonts                Don't fall back to system fonts for complex scripts and CJK
";

/// The command line.
#[derive(Default)]
pub(crate) struct Args {
    pub(crate) fullscreen: bool,
    /// The benchmark mode's camera path.
    pub(crate) benchmark: Option<mln_bench_workload::paths::Path>,
    /// The benchmark's laps, or `None` for its default.
    pub(crate) laps: Option<usize>,
    pub(crate) network: network::Network,
    pub(crate) disable_cache: bool,
    pub(crate) style: Option<String>,
    pub(crate) cache: Option<String>,
    pub(crate) lon: Option<f64>,
    pub(crate) lat: Option<f64>,
    pub(crate) alt: Option<f64>,
    pub(crate) fov: Option<f64>,
    pub(crate) zoom: Option<f64>,
    pub(crate) bearing: Option<f64>,
    pub(crate) pitch: Option<f64>,
    pub(crate) roll: Option<f64>,
    pub(crate) max_pitch: Option<f64>,
    pub(crate) readback: Option<String>,
    pub(crate) quit_after: Option<f64>,
    pub(crate) gesture_test: bool,
    pub(crate) free_bearing: bool,
    pub(crate) no_system_fonts: bool,
    /// `None` renders on the device that the library picks.
    pub(crate) backend: Option<backend::Kind>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        // --name=value, --name value, -xvalue, -x value
        let (name, inline): (String, Option<String>) = if let Some(long) = arg.strip_prefix("--") {
            match long.split_once('=') {
                Some((n, v)) => (n.to_owned(), Some(v.to_owned())),
                None => (long.to_owned(), None),
            }
        } else if let Some(short) = arg.strip_prefix('-')
            && !short.is_empty()
        {
            let (n, v) = short.split_at(1);
            (n.to_owned(), (!v.is_empty()).then(|| v.to_owned()))
        } else {
            return Err(format!("Passed in argument, but no positional arguments were ready to receive it: {arg}"));
        };
        let mut value = |name: &str| -> Result<String, String> {
            inline.clone().or_else(|| it.next()).ok_or_else(|| format!("Flag '{name}' requires an argument"))
        };
        let number = |s: String, name: &str| {
            s.parse::<f64>().map_err(|_| format!("Argument '{name}' received invalid value type '{s}'"))
        };
        match name.as_str() {
            "h" | "help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "f" | "fullscreen" => args.fullscreen = true,
            "b" | "benchmark" => {
                let path = inline
                    .as_deref()
                    .map_or(Ok(mln_bench_workload::paths::Path::Street), mln_bench_workload::paths::Path::parse)?;
                args.benchmark = Some(path);
            }
            "laps" => {
                let laps = value(&name)?;
                args.laps =
                    Some(laps.parse().map_err(|_| format!("Argument '{name}' received invalid value type '{laps}'"))?);
            }
            "o" | "offline" => args.network = network::Network::Offline,
            "network" => args.network = network::Network::parse(&value(&name)?)?,
            "disable-cache" => args.disable_cache = true,
            "s" | "style" => args.style = Some(value(&name)?),
            "c" | "cache" => args.cache = Some(value(&name)?),
            "x" | "lon" => args.lon = Some(number(value(&name)?, &name)?),
            "y" | "lat" => args.lat = Some(number(value(&name)?, &name)?),
            "a" | "alt" => args.alt = Some(number(value(&name)?, &name)?),
            "fov" => args.fov = Some(number(value(&name)?, &name)?),
            "z" | "zoom" => args.zoom = Some(number(value(&name)?, &name)?),
            "bearing" => args.bearing = Some(number(value(&name)?, &name)?),
            "p" | "pitch" => args.pitch = Some(number(value(&name)?, &name)?),
            "r" | "roll" => args.roll = Some(number(value(&name)?, &name)?),
            "P" | "maxPitch" => args.max_pitch = Some(number(value(&name)?, &name)?),
            "readback" => args.readback = Some(value(&name)?),
            "quit-after" => args.quit_after = Some(number(value(&name)?, &name)?),
            "gesture-test" => args.gesture_test = true,
            "free-bearing" => args.free_bearing = true,
            "no-system-fonts" => args.no_system_fonts = true,
            "backend" => args.backend = Some(backend::Kind::parse(&value(&name)?)?),
            _ => return Err(format!("Flag could not be matched: {name}")),
        }
    }
    Ok(args)
}

/// Prints the `log` records of the library and this program, and other
/// crates' warnings and errors, to stderr.
struct StderrLog;

impl log::Log for StderrLog {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Warn
            || (metadata.level() <= log::Level::Info && metadata.target().starts_with("mln_"))
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            eprintln!("[{}] [{}] {}", record.level(), record.target(), record.args());
        }
    }

    fn flush(&self) {}
}

fn main() {
    if log::set_logger(&StderrLog).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    let args = match parse_args() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("{e}");
            eprint!("{USAGE}");
            std::process::exit(1);
        }
    };

    let mut settings = Settings {
        network: args.network,
        cache_disabled: args.disable_cache,
        free_bearing: args.free_bearing,
        ..Default::default()
    };
    for (value, field) in [
        (args.lon, &mut settings.longitude),
        (args.lat, &mut settings.latitude),
        (args.alt, &mut settings.altitude),
        (args.fov, &mut settings.fov),
        (args.max_pitch, &mut settings.max_pitch),
        (args.zoom, &mut settings.zoom),
        (args.bearing, &mut settings.bearing),
        (args.pitch, &mut settings.pitch),
        (args.roll, &mut settings.roll),
    ] {
        if let Some(value) = value {
            *field = value;
        }
    }

    if let Err(e) = host::run(args, settings) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
