//! `mln-bench-snapshot <id>`: builds the benchmarks' fixture snapshot from
//! pinned public map data, stages it in `target/bench-fixtures-upload/<id>/`
//! as it will sit under the manifest's base URL, and writes the manifest,
//! `tools/bench/fixtures.json` (tools/bench/README.md).
//!
//! Downloads go through curl, at most 8 at once, and are kept in
//! `target/bench-snapshot/downloads`, so building again fetches only what is
//! missing; delete that directory to fetch everything anew.

mod fetch;
mod style;
mod tiles;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use pmtiles::{TileCoord, TileId};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::fetch::{Download, Found, encode_segment};
use crate::tiles::{CJK, Encoding, STREET, TERRAIN, TileSet, WORLD};

/// The OpenFreeMap planet build the tiles come from. The TileJSON at
/// `https://tiles.openfreemap.org/planet` names the build it serves now by a
/// dated path; to snapshot a newer build, put that path here.
const PLANET: &str = "https://tiles.openfreemap.org/planet/20261115_080022_pt";
/// OpenFreeMap's Liberty style, as it serves it now.
const STYLE: &str = "https://tiles.openfreemap.org/styles/liberty";
/// OpenFreeMap's glyph server, which the style names.
const GLYPHS: &str = "https://tiles.openfreemap.org/fonts";
/// Mapterhorn's terrain tiles, which it builds from the best open elevation
/// data of each area: USGS 3DEP around San Francisco, and Copernicus GLO-30 at
/// low zooms. Mapterhorn serves only its latest build, so the snapshot holds
/// the tiles as they were when it was built.
const TERRAIN_TILES: &str = "https://tiles.mapterhorn.com";
/// Where the snapshots are published, each under its id.
const BASE_URL: &str = "https://mlc-data.sargunv.dev/benchmark-fixtures/";

/// A file the snapshot holds as its source publishes it, at `path`.
struct Copied {
    path: &'static str,
    url: &'static str,
}

/// Natural Earth's countries at 1:10m, from release v5.1.2: the GeoJSON
/// scenarios' data.
const GEOJSON: Copied = Copied {
    path: "geojson/ne_10m_admin_0_countries.geojson",
    url: "https://raw.githubusercontent.com/nvkelso/natural-earth-vector/f1890d9f152c896d250a77557a5751a93d494776/geojson/ne_10m_admin_0_countries.geojson",
};

/// The earthquakes of October 2026, as the USGS's event service lists them,
/// oldest first: the data visualization scenarios' points. The service may
/// revise an event later, so the snapshot holds the list as it was when it
/// was built.
const EARTHQUAKES: Copied = Copied {
    path: "geojson/earthquakes-2026-10.geojson",
    url: "https://earthquake.usgs.gov/fdsnws/event/1/query?format=geojson&starttime=2026-10-01&endtime=2026-11-01&orderby=time-asc",
};

/// Font files, for the text scenarios that shape and rasterize glyphs from a
/// font rather than read them from glyph ranges, with their licenses.
const FONTS: [Copied; 4] = [
    Copied {
        path: "fonts/NotoSans-Regular.ttf",
        url: "https://raw.githubusercontent.com/notofonts/notofonts.github.io/28b15b4b43b7bed62b5cf6e6b0b5ff5846270535/fonts/NotoSans/hinted/ttf/NotoSans-Regular.ttf",
    },
    Copied {
        path: "fonts/NotoSans-LICENSE.txt",
        url: "https://raw.githubusercontent.com/notofonts/notofonts.github.io/28b15b4b43b7bed62b5cf6e6b0b5ff5846270535/fonts/LICENSE",
    },
    Copied {
        path: "fonts/NotoSansJP-Regular.otf",
        url: "https://raw.githubusercontent.com/notofonts/noto-cjk/523d033d6cb47f4a80c58a35753646f5c3608a78/Sans/SubsetOTF/JP/NotoSansJP-Regular.otf",
    },
    Copied {
        path: "fonts/NotoSansJP-LICENSE.txt",
        url: "https://raw.githubusercontent.com/notofonts/noto-cjk/523d033d6cb47f4a80c58a35753646f5c3608a78/LICENSE",
    },
];

/// The licenses of the style, its icons and its fonts.
const STYLE_LICENSE: Copied = Copied {
    path: "styles/LICENSE.md",
    url: "https://raw.githubusercontent.com/hyperknot/openfreemap-styles/006ea5796803ba2df4330888836cb0a8f17473b4/LICENSE.md",
};

/// Glyph ranges a map can ask for whatever the labels' own characters:
/// Arabic presentation forms, which shaping Arabic text gives, and the
/// vertical forms of CJK punctuation. These, and the ranges of surrogates,
/// are kept if the glyph server has them; every other range the labels need
/// must be there.
const SHAPING_RANGES: [u32; 4] = [0xFB00, 0xFC00, 0xFD00, 0xFE00];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [id] = args.as_slice() else {
        eprintln!("usage: mln-bench-snapshot <snapshot id>");
        return ExitCode::FAILURE;
    };
    match build(id) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("mln-bench-snapshot: {error}");
            ExitCode::FAILURE
        }
    }
}

fn build(id: &str) -> Result<(), String> {
    if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte)) {
        return Err(format!("{id:?} isn't a snapshot id: use letters, digits, '-', '_' and '.'"));
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let root = root.canonicalize().map_err(|error| format!("{}: {error}", root.display()))?;
    let downloads = root.join("target/bench-snapshot/downloads");
    let uploads = root.join("target/bench-fixtures-upload");
    let staged = uploads.join(id);
    if staged.exists() {
        return Err(format!("{} exists; a snapshot never changes, so give this one another id", staged.display()));
    }
    std::fs::create_dir_all(&uploads).map_err(|error| format!("{}: {error}", uploads.display()))?;
    // Built beside its place and renamed into it once complete.
    let partial = tempfile::Builder::new()
        .prefix(&format!(".{id}-"))
        .tempdir_in(&uploads)
        .map_err(|error| format!("{}: {error}", uploads.display()))?;
    let out = partial.path();

    // The planet's description and the style.
    let planet = Download::new(&downloads, PLANET.to_owned());
    let liberty = Download::new(&downloads, STYLE.to_owned());
    fetch_all(&[&planet, &liberty])?;
    let tilejson = read_json(&planet.path)?;
    let mut liberty = read_json(&liberty.path)?;
    let tile_url = format!("{PLANET}/{{z}}/{{x}}/{{y}}.pbf");
    if tilejson["tiles"][0] != tile_url.as_str() {
        return Err(format!("{PLANET} names tiles {}, not {tile_url}", tilejson["tiles"][0]));
    }
    let sprite_url = liberty["sprite"].as_str().ok_or("the style has no sprite")?.to_owned();
    let sprite_path =
        sprite_url.strip_prefix("https://tiles.openfreemap.org/").ok_or("the sprite isn't OpenFreeMap's")?.to_owned();
    let stacks = style::font_stacks(&liberty)?;
    let label_properties = style::label_properties(&liberty);
    let source_layers = style::source_layers(&liberty);

    // The tiles, checked against what the style reads.
    let mut sets = Vec::new();
    for set in [&STREET, &WORLD, &CJK] {
        let tiles = fetch_tiles(&downloads, set)?;
        check_tiles(set, &tiles, &source_layers)?;
        sets.push((set, tiles));
    }

    // Every glyph range that the labels' characters fall in, for every font.
    let mut ranges = BTreeSet::from([0]);
    ranges.extend(SHAPING_RANGES);
    let mut cjk_labels = 0;
    for (set, tiles) in &sets {
        for data in tiles.values() {
            for text in label_texts(data, &label_properties)? {
                if set.name == CJK.name && text.chars().any(is_cjk) {
                    cjk_labels += 1;
                }
                // A map shapes text as UTF-16, and asks for the range of each
                // code unit: a character beyond the BMP asks for its
                // surrogates' ranges.
                let upper = text.to_uppercase();
                for unit in text.encode_utf16().chain(upper.encode_utf16()) {
                    ranges.insert(u32::from(unit) & !0xFF);
                }
            }
        }
    }
    if cjk_labels == 0 {
        return Err("the cjk tiles have no label in Japanese script".to_owned());
    }
    eprintln!("mln-bench-snapshot: {cjk_labels} label strings of the cjk tiles are in CJK script");
    let mut glyphs = Vec::new();
    for stack in &stacks {
        for &start in &ranges {
            let range = format!("{start}-{}.pbf", start + 255);
            let url = format!("{GLYPHS}/{}/{range}", encode_segment(stack));
            glyphs.push((format!("glyphs/{stack}/{range}"), start, Download::new(&downloads, url)));
        }
    }
    let found = fetch::fetch(&glyphs.iter().map(|(_, _, download)| download).collect::<Vec<_>>())?;
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    let mut absent = Vec::new();
    for ((path, start, download), found) in glyphs.into_iter().zip(found) {
        match found {
            Found::Body => files.push((path, download.path)),
            Found::Missing if SHAPING_RANGES.contains(&start) || (0xD800..0xE000).contains(&start) => {
                eprintln!("mln-bench-snapshot: the glyph server has no {path}, which a map then lacks too");
            }
            Found::Missing => absent.push(path),
        }
    }
    if !absent.is_empty() {
        return Err(format!("the glyph server lacks ranges the labels need:\n  {}", absent.join("\n  ")));
    }
    eprintln!("mln-bench-snapshot: {} glyph ranges for {} font stacks", files.len(), stacks.len());

    // The sprite, the font files, the GeoJSON and the licenses.
    let mut copies = Vec::new();
    for suffix in [".json", ".png", "@2x.json", "@2x.png"] {
        copies.push((format!("{sprite_path}{suffix}"), Download::new(&downloads, format!("{sprite_url}{suffix}"))));
    }
    for copied in FONTS.iter().chain([&GEOJSON, &EARTHQUAKES, &STYLE_LICENSE]) {
        copies.push((copied.path.to_owned(), Download::new(&downloads, copied.url.to_owned())));
    }
    fetch_all(&copies.iter().map(|(_, download)| download).collect::<Vec<_>>())?;
    files.extend(copies.into_iter().map(|(path, download)| (path, download.path)));
    for (path, download) in &files {
        let target = out.join(path);
        create_parent(&target)?;
        std::fs::copy(download, &target).map_err(|error| format!("{}: {error}", target.display()))?;
    }

    // The archives, and the street tiles again as MLT.
    let mut tile_sets = serde_json::Map::new();
    for (set, tiles) in &sets {
        let path = format!("tiles/{}.pmtiles", set.name);
        write_archive(out, &path, set, Encoding::Mvt, tiles, &tilejson)?;
        tile_sets.insert(set.name.to_owned(), tile_set_entry(set, &path, Encoding::Mvt, tiles));
    }
    let street = &sets[0].1;
    eprintln!("mln-bench-snapshot: encoding {} street tiles as MLT", street.len());
    let mlt = encode_all(street)?;
    for (id, data) in &mlt {
        let mvt = tiles::mvt_layers(&street[id])?;
        let layers = tiles::mlt_layers(data)?;
        if layers != mvt {
            return Err(format!("MLT tile {:?} has layers {layers:?}, its MVT tile {mvt:?}", TileCoord::from(*id)));
        }
    }
    let path = "tiles/street-mlt.pmtiles";
    write_archive(out, path, &STREET, Encoding::Mlt, &mlt, &tilejson)?;
    tile_sets.insert("street-mlt".to_owned(), tile_set_entry(&STREET, path, Encoding::Mlt, &mlt));

    // The terrain.
    let terrain = fetch_terrain(&downloads)?;
    let path = format!("tiles/{}.pmtiles", TERRAIN.name);
    let file = out.join(&path);
    create_parent(&file)?;
    let metadata = json!({
        "name": TERRAIN.name,
        "description": TERRAIN.description,
        "attribution": "<a href='https://mapterhorn.com/attribution'>© Mapterhorn</a>",
        "encoding": "terrarium",
    });
    tiles::write_archive(&file, &TERRAIN, Encoding::Webp, &terrain, &metadata)?;
    tiles::check_archive(&file, &terrain)?;
    tile_sets.insert(TERRAIN.name.to_owned(), tile_set_entry(&TERRAIN, &path, Encoding::Webp, &terrain));

    // The style.
    let style_changes = style::localize(&mut liberty, &sprite_path)?;
    let style_file = out.join("styles/liberty.json");
    create_parent(&style_file)?;
    let text = serde_json::to_string_pretty(&liberty).expect("a style serializes") + "\n";
    std::fs::write(&style_file, text).map_err(|error| format!("{}: {error}", style_file.display()))?;

    // The manifest, and the snapshot in its place.
    let files = list_files(out)?;
    let manifest = json!({
        "snapshot": id,
        "base_url": format!("{BASE_URL}{id}/"),
        "description": "The benchmarks' map data: OpenFreeMap's OpenMapTiles vector tiles of central San \
            Francisco (street, and street-mlt as MLT), of the whole world to zoom 4 (world) and of central \
            Tokyo (cjk), and Mapterhorn's terrain tiles of central San Francisco (terrain), each a PMTiles \
            archive; OpenFreeMap's Liberty style pointed at them, with its sprite and the glyph ranges the \
            tiles' labels need; Noto Sans font files; and as GeoJSON, Natural Earth's countries and the USGS's \
            earthquakes of October 2026. tools/bench-snapshot built it.",
        "attribution": "OpenFreeMap © OpenMapTiles Data from OpenStreetMap",
        "sources": sources(&sprite_url),
        "style_changes": style_changes,
        "tile_sets": tile_sets,
        "files": files,
    });
    let total: u64 = files.iter().map(|file| file["size"].as_u64().expect("a size")).sum();
    std::fs::rename(partial.keep(), &staged).map_err(|error| format!("{}: {error}", staged.display()))?;
    let manifest_path = root.join("tools/bench/fixtures.json");
    let text = serde_json::to_string_pretty(&manifest).expect("a manifest serializes") + "\n";
    std::fs::write(&manifest_path, text).map_err(|error| format!("{}: {error}", manifest_path.display()))?;
    println!("staged {} ({} files, {:.1} MB)", staged.display(), files.len(), total as f64 / 1e6);
    println!("wrote {}", manifest_path.display());
    Ok(())
}

impl Download {
    /// A download of `url` into the download directory, at a path that
    /// mirrors the URL's.
    fn new(downloads: &Path, url: String) -> Download {
        let relative = url.split_once("://").map_or(url.as_str(), |(_, rest)| rest);
        let mut path = downloads.to_path_buf();
        for segment in relative.split('/').filter(|segment| !segment.is_empty() && *segment != "..") {
            path.push(segment);
        }
        // A URL that names a directory, such as the planet's TileJSON, which
        // its tiles' URLs continue.
        path.push("@");
        Download { url, path }
    }
}

/// Fetches every download, each of which must be found.
fn fetch_all(downloads: &[&Download]) -> Result<(), String> {
    let found = fetch::fetch(downloads)?;
    let missing: Vec<&str> = downloads
        .iter()
        .zip(found)
        .filter(|(_, found)| *found == Found::Missing)
        .map(|(download, _)| download.url.as_str())
        .collect();
    if missing.is_empty() { Ok(()) } else { Err(format!("not found:\n  {}", missing.join("\n  "))) }
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_slice(&text).map_err(|error| format!("{}: {error}", path.display()))
}

fn create_parent(path: &Path) -> Result<(), String> {
    let parent = path.parent().expect("a file's directory");
    std::fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))
}

/// The terrain tiles over [`TERRAIN`]'s area. A tile Mapterhorn lacks, as
/// over the sea, is left out; each zoom must have some.
fn fetch_terrain(downloads: &Path) -> Result<BTreeMap<TileId, Vec<u8>>, String> {
    let coords = TERRAIN.tiles();
    let fetches: Vec<Download> = coords
        .iter()
        .map(|coord| Download::new(downloads, format!("{TERRAIN_TILES}/{}/{}/{}.webp", coord.z(), coord.x(), coord.y())))
        .collect();
    eprintln!("mln-bench-snapshot: {} tiles for {}", coords.len(), TERRAIN.name);
    let found = fetch::fetch(&fetches.iter().collect::<Vec<_>>())?;
    let mut tiles = BTreeMap::new();
    for ((coord, download), found) in coords.iter().zip(&fetches).zip(found) {
        if found == Found::Missing {
            continue;
        }
        let data = std::fs::read(&download.path).map_err(|error| format!("{}: {error}", download.path.display()))?;
        if !data.starts_with(b"RIFF") {
            return Err(format!("{} isn't a WebP image", download.url));
        }
        tiles.insert(TileId::from(*coord), data);
    }
    for z in 0..=TERRAIN.max_zoom {
        if !tiles.keys().any(|&id| TileCoord::from(id).z() == z) {
            return Err(format!("{} has no tile at zoom {z}", TERRAIN.name));
        }
    }
    Ok(tiles)
}

/// The tiles of `set`, uncompressed, by id. The planet has every tile, so a
/// missing one fails; an empty one, which the planet may answer with no
/// content, is left out of the archive as PMTiles requires.
fn fetch_tiles(downloads: &Path, set: &TileSet) -> Result<BTreeMap<TileId, Vec<u8>>, String> {
    let coords = set.tiles();
    let fetches: Vec<Download> = coords
        .iter()
        .map(|coord| Download::new(downloads, format!("{PLANET}/{}/{}/{}.pbf", coord.z(), coord.x(), coord.y())))
        .collect();
    eprintln!("mln-bench-snapshot: {} tiles for {}", coords.len(), set.name);
    fetch_all(&fetches.iter().collect::<Vec<_>>())?;
    let mut tiles = BTreeMap::new();
    for (coord, download) in coords.iter().zip(&fetches) {
        let data = std::fs::read(&download.path).map_err(|error| format!("{}: {error}", download.path.display()))?;
        if !data.is_empty() {
            tiles.insert(TileId::from(*coord), data);
        }
    }
    Ok(tiles)
}

/// Checks that `set` has tiles at each of its zooms, and that its tiles hold
/// every layer the style draws, at least one tile each.
fn check_tiles(
    set: &TileSet,
    tiles: &BTreeMap<TileId, Vec<u8>>,
    source_layers: &BTreeSet<String>,
) -> Result<(), String> {
    let mut by_zoom = BTreeMap::<u8, usize>::new();
    let mut layers = BTreeSet::new();
    for (&id, data) in tiles {
        *by_zoom.entry(TileCoord::from(id).z()).or_default() += 1;
        layers.extend(tiles::mvt_layers(data)?);
    }
    let expected = set.tiles();
    for z in 0..=set.max_zoom {
        let count = expected.iter().filter(|coord| coord.z() == z).count();
        let found = by_zoom.get(&z).copied().unwrap_or(0);
        eprintln!("mln-bench-snapshot: {} zoom {z}: {found} of {count} tiles have data", set.name);
        if found == 0 {
            return Err(format!("{} has no tile at zoom {z}", set.name));
        }
    }
    let missing: Vec<&String> = source_layers.difference(&layers).collect();
    eprintln!("mln-bench-snapshot: {} lacks the style's layers {missing:?}", set.name);
    if set.name != WORLD.name && !missing.is_empty() {
        return Err(format!("{}'s tiles lack the style's layers {missing:?}", set.name));
    }
    Ok(())
}

/// The strings of the properties that labels read, in every feature of a
/// tile.
fn label_texts(data: &[u8], properties: &BTreeSet<String>) -> Result<Vec<String>, String> {
    let features = mlt_core::mvt::mvt_to_feature_collection(data).map_err(|error| error.to_string())?;
    let mut texts = Vec::new();
    for feature in features.features {
        for (name, value) in feature.properties {
            if properties.contains(&name)
                && let Value::String(text) = value
            {
                texts.push(text);
            }
        }
    }
    Ok(texts)
}

fn is_cjk(character: char) -> bool {
    matches!(u32::from(character), 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF)
}

/// Every tile as MLT, encoded on every core.
fn encode_all(tiles: &BTreeMap<TileId, Vec<u8>>) -> Result<BTreeMap<TileId, Vec<u8>>, String> {
    let tiles: Vec<(&TileId, &Vec<u8>)> = tiles.iter().collect();
    let threads = std::thread::available_parallelism().map_or(4, usize::from);
    let chunk = tiles.len().div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        let workers: Vec<_> = tiles
            .chunks(chunk)
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|&(&id, data)| Ok((id, tiles::encode_mlt(data)?)))
                        .collect::<Result<Vec<_>, String>>()
                })
            })
            .collect();
        let mut encoded = BTreeMap::new();
        for worker in workers {
            encoded.extend(worker.join().expect("an encoding thread")?);
        }
        Ok(encoded)
    })
}

/// Writes the archive at `path` under `out`, and reads it back.
fn write_archive(
    out: &Path,
    path: &str,
    set: &TileSet,
    encoding: Encoding,
    tiles: &BTreeMap<TileId, Vec<u8>>,
    tilejson: &Value,
) -> Result<(), String> {
    let file = out.join(path);
    create_parent(&file)?;
    tiles::write_archive(&file, set, encoding, tiles, &tiles::metadata(set, encoding, tilejson))?;
    tiles::check_archive(&file, tiles)
}

/// The manifest's description of a tile set's archive.
fn tile_set_entry(set: &TileSet, path: &str, encoding: Encoding, tiles: &BTreeMap<TileId, Vec<u8>>) -> Value {
    json!({
        "archive": path,
        "encoding": match encoding {
            Encoding::Mvt => "mvt",
            Encoding::Mlt => "mlt",
            Encoding::Webp => "terrarium-webp",
        },
        "description": set.description,
        "area": set.area,
        "bounds": set.bounds(),
        "zooms": [0, set.max_zoom],
        "tiles": tiles.len(),
    })
}

/// Every file under `dir`, by path relative to it with `/` between
/// components, with its size and SHA-256, sorted by path.
fn list_files(dir: &Path) -> Result<Vec<Value>, String> {
    fn walk(dir: &Path, prefix: &str, files: &mut Vec<Value>) -> Result<(), String> {
        let entries = std::fs::read_dir(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| format!("{}: {error}", dir.display()))?;
            let name = entry.file_name().into_string().map_err(|name| format!("{} isn't UTF-8", name.display()))?;
            let path = format!("{prefix}{name}");
            if entry.path().is_dir() {
                walk(&entry.path(), &format!("{path}/"), files)?;
            } else {
                let data = std::fs::read(entry.path()).map_err(|error| format!("{path}: {error}"))?;
                files.push(json!({ "path": path, "size": data.len(), "sha256": hex(&Sha256::digest(&data)) }));
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(dir, "", &mut files)?;
    files.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    Ok(files)
}

fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(text, "{byte:02x}").expect("writing to a string");
    }
    text
}

/// Where the snapshot's data comes from, and under what terms.
fn sources(sprite_url: &str) -> Value {
    json!([
        {
            "files": ["tiles/street.pmtiles", "tiles/street-mlt.pmtiles", "tiles/world.pmtiles", "tiles/cjk.pmtiles"],
            "origin": format!("OpenFreeMap's planet build {PLANET}, in the OpenMapTiles schema; street-mlt holds the street tiles re-encoded as MLT"),
            "license": "Map data © OpenStreetMap contributors, under the Open Database License 1.0 \
                (https://www.openstreetmap.org/copyright). OpenMapTiles' schema and design are under CC BY 4.0 \
                (https://github.com/openmaptiles/openmaptiles/blob/master/LICENSE.md).",
            "attribution": "OpenFreeMap © OpenMapTiles Data from OpenStreetMap",
        },
        {
            "files": ["styles/liberty.json", "styles/LICENSE.md"],
            "origin": format!("OpenFreeMap's Liberty style, {STYLE}, changed as style_changes says"),
            "license": "MIT (OpenFreeMap styles), from OSM Liberty and OSM Bright: code under the BSD 3-Clause \
                License, design under CC BY 4.0; styles/LICENSE.md has the terms.",
        },
        {
            "files": ["sprites/"],
            "origin": format!("The style's sprite, {sprite_url}"),
            "license": "The Maki icons, CC0 1.0; the right arrow from Wikimedia Commons, public domain \
                (styles/LICENSE.md).",
        },
        {
            "files": ["glyphs/"],
            "origin": format!("OpenFreeMap's glyph ranges of Noto Sans, {GLYPHS}"),
            "license": "SIL Open Font License 1.1 (fonts/NotoSans-LICENSE.txt).",
        },
        {
            "files": ["fonts/"],
            "origin": "Noto Sans (notofonts/notofonts.github.io) and Noto Sans JP (notofonts/noto-cjk, Sans2.004)",
            "license": "SIL Open Font License 1.1 (fonts/NotoSans-LICENSE.txt, fonts/NotoSansJP-LICENSE.txt).",
        },
        {
            "files": [GEOJSON.path],
            "origin": "Natural Earth v5.1.2, Admin 0 – Countries at 1:10m (nvkelso/natural-earth-vector)",
            "license": "Public domain (https://www.naturalearthdata.com/about/terms-of-use/).",
        },
        {
            "files": ["tiles/terrain.pmtiles"],
            "origin": format!("Mapterhorn's terrain tiles, {TERRAIN_TILES}, as served when the snapshot was built"),
            "license": "Around San Francisco, USGS 3DEP elevation models, public domain (U.S. Government Work); \
                at low zooms, Copernicus GLO-30 under the Copernicus full, free and open license: © DLR e.V. \
                2010-2014 and © Airbus Defence and Space GmbH 2014-2018, provided under COPERNICUS by the \
                European Union and ESA, all rights reserved. Mapterhorn lists its sources at \
                https://mapterhorn.com/attribution.",
            "attribution": "© Mapterhorn",
        },
        {
            "files": [EARTHQUAKES.path],
            "origin": format!("The USGS's earthquake catalog, {}, as served when the snapshot was built", EARTHQUAKES.url),
            "license": "Public domain (U.S. Government Work, https://www.usgs.gov/information-policies-and-instructions/copyrights-and-credits).",
            "attribution": "U.S. Geological Survey",
        },
    ])
}
