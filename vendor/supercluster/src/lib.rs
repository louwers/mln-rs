// Ports vendor/supercluster/include/supercluster.hpp (mapbox::supercluster:
// Cluster, Options, Supercluster).
//
// Property maps are `feature::PropertyMap` (key order; C++:
// std::unordered_map, which only shows in iteration order).
// `std::function` map/reduce options are shared closures.
// Exceptions (`std::runtime_error`, `bad_variant_access`) are `Err`s with
// the C++ `what()`.

use std::collections::HashMap;
use std::f64::consts::PI;
use std::sync::Arc;

use feature::{Feature, Identifier, PropertyMap, Value};
use geometry::{Geometry, Point};
use kdbush::{KDBush, Nth};

/// `feature_collection<double>`
pub(crate) type GeoJSONFeatures = Vec<Feature<f64>>;
/// `feature_collection<std::int16_t>`
pub(crate) type TileFeatures = Vec<Feature<i16>>;

/// `Cluster`
#[derive(Clone, Debug)]
pub(crate) struct Cluster {
    pub(crate) pos: Point<f64>,
    pub(crate) num_points: u32,
    pub(crate) id: u32,
    pub(crate) parent_id: u32,
    pub(crate) visited: bool,
    pub(crate) properties: Option<Box<PropertyMap>>,
}

impl Cluster {
    /// `Cluster(pos, num_points, id)`
    #[must_use]
    pub(crate) fn new(pos: Point<f64>, num_points: u32, id: u32) -> Self {
        Cluster { pos, num_points, id, parent_id: 0, visited: false, properties: None }
    }

    /// `Cluster(pos, num_points, id, properties)`: keeps the properties only
    /// if there are any.
    #[must_use]
    pub(crate) fn with_properties(pos: Point<f64>, num_points: u32, id: u32, properties: &PropertyMap) -> Self {
        let mut c = Self::new(pos, num_points, id);
        if !properties.is_empty() {
            c.properties = Some(Box::new(properties.clone()));
        }
        c
    }

    /// `toGeoJSON()`
    #[must_use]
    pub(crate) fn to_geojson(&self) -> Feature<f64> {
        let x = (self.pos.x - 0.5) * 360.0;
        let y = 360.0 * ((180.0 - self.pos.y * 360.0) * PI / 180.0).exp().atan() / PI - 90.0;
        Feature {
            geometry: Geometry::Point(Point::new(x, y)),
            properties: self.get_properties(),
            id: Identifier::Uint(u64::from(self.id)),
        }
    }

    /// `getProperties()`
    #[must_use]
    pub(crate) fn get_properties(&self) -> PropertyMap {
        let mut result = PropertyMap::new();
        result.insert("cluster".to_owned(), Value::Bool(true));
        result.insert("cluster_id".to_owned(), Value::Uint(u64::from(self.id)));
        result.insert("point_count".to_owned(), Value::Uint(u64::from(self.num_points)));
        result
            .entry("point_count_abbreviated".to_owned())
            .or_insert_with(|| Value::String(abbreviate(self.num_points)));
        if let Some(properties) = &self.properties {
            for (k, v) in properties.iter() {
                result.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        result
    }
}

/// The `std::stringstream` text of `point_count_abbreviated`: from 1000 on
/// `std::fixed` with `setprecision(1)` below 10000 and the default
/// precision (6) above, then "k". (`%.1f`/`%.6f` of the exact double,
/// glibc's round-half-even on exact ties, which Rust's formatting matches.)
#[must_use]
pub(crate) fn abbreviate(num_points: u32) -> String {
    if num_points >= 1000 {
        let k = f64::from(num_points) / 1000.0;
        if num_points < 10000 { format!("{k:.1}k") } else { format!("{k:.6}k") }
    } else {
        num_points.to_string()
    }
}

impl Nth for Cluster {
    type Number = f64;
    fn nth0(&self) -> f64 {
        self.pos.x
    }
    fn nth1(&self) -> f64 {
        self.pos.y
    }
}

/// `std::function<property_map(const property_map&)>`
pub(crate) type MapFn = Arc<dyn Fn(&PropertyMap) -> PropertyMap + Send + Sync>;
/// `std::function<void(property_map&, const property_map&)>`
pub(crate) type ReduceFn = Arc<dyn Fn(&mut PropertyMap, &PropertyMap) + Send + Sync>;

/// `Options`
#[derive(Clone)]
pub struct Options {
    /// min zoom to generate clusters on
    pub min_zoom: u8,
    /// max zoom level to cluster the points on
    pub max_zoom: u8,
    /// cluster radius in pixels
    pub radius: u16,
    /// tile extent (radius is calculated relative to it)
    pub extent: u16,
    /// minimum points to form a cluster
    pub min_points: usize,
    /// whether to generate numeric ids for input features (in vector tiles)
    pub generate_id: bool,

    pub map: MapFn,
    pub reduce: Option<ReduceFn>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            min_zoom: 0,
            max_zoom: 16,
            radius: 40,
            extent: 512,
            min_points: 2,
            generate_id: false,
            map: Arc::new(|p: &PropertyMap| p.clone()),
            reduce: None,
        }
    }
}

impl std::fmt::Debug for Options {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Options")
            .field("min_zoom", &self.min_zoom)
            .field("max_zoom", &self.max_zoom)
            .field("radius", &self.radius)
            .field("extent", &self.extent)
            .field("min_points", &self.min_points)
            .field("generate_id", &self.generate_id)
            .field("reduce", &self.reduce.is_some())
            .finish_non_exhaustive()
    }
}

/// `Supercluster::Zoom`
#[derive(Clone, Debug, Default)]
struct Zoom {
    tree: KDBush<f64, u32>,
    clusters: Vec<Cluster>,
}

impl Zoom {
    /// `Zoom(features, options)`: a cluster for each point.
    fn from_features(features: &GeoJSONFeatures, options: &Options) -> Result<Self, String> {
        let mut zoom = Zoom::default();
        let mut i: u32 = 0;
        zoom.clusters.reserve(features.len());
        for f in features {
            // `f.geometry.get<GeoJSONPoint>()`
            let Geometry::Point(p) = &f.geometry else {
                return Err("in get<T>()".to_owned());
            };
            if options.reduce.is_some() {
                let cluster_properties = (options.map)(&f.properties);
                zoom.clusters.push(Cluster::with_properties(project(p), 1, i, &cluster_properties));
            } else {
                zoom.clusters.push(Cluster::new(project(p), 1, i));
            }
            i = i.wrapping_add(1);
        }
        zoom.tree.fill(&zoom.clusters);
        Ok(zoom)
    }

    /// `Zoom(previous, r, zoom, options)`: clusters points from the previous
    /// (higher) zoom.
    fn from_previous(previous: &mut Zoom, r: f64, zoom: u8, options: &Options) -> Self {
        // The zoom parameter is restricted to [minZoom, maxZoom] by caller
        debug_assert_eq!(((u32::from(zoom) + 1) & 0b11111), (u32::from(zoom) + 1));

        let mut result = Zoom::default();
        let clusters = &mut result.clusters;

        // Since point index is encoded in the upper 27 bits, clamp the count
        // of clusters
        let previous_clusters_size = previous.clusters.len().min(0x07ff_ffff);

        for i in 0..previous_clusters_size {
            if previous.clusters[i].visited {
                continue;
            }

            previous.clusters[i].visited = true;

            let p_pos = previous.clusters[i].pos;
            let num_points_origin = previous.clusters[i].num_points;
            let mut num_points = num_points_origin;
            // count the number of points in a potential cluster
            {
                let prev = &previous.clusters;
                previous.tree.within(p_pos.x, p_pos.y, r, &mut |neighbor_id| {
                    let b = &prev[neighbor_id as usize];
                    // filter out neighbors that are already processed
                    if !b.visited {
                        num_points = num_points.wrapping_add(b.num_points);
                    }
                });
            }

            let mut cluster_properties = previous.clusters[i].properties.as_deref().cloned().unwrap_or_default();
            if num_points as usize >= options.min_points {
                // enough points to form a cluster
                let mut weight =
                    Point::new(p_pos.x * f64::from(num_points_origin), p_pos.y * f64::from(num_points_origin));
                let id = ((i as u64) << 5).wrapping_add(u64::from(zoom) + 1) as u32;

                // find all nearby points
                let (tree, prev) = (&previous.tree, &mut previous.clusters);
                tree.within(p_pos.x, p_pos.y, r, &mut |neighbor_id| {
                    let b = &mut prev[neighbor_id as usize];

                    // filter out neighbors that are already processed
                    if b.visited {
                        return;
                    }

                    b.visited = true;
                    b.parent_id = id;

                    // accumulate coordinates for calculating weighted center
                    weight.x += b.pos.x * f64::from(b.num_points);
                    weight.y += b.pos.y * f64::from(b.num_points);

                    if let (Some(reduce), Some(properties)) = (&options.reduce, &b.properties) {
                        // apply reduce function to update clusterProperites
                        reduce(&mut cluster_properties, properties);
                    }
                });
                previous.clusters[i].parent_id = id;
                clusters.push(Cluster::with_properties(
                    Point::new(weight.x / f64::from(num_points), weight.y / f64::from(num_points)),
                    num_points,
                    id,
                    &cluster_properties,
                ));
            } else {
                clusters.push(Cluster::with_properties(p_pos, 1, previous.clusters[i].id, &cluster_properties));
                if num_points > 1 {
                    let (tree, prev) = (&previous.tree, &mut previous.clusters);
                    tree.within(p_pos.x, p_pos.y, r, &mut |neighbor_id| {
                        let b = &mut prev[neighbor_id as usize];
                        // filter out neighbors that are already processed
                        if b.visited {
                            return;
                        }
                        b.visited = true;
                        let properties = b.properties.as_deref().cloned().unwrap_or_default();
                        clusters.push(Cluster::with_properties(b.pos, 1, b.id, &properties));
                    });
                }
            }
        }

        result.tree.fill(&result.clusters);
        result
    }
}

/// `Supercluster`
#[derive(Debug)]
pub struct Supercluster {
    pub(crate) features: GeoJSONFeatures,
    pub(crate) options: Options,
    zooms: HashMap<u8, Zoom>,
}

const NO_CLUSTER: &str = "No cluster with the specified id.";

impl Supercluster {
    /// `Supercluster(features, options)`; `Err` if a feature is not a point
    /// (C++: `bad_variant_access` `"in get<T>()"`).
    pub fn new(features: &GeoJSONFeatures, options: Options) -> Result<Self, String> {
        let mut index = Supercluster { features: features.clone(), options, zooms: HashMap::new() };
        let options = &index.options;

        // convert and index initial points
        let initial = Zoom::from_features(&index.features, options)?;
        index.zooms.insert(options.max_zoom.wrapping_add(1), initial);
        let mut z = i32::from(options.max_zoom);
        while z >= i32::from(options.min_zoom) {
            // cluster points from the previous zoom level
            let r = f64::from(options.radius) / (f64::from(options.extent) * 2.0f64.powf(f64::from(z)));
            // `zooms[z + 1]` default-constructs a missing level.
            let mut previous = index.zooms.remove(&((z + 1) as u8)).unwrap_or_default();
            let zoom = Zoom::from_previous(&mut previous, r, z as u8, options);
            index.zooms.insert((z + 1) as u8, previous);
            index.zooms.entry(z as u8).or_insert(zoom);
            z -= 1;
        }

        Ok(index)
    }

    /// `getTile(z, x, y)`
    #[must_use]
    pub fn get_tile(&self, z: u8, x_: u32, y: u32) -> TileFeatures {
        let mut result = TileFeatures::new();

        let zoom = self.zooms.get(&self.limit_zoom(z)).expect("zoom_iter != zooms.end()");

        let z2 = 2.0f64.powf(f64::from(z)) as u32;
        let r = f64::from(self.options.radius) / f64::from(self.options.extent);
        let mut x = x_ as i32;
        let extent = f64::from(self.options.extent);
        let yf = f64::from(y);

        let mut visitor = |x: i32, id: u32| {
            let c = &zoom.clusters[id as usize];

            let point = Point::new(
                (extent * (c.pos.x * f64::from(z2) - f64::from(x))).round() as i16,
                (extent * (c.pos.y * f64::from(z2) - yf)).round() as i16,
            );

            if c.num_points == 1 {
                let original_feature = &self.features[c.id as usize];
                // Generate feature id if options.generateId is set.
                let feature_id = if self.options.generate_id {
                    Identifier::Uint(u64::from(c.id))
                } else {
                    original_feature.id.clone()
                };
                result.push(Feature {
                    geometry: Geometry::Point(point),
                    properties: original_feature.properties.clone(),
                    id: feature_id,
                });
            } else {
                result.push(Feature {
                    geometry: Geometry::Point(point),
                    properties: c.get_properties(),
                    id: Identifier::Uint(u64::from(c.id)),
                });
            }
        };

        let z2f = f64::from(z2);
        let top = (yf - r) / z2f;
        let bottom = (f64::from(y.wrapping_add(1)) + r) / z2f;

        zoom.tree.range((f64::from(x) - r) / z2f, top, (f64::from(x.wrapping_add(1)) + r) / z2f, bottom, &mut |id| {
            visitor(x, id);
        });

        if x_ == 0 {
            x = z2 as i32;
            zoom.tree.range(1.0 - r / z2f, top, 1.0, bottom, &mut |id| visitor(x, id));
        }
        if x_ == z2.wrapping_sub(1) {
            x = -1;
            zoom.tree.range(0.0, top, r / z2f, bottom, &mut |id| visitor(x, id));
        }

        result
    }

    /// `getChildren(cluster_id)`
    pub fn get_children(&self, cluster_id: u32) -> Result<GeoJSONFeatures, String> {
        let mut children = GeoJSONFeatures::new();
        self.each_child(cluster_id, &mut |c| {
            children.push(self.cluster_to_geojson(c));
            Ok(())
        })?;
        Ok(children)
    }

    /// `getLeaves(cluster_id, limit, offset)` (C++ defaults: 10, 0)
    pub fn get_leaves(&self, cluster_id: u32, limit: u32, offset: u32) -> Result<GeoJSONFeatures, String> {
        let mut leaves = GeoJSONFeatures::new();
        let mut skipped: u32 = 0;
        let mut limit_ = limit;
        self.each_leaf(cluster_id, &mut limit_, offset, &mut skipped, &mut |c| {
            leaves.push(self.cluster_to_geojson(c));
        })?;
        Ok(leaves)
    }

    /// `getClusterExpansionZoom(cluster_id)`
    pub fn get_cluster_expansion_zoom(&self, cluster_id: u32) -> Result<u8, String> {
        let mut cluster_id = cluster_id;
        let mut cluster_zoom: u32 = (cluster_id % 32).wrapping_sub(1);
        while cluster_zoom <= u32::from(self.options.max_zoom) {
            let mut num_children: u32 = 0;

            let mut next_id = cluster_id;
            self.each_child(cluster_id, &mut |c| {
                num_children = num_children.wrapping_add(1);
                next_id = c.id;
                Ok(())
            })?;
            cluster_id = next_id;

            cluster_zoom = cluster_zoom.wrapping_add(1);

            if num_children != 1 {
                break;
            }
        }
        Ok(cluster_zoom as u8)
    }

    /// `limitZoom(z)`
    fn limit_zoom(&self, z: u8) -> u8 {
        if z < self.options.min_zoom {
            return self.options.min_zoom;
        }
        if i32::from(z) > i32::from(self.options.max_zoom) + 1 {
            return self.options.max_zoom.wrapping_add(1);
        }
        z
    }

    /// `eachChild(cluster_id, visitor)`; the visitor's `Err` (an exception
    /// from a nested `eachChild`) propagates.
    fn each_child(
        &self,
        cluster_id: u32,
        visitor: &mut dyn FnMut(&Cluster) -> Result<(), String>,
    ) -> Result<(), String> {
        let origin_id = cluster_id >> 5;
        let origin_zoom = cluster_id % 32;

        let Some(zoom) = self.zooms.get(&(origin_zoom as u8)) else {
            return Err(NO_CLUSTER.to_owned());
        };

        if origin_id as usize >= zoom.clusters.len() {
            return Err(NO_CLUSTER.to_owned());
        }

        // `std::pow(2, origin_zoom - 1)` in unsigned arithmetic: 2^(2^32 - 1)
        // = inf for origin_zoom 0.
        let r = f64::from(self.options.radius)
            / (f64::from(self.options.extent) * 2.0f64.powf(f64::from(origin_zoom.wrapping_sub(1))));
        let origin = &zoom.clusters[origin_id as usize];

        let mut has_children = false;
        let mut error = None;

        zoom.tree.within(origin.pos.x, origin.pos.y, r, &mut |id| {
            if error.is_some() {
                return;
            }
            let cluster_child = &zoom.clusters[id as usize];
            if cluster_child.parent_id == cluster_id {
                if let Err(e) = visitor(cluster_child) {
                    error = Some(e);
                    return;
                }
                has_children = true;
            }
        });

        if let Some(e) = error {
            return Err(e);
        }

        if !has_children {
            return Err(NO_CLUSTER.to_owned());
        }
        Ok(())
    }

    /// `eachLeaf(cluster_id, limit, offset, skipped, visitor)`
    fn each_leaf(
        &self,
        cluster_id: u32,
        limit: &mut u32,
        offset: u32,
        skipped: &mut u32,
        visitor: &mut dyn FnMut(&Cluster),
    ) -> Result<(), String> {
        self.each_child(cluster_id, &mut |cluster_leaf| {
            if *limit == 0 {
                return Ok(());
            }
            if cluster_leaf.num_points > 1 {
                if skipped.wrapping_add(cluster_leaf.num_points) <= offset {
                    // skip the whole cluster
                    *skipped = skipped.wrapping_add(cluster_leaf.num_points);
                } else {
                    // enter the cluster
                    self.each_leaf(cluster_leaf.id, limit, offset, skipped, visitor)?;
                    // exit the cluster
                }
            } else if *skipped < offset {
                // skip a single point
                *skipped = skipped.wrapping_add(1);
            } else {
                // visit a single point
                visitor(cluster_leaf);
                *limit -= 1;
            }
            Ok(())
        })
    }

    /// `clusterToGeoJSON(c)`
    fn cluster_to_geojson(&self, c: &Cluster) -> Feature<f64> {
        if c.num_points == 1 { self.features[c.id as usize].clone() } else { c.to_geojson() }
    }
}

/// `project(p)`
#[must_use]
pub(crate) fn project(p: &Point<f64>) -> Point<f64> {
    let lng_x = p.x / 360.0 + 0.5;
    let sine = (p.y * PI / 180.0).sin();
    let y = 0.5 - 0.25 * ((1.0 + sine) / (1.0 - sine)).ln() / PI;
    // std::min(std::max(y, 0.0), 1.0)
    let y = if y < 0.0 { 0.0 } else { y };
    let lat_y = if 1.0 < y { 1.0 } else { y };
    Point::new(lng_x, lat_y)
}

#[cfg(test)]
mod tests;
