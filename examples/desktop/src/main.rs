// The desktop example app: a map in a winit window (OpenFreeMap Liberty, or
// `--style`, or MLN_STYLE_URL), with the web demo's controls over it in egui.
//
// - host.rs: the window and its event loop on the main thread, which only
//   turns window events into messages for the render thread.
// - view.rs: the render thread, which owns the RunLoop, the map, its
//   renderer and the surface, applies the messages and draws.
// - ui.rs: the controls, which view.rs lays out and an overlay of the window's
//   backend paints over the map (overlay.rs, metal_overlay.rs,
//   vulkan_overlay.rs, gl_overlay.rs).
//
// Command line: -h/--help, -f/--fullscreen, -b/--benchmark, -o/--offline,
// -s/--style URL, -t/--apikey KEY, -c/--cache FILE, -x/--lon, -y/--lat,
// -a/--alt, --fov, -z/--zoom, --bearing, -p/--pitch, -r/--roll,
// -P/--maxPitch, --backend (metal, the default on macOS, webgpu: wgpu, the
// default elsewhere, vulkan, elsewhere than on Apple's platforms, or opengl:
// OpenGL ES, on Linux; see backend.rs), and for checking the app:
// --readback FILE.png (write the presented frame once the map is idle, see
// view.rs), --quit-after SECONDS, --gesture-test (scripted input through the
// view's handlers) and --no-system-fonts: without it, the map falls back to
// system fonts for the scripts that its glyph PBFs can't shape, and for CJK
// (system_fonts.rs).

mod backend;
#[cfg(target_os = "linux")]
mod gl_overlay;
mod host;
#[cfg(target_os = "macos")]
mod metal_layer;
#[cfg(target_os = "macos")]
mod metal_overlay;
mod overlay;
mod settings;
mod styles;
mod system_fonts;
mod ui;
mod view;
#[cfg(not(target_vendor = "apple"))]
mod vulkan_overlay;

use crate::settings::Settings;

const USAGE: &str = "  mln-desktop {OPTIONS}

    The mln-rs desktop example

  OPTIONS:

      -h, --help                        Display this help menu
      -f, --fullscreen                  Toggle fullscreen
      -b, --benchmark                   Toggle benchmark
      -o, --offline                     Toggle offline
      -t[key], --apikey=[key]           API key
      -s[URL], --style=[URL]            Map stylesheet
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
      --backend=[backend]               Rendering backend: metal (macOS), webgpu,
                                        vulkan (not macOS) or opengl (Linux)
      --readback=[file.png]             Write the frame once the map is idle
      --quit-after=[seconds]            Close the window after this time
      --gesture-test                    Replay scripted input, log the camera, quit
      --free-bearing                    Let drags and zooms on the globe turn the bearing
      --no-system-fonts                Don't fall back to system fonts for complex scripts and CJK
";

/// The command line.
#[derive(Default)]
pub(crate) struct Args {
    pub(crate) fullscreen: bool,
    pub(crate) benchmark: bool,
    pub(crate) offline: bool,
    pub(crate) apikey: Option<String>,
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
    pub(crate) backend: backend::Kind,
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
            "b" | "benchmark" => args.benchmark = true,
            "o" | "offline" => args.offline = true,
            "t" | "apikey" => args.apikey = Some(value(&name)?),
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
            "backend" => args.backend = backend::Kind::parse(&value(&name)?)?,
            _ => return Err(format!("Flag could not be matched: {name}")),
        }
    }
    Ok(args)
}

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("{e}");
            eprint!("{USAGE}");
            std::process::exit(1);
        }
    };

    let mut settings = Settings { online: !args.offline, free_bearing: args.free_bearing, ..Default::default() };
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
