//! The installed fixture snapshot, as a map reads it.

use std::path::{Path, PathBuf};

use mln_style::geo::LatLng;

use crate::manifest::{CACHE_VARIABLE, Manifest, TileSet, cache_root};

/// The snapshot's tile sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tiles {
    /// Central San Francisco, zooms 0 to 14.
    Street,
    /// The street tiles as MLT.
    StreetMlt,
    /// The whole world, zooms 0 to 4.
    World,
    /// Central Tokyo, zooms 0 to 14, its names in Japanese script.
    Cjk,
    /// The world to zoom 4, with central San Francisco and central Tokyo to
    /// zoom 14, for a flight from one city to the other.
    Flight,
    /// Elevation of central San Francisco, zooms 0 to 12: Terrarium-encoded
    /// WebP images of 512 pixels.
    Terrain,
}

impl Tiles {
    /// The set's name in the manifest.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Tiles::Street => "street",
            Tiles::StreetMlt => "street-mlt",
            Tiles::World => "world",
            Tiles::Cjk => "cjk",
            Tiles::Flight => "flight",
            Tiles::Terrain => "terrain",
        }
    }
}

/// The installed snapshot of [`Manifest::current`].
pub struct Snapshot {
    dir: PathBuf,
    manifest: &'static Manifest,
}

impl Snapshot {
    /// The snapshot, where it's installed. Fails, naming the command that
    /// installs it, when it isn't.
    pub fn find() -> Result<Snapshot, String> {
        Snapshot::find_in(Manifest::current(), &cache_root()?)
    }

    fn find_in(manifest: &'static Manifest, root: &Path) -> Result<Snapshot, String> {
        let dir = root.join(&manifest.hash);
        if !dir.is_dir() {
            return Err(format!(
                "the benchmark fixtures (snapshot {}) aren't installed in {}: run `mise run bench:fixtures` \
                 ({CACHE_VARIABLE} names another directory)",
                manifest.snapshot,
                root.display()
            ));
        }
        Ok(Snapshot { dir, manifest })
    }

    #[must_use]
    pub fn manifest(&self) -> &'static Manifest {
        self.manifest
    }

    /// The directory the snapshot is installed in: the asset path that
    /// resolves the style's `asset://` URLs.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The file at `path` in the snapshot, such as
    /// `geojson/ne_10m_admin_0_countries.geojson`. Panics for a path the
    /// manifest doesn't name.
    #[must_use]
    pub fn path(&self, path: &str) -> PathBuf {
        assert!(self.manifest.file(path).is_some(), "the fixtures have no {path}");
        self.dir.join(path)
    }

    /// The file at `path` in the snapshot, read. Panics for a path the
    /// manifest doesn't name, or a file that can't be read.
    #[must_use]
    pub fn read(&self, path: &str) -> Vec<u8> {
        let path = self.path(path);
        std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
    }

    /// OpenFreeMap's Liberty style, its vector source on `tiles`, as JSON.
    /// Its URLs are `asset://` URLs of the snapshot.
    #[must_use]
    pub fn style(&self, tiles: Tiles) -> String {
        self.style_value(tiles).to_string()
    }

    /// [`Snapshot::style`] as a JSON value, to change.
    #[must_use]
    pub fn style_value(&self, tiles: Tiles) -> serde_json::Value {
        let mut style: serde_json::Value =
            serde_json::from_slice(&self.read("styles/liberty.json")).expect("the style is JSON");
        let url = format!("pmtiles://asset://{}", self.tile_set(tiles).archive);
        style["sources"]["openmaptiles"]["url"] = url.into();
        style
    }

    /// A `raster-dem` source of the terrain tiles.
    #[must_use]
    pub fn terrain_source(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "raster-dem",
            "url": format!("pmtiles://asset://{}", self.tile_set(Tiles::Terrain).archive),
            "encoding": "terrarium",
            "tileSize": 512,
        })
    }

    /// A GeoJSON source of the earthquakes of October 2026, 11,342 points
    /// with their magnitude (`mag`) among their properties.
    #[must_use]
    pub fn earthquakes_source(&self) -> serde_json::Value {
        serde_json::json!({ "type": "geojson", "data": format!("asset://{EARTHQUAKES}") })
    }

    /// Liberty over the world's tiles, with the earthquakes drawn as an
    /// application that maps them draws them, below the labels: a heatmap of
    /// them weighted by magnitude that fades out from zoom 7 to 9, and from
    /// zoom 7 a circle for each, sized by magnitude and coloured by depth.
    #[must_use]
    pub fn earthquakes_style(&self) -> String {
        let mut style = self.style_value(Tiles::World);
        style["sources"]["earthquakes"] = self.earthquakes_source();
        let heatmap = serde_json::json!({
            "id": "earthquakes-heat", "type": "heatmap", "source": "earthquakes", "maxzoom": 9,
            "paint": {
                "heatmap-weight": ["interpolate", ["linear"], ["get", "mag"], 0, 0, 6, 1],
                "heatmap-intensity": ["interpolate", ["linear"], ["zoom"], 0, 1, 9, 3],
                "heatmap-color": ["interpolate", ["linear"], ["heatmap-density"],
                    0, "rgba(33,102,172,0)", 0.2, "rgb(103,169,207)", 0.4, "rgb(209,229,240)",
                    0.6, "rgb(253,219,199)", 0.8, "rgb(239,138,98)", 1, "rgb(178,24,43)"],
                "heatmap-radius": ["interpolate", ["linear"], ["zoom"], 0, 2, 9, 20],
                "heatmap-opacity": ["interpolate", ["linear"], ["zoom"], 7, 1, 9, 0],
            },
        });
        let circles = serde_json::json!({
            "id": "earthquakes-point", "type": "circle", "source": "earthquakes", "minzoom": 7,
            "paint": {
                "circle-radius": ["interpolate", ["linear"], ["zoom"],
                    7, ["interpolate", ["linear"], ["get", "mag"], 1, 1, 6, 4],
                    16, ["interpolate", ["linear"], ["get", "mag"], 1, 5, 6, 50]],
                "circle-color": ["interpolate", ["linear"], ["coalesce", ["get", "depth"], 0],
                    0, "rgb(178,24,43)", 50, "rgb(239,138,98)", 300, "rgb(103,169,207)"],
                "circle-stroke-color": "white",
                "circle-stroke-width": 1,
                "circle-opacity": ["interpolate", ["linear"], ["zoom"], 7, 0, 8, 1],
            },
        });
        insert_below_labels(&mut style, [heatmap, circles]);
        style.to_string()
    }

    /// Liberty over the street tiles, shaded by the terrain as an outdoors
    /// map is: its land tinted by elevation (color relief) and hillshaded,
    /// both above the land cover and below the roads.
    #[must_use]
    pub fn terrain_style(&self) -> String {
        let mut style = self.style_value(Tiles::Street);
        style["sources"]["terrain"] = self.terrain_source();
        let relief = serde_json::json!({
            "id": "terrain-relief", "type": "color-relief", "source": "terrain",
            "paint": {
                "color-relief-color": ["interpolate", ["linear"], ["elevation"],
                    0, "rgba(112,153,89,0.3)", 100, "rgba(242,238,162,0.3)",
                    250, "rgba(222,184,135,0.3)", 500, "rgba(166,123,91,0.3)"],
            },
        });
        let hillshade = serde_json::json!({
            "id": "terrain-hillshade", "type": "hillshade", "source": "terrain",
            "paint": { "hillshade-exaggeration": 0.5, "hillshade-shadow-color": "#473B24" },
        });
        let layers = style["layers"].as_array_mut().expect("a style's layers");
        let roads =
            layers.iter().position(|layer| layer["source-layer"] == "transportation").expect("Liberty draws roads");
        layers.splice(roads..roads, [relief, hillshade]);
        style.to_string()
    }

    /// [`Snapshot::terrain_style`] in 3D: the terrain tiles that shade it
    /// also raise its ground, which its fills, lines, color relief and
    /// hillshade drape over and its labels stand on.
    #[must_use]
    pub fn terrain_3d_style(&self) -> String {
        let mut style: serde_json::Value = serde_json::from_str(&self.terrain_style()).expect("the style is JSON");
        style["terrain"] = serde_json::json!({ "source": "terrain" });
        style.to_string()
    }

    /// Liberty over the street tiles with its POI labels placed as many
    /// styles place them: each tries the anchors below, beside and above its
    /// icon (`text-variable-anchor`) instead of one.
    #[must_use]
    pub fn labels_style(&self) -> String {
        let mut style = self.style_value(Tiles::Street);
        for layer in style["layers"].as_array_mut().expect("a style's layers") {
            if layer["source-layer"] != "poi" {
                continue;
            }
            let layout = layer["layout"].as_object_mut().expect("a POI layer's layout");
            layout.remove("text-anchor");
            layout.remove("text-offset");
            layout.insert("text-variable-anchor".into(), serde_json::json!(["top", "bottom", "left", "right"]));
            layout.insert("text-radial-offset".into(), serde_json::json!(0.8));
            layout.insert("text-justify".into(), serde_json::json!("auto"));
        }
        style.to_string()
    }

    /// The style over the world's tiles, on the globe.
    #[must_use]
    pub fn globe_style(&self) -> String {
        let mut style = self.style_value(Tiles::World);
        style["projection"] = serde_json::json!({ "type": "globe" });
        style.to_string()
    }

    /// The manifest's description of a tile set: its archive, and the area a
    /// camera can center on and see only tiles of the set.
    #[must_use]
    pub fn tile_set(&self, tiles: Tiles) -> &'static TileSet {
        &self.manifest.tile_sets[tiles.name()]
    }

    /// The center of the area of `tiles`.
    #[must_use]
    pub fn area_center(&self, tiles: Tiles) -> LatLng {
        let [west, south, east, north] = self.tile_set(tiles).area;
        LatLng::new(f64::midpoint(south, north), f64::midpoint(west, east)).expect("an area's center")
    }
}

/// The earthquakes' GeoJSON in the snapshot.
pub const EARTHQUAKES: &str = "geojson/earthquakes-2026-10.geojson";

/// Inserts `layers` into `style` below its first symbol layer.
fn insert_below_labels(style: &mut serde_json::Value, layers: impl IntoIterator<Item = serde_json::Value>) {
    let all = style["layers"].as_array_mut().expect("a style's layers");
    let labels = all.iter().position(|layer| layer["type"] == "symbol").unwrap_or(all.len());
    all.splice(labels..labels, layers);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_snapshot_names_the_command_that_installs_it() {
        let root = std::env::temp_dir().join(format!("mln-bench-workload-test-{}", std::process::id()));
        let Err(error) = Snapshot::find_in(Manifest::current(), &root) else {
            panic!("an empty directory holds no snapshot");
        };
        assert!(error.contains("run `mise run bench:fixtures`"), "{error}");
        assert!(error.contains(&root.display().to_string()), "{error}");
    }
}
