//! Point clustering, for GeoJSON sources with `cluster: true`.
//!
//! [`Clusters`] groups points into clusters for each zoom up to a maximum.
//! At each zoom, from the maximum down, it visits the points and clusters of
//! the zoom above in order; one that no cluster took yet, together with the
//! others within the radius that no cluster took, becomes a cluster at their
//! weighted center, if they hold at least the minimum number of points. A
//! cluster's children are therefore the points and clusters it took at the
//! zoom above.

use std::sync::Arc;

use geo_index::kdtree::{KDTree, KDTreeBuilder, KDTreeIndex};
use geo_types::{Coord, Geometry, Point};
use mln_style::feature::{Feature, Identifier, PropertyMap, Value};

use crate::{TileFeatures, mercator};

/// How points cluster.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// The highest zoom with clusters. Past it, every point stands alone.
    /// At most [`MAX_ZOOM`]; a higher one counts as [`MAX_ZOOM`].
    pub max_zoom: u8,
    /// How close points must be to cluster, in tile units.
    pub radius: f64,
    /// The size of a tile, in tile units.
    pub extent: u16,
    /// The fewest points that make a cluster.
    pub min_points: usize,
}

/// The highest [`Options::max_zoom`]: a cluster ID holds the zoom of its
/// children in five bits.
pub const MAX_ZOOM: u8 = 30;

/// The most points that can be clustered: a cluster ID holds the index of a
/// point at a zoom in the bits its zoom leaves.
pub const MAX_POINTS: usize = 1 << 27;

/// Computes a cluster's properties from its points' properties, as a GeoJSON
/// source's `clusterProperties` do.
pub trait Aggregate {
    /// The properties that a point with `properties` contributes.
    fn map(&self, properties: &PropertyMap) -> PropertyMap;

    /// Folds `other`, the contribution of a point or of a cluster, into
    /// `accumulated`.
    fn reduce(&self, accumulated: &mut PropertyMap, other: &PropertyMap);
}

/// Why points can't be clustered, or a cluster can't be found.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("feature {0} is not a point")]
    NotAPoint(usize),
    #[error("more than {MAX_POINTS} points to cluster")]
    TooManyPoints,
    #[error("no cluster with the specified id")]
    NoSuchCluster,
}

/// A cluster's ID: the zoom of its children, and the index there of the
/// child it started from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ClusterId(u32);

impl ClusterId {
    /// The ID of a cluster at `zoom` that started from the point or cluster
    /// at `origin` in the zoom above.
    fn new(origin: usize, zoom: u8) -> Self {
        debug_assert!(origin < MAX_POINTS && zoom <= MAX_ZOOM);
        ClusterId(((origin as u32) << 5) | u32::from(zoom + 1))
    }

    fn origin(self) -> usize {
        (self.0 >> 5) as usize
    }

    fn child_zoom(self) -> usize {
        (self.0 & 0b11111) as usize
    }
}

/// A point or a cluster at one zoom.
#[derive(Clone, Debug)]
struct Node {
    /// The position, in Web Mercator's unit square.
    pos: Coord,
    kind: Kind,
    /// The cluster that took this node at the zoom below, if one did.
    parent: Option<ClusterId>,
    /// What the node contributes to its cluster's properties, when there's
    /// an [`Aggregate`].
    properties: Option<Arc<PropertyMap>>,
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    /// The point of the feature at this index.
    Point(u32),
    Cluster {
        id: ClusterId,
        num_points: u32,
    },
}

impl Node {
    fn num_points(&self) -> u32 {
        match self.kind {
            Kind::Point(_) => 1,
            Kind::Cluster { num_points, .. } => num_points,
        }
    }
}

/// The nodes of one zoom, and their index.
struct Level {
    nodes: Vec<Node>,
    /// Indexes `nodes` by position; `None` when there are none.
    tree: Option<KDTree<f64>>,
}

impl Level {
    fn new(nodes: Vec<Node>) -> Self {
        let tree = (!nodes.is_empty()).then(|| {
            // `nodes` has at most MAX_POINTS nodes.
            let mut builder = KDTreeBuilder::new(nodes.len() as u32);
            for node in &nodes {
                builder.add(node.pos.x, node.pos.y);
            }
            builder.finish()
        });
        Level { nodes, tree }
    }

    /// The indices of the nodes within `r` of `pos`.
    fn within(&self, pos: Coord, r: f64) -> Vec<u32> {
        self.tree.as_ref().map_or_else(Vec::new, |tree| tree.within(pos.x, pos.y, r))
    }

    /// The indices of the nodes in the box from `min` to `max`.
    fn range(&self, min: Coord, max: Coord) -> Vec<u32> {
        self.tree.as_ref().map_or_else(Vec::new, |tree| tree.range(min.x, min.y, max.x, max.y))
    }
}

/// Points, clustered for each zoom.
pub struct Clusters {
    features: Vec<Feature<f64, Arc<PropertyMap>>>,
    options: Options,
    /// The nodes of each zoom from 0 to `max_zoom + 1`, where every point
    /// stands alone.
    levels: Vec<Level>,
}

impl std::fmt::Debug for Clusters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Clusters")
            .field("points", &self.features.len())
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

impl Clusters {
    /// Clusters `features`, which must all be points, with cluster
    /// properties from `aggregate`.
    pub fn new(
        features: Vec<Feature<f64>>,
        options: Options,
        aggregate: Option<&dyn Aggregate>,
    ) -> Result<Self, Error> {
        if features.len() > MAX_POINTS {
            return Err(Error::TooManyPoints);
        }
        let options = Options { max_zoom: options.max_zoom.min(MAX_ZOOM), ..options };
        let features: Vec<_> = features
            .into_iter()
            .map(|f| Feature { geometry: f.geometry, properties: Arc::new(f.properties), id: f.id })
            .collect();

        let points = features
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let Some(Geometry::Point(p)) = f.geometry else { return Err(Error::NotAPoint(i)) };
                let properties = aggregate.map(|a| Arc::new(a.map(&f.properties)));
                Ok(Node { pos: mercator::project(p.0), kind: Kind::Point(i as u32), parent: None, properties })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let mut levels = Vec::with_capacity(usize::from(options.max_zoom) + 2);
        let mut above = Level::new(points);
        for zoom in (0..=options.max_zoom).rev() {
            let r = options.radius / (f64::from(options.extent) * 2f64.powi(i32::from(zoom)));
            let level = cluster(&mut above, zoom, r, options.min_points, aggregate);
            levels.push(std::mem::replace(&mut above, level));
        }
        levels.push(above);
        levels.reverse();
        Ok(Clusters { features, options, levels })
    }

    /// The points and clusters of tile `x`, `y` at zoom `z`, with those
    /// within the radius of its edges. A tile at the antimeridian also has
    /// those across it.
    #[must_use]
    pub fn tile(&self, z: u8, x: u32, y: u32) -> TileFeatures {
        let level = &self.levels[usize::from(z.min(self.options.max_zoom + 1))];
        let z2 = 2f64.powi(i32::from(z));
        let extent = f64::from(self.options.extent);
        let r = self.options.radius / extent;
        let (x, y) = (f64::from(x), f64::from(y));
        let (top, bottom) = ((y - r) / z2, (y + 1.0 + r) / z2);

        let mut features = TileFeatures::new();
        // Adds the nodes `ids` of tile column `tile_x`.
        let mut add = |ids: Vec<u32>, tile_x: f64| {
            for id in ids {
                let node = &level.nodes[id as usize];
                let point = Point::new(
                    (extent * (node.pos.x * z2 - tile_x)).round() as i16,
                    (extent * (node.pos.y * z2 - y)).round() as i16,
                );
                features.push(self.tile_feature(node, point));
            }
        };

        add(level.range(Coord { x: (x - r) / z2, y: top }, Coord { x: (x + 1.0 + r) / z2, y: bottom }), x);
        if x == 0.0 {
            add(level.range(Coord { x: 1.0 - r / z2, y: top }, Coord { x: 1.0, y: bottom }), z2);
        }
        if x == z2 - 1.0 {
            add(level.range(Coord { x: 0.0, y: top }, Coord { x: r / z2, y: bottom }), -1.0);
        }
        features
    }

    /// The points and clusters that cluster `cluster_id` took at the zoom
    /// above its own.
    pub fn children(&self, cluster_id: u32) -> Result<Vec<Feature<f64>>, Error> {
        Ok(self.child_nodes(ClusterId(cluster_id))?.into_iter().map(|node| self.geojson(node)).collect())
    }

    /// Up to `limit` of the points in cluster `cluster_id`, after skipping
    /// `offset` of them.
    pub fn leaves(&self, cluster_id: u32, limit: usize, offset: usize) -> Result<Vec<Feature<f64>>, Error> {
        let mut leaves = Vec::new();
        let mut skip = offset;
        self.append_leaves(ClusterId(cluster_id), limit, &mut skip, &mut leaves)?;
        Ok(leaves)
    }

    /// The zoom at which cluster `cluster_id` splits into more than one
    /// point or cluster.
    pub fn expansion_zoom(&self, cluster_id: u32) -> Result<u8, Error> {
        let mut id = ClusterId(cluster_id);
        loop {
            let children = self.child_nodes(id)?;
            let zoom = id.child_zoom();
            match children[..] {
                [&Node { kind: Kind::Cluster { id: child, .. }, .. }] if zoom <= usize::from(self.options.max_zoom) => {
                    id = child;
                }
                // At most MAX_ZOOM + 1.
                _ => return Ok(zoom as u8),
            }
        }
    }

    /// The nodes whose parent is cluster `id`.
    fn child_nodes(&self, id: ClusterId) -> Result<Vec<&Node>, Error> {
        let zoom = id.child_zoom();
        let level = self.levels.get(zoom).filter(|_| zoom > 0).ok_or(Error::NoSuchCluster)?;
        let origin = level.nodes.get(id.origin()).ok_or(Error::NoSuchCluster)?;
        let r = self.options.radius / (f64::from(self.options.extent) * 2f64.powi(zoom as i32 - 1));
        let children: Vec<&Node> = level
            .within(origin.pos, r)
            .into_iter()
            .map(|i| &level.nodes[i as usize])
            .filter(|node| node.parent == Some(id))
            .collect();
        if children.is_empty() { Err(Error::NoSuchCluster) } else { Ok(children) }
    }

    /// Appends the points of cluster `id` to `leaves` until it holds
    /// `limit`, skipping the first `skip` of them.
    fn append_leaves(
        &self,
        id: ClusterId,
        limit: usize,
        skip: &mut usize,
        leaves: &mut Vec<Feature<f64>>,
    ) -> Result<(), Error> {
        for child in self.child_nodes(id)? {
            if leaves.len() >= limit {
                break;
            }
            match child.kind {
                Kind::Cluster { id, num_points } => {
                    if *skip >= num_points as usize {
                        *skip -= num_points as usize;
                    } else {
                        self.append_leaves(id, limit, skip, leaves)?;
                    }
                }
                Kind::Point(_) if *skip > 0 => *skip -= 1,
                Kind::Point(_) => leaves.push(self.geojson(child)),
            }
        }
        Ok(())
    }

    /// A node as a tile's feature at `point`.
    fn tile_feature(&self, node: &Node, point: Point<i16>) -> Feature<i16, Arc<PropertyMap>> {
        let geometry = Some(Geometry::Point(point));
        match node.kind {
            Kind::Point(i) => {
                let feature = &self.features[i as usize];
                Feature { geometry, properties: Arc::clone(&feature.properties), id: feature.id.clone() }
            }
            Kind::Cluster { id, num_points } => Feature {
                geometry,
                properties: Arc::new(cluster_properties(id, num_points, node.properties.as_deref())),
                id: Identifier::Uint(u64::from(id.0)),
            },
        }
    }

    /// A node as a GeoJSON feature: a point's input feature, or a cluster's
    /// center and properties.
    fn geojson(&self, node: &Node) -> Feature<f64> {
        match node.kind {
            Kind::Point(i) => {
                let f = &self.features[i as usize];
                Feature {
                    geometry: f.geometry.clone(),
                    properties: PropertyMap::clone(&f.properties),
                    id: f.id.clone(),
                }
            }
            Kind::Cluster { id, num_points } => Feature {
                geometry: Some(Geometry::Point(Point(mercator::unproject(node.pos)))),
                properties: cluster_properties(id, num_points, node.properties.as_deref()),
                id: Identifier::Uint(u64::from(id.0)),
            },
        }
    }
}

/// Clusters the nodes of the zoom above `zoom`, setting their parents.
fn cluster(above: &mut Level, zoom: u8, r: f64, min_points: usize, aggregate: Option<&dyn Aggregate>) -> Level {
    let mut taken = vec![false; above.nodes.len()];
    let mut nodes = Vec::new();
    for i in 0..above.nodes.len() {
        if taken[i] {
            continue;
        }
        taken[i] = true;
        let origin = &above.nodes[i];
        let neighbors: Vec<usize> =
            above.within(origin.pos, r).into_iter().map(|n| n as usize).filter(|&n| !taken[n]).collect();
        let num_points = origin.num_points() + neighbors.iter().map(|&n| above.nodes[n].num_points()).sum::<u32>();

        if neighbors.is_empty() || (num_points as usize) < min_points {
            // Nothing to merge, or too few points: they stay as they are.
            nodes.push(Node { parent: None, ..origin.clone() });
            for n in neighbors {
                taken[n] = true;
                nodes.push(Node { parent: None, ..above.nodes[n].clone() });
            }
            continue;
        }

        let id = ClusterId::new(i, zoom);
        let mut weighted = origin.pos * f64::from(origin.num_points());
        let mut properties = aggregate.map(|_| origin.properties.as_deref().cloned().unwrap_or_default());
        above.nodes[i].parent = Some(id);
        for n in neighbors {
            taken[n] = true;
            let node = &mut above.nodes[n];
            node.parent = Some(id);
            weighted = weighted + node.pos * f64::from(node.num_points());
            if let (Some(aggregate), Some(properties), Some(other)) = (aggregate, &mut properties, &node.properties) {
                aggregate.reduce(properties, other);
            }
        }
        nodes.push(Node {
            pos: weighted / f64::from(num_points),
            kind: Kind::Cluster { id, num_points },
            parent: None,
            properties: properties.map(Arc::new),
        });
    }
    Level::new(nodes)
}

/// A cluster's properties: `cluster`, `cluster_id`, `point_count` and
/// `point_count_abbreviated`, and those it aggregated.
fn cluster_properties(id: ClusterId, num_points: u32, aggregated: Option<&PropertyMap>) -> PropertyMap {
    let mut properties = PropertyMap::new();
    properties.insert("cluster".to_owned(), Value::Bool(true));
    properties.insert("cluster_id".to_owned(), Value::Uint(u64::from(id.0)));
    properties.insert("point_count".to_owned(), Value::Uint(u64::from(num_points)));
    properties.insert("point_count_abbreviated".to_owned(), abbreviate(num_points));
    for (key, value) in aggregated.into_iter().flatten() {
        properties.entry(key.clone()).or_insert_with(|| value.clone());
    }
    properties
}

/// `count` for a label, as GL JS gives it: the number below 1,000, then
/// text such as "1.2k" and, from 10,000 on, "12k".
fn abbreviate(count: u32) -> Value {
    let thousands = f64::from(count) / 1000.0;
    if count >= 10_000 {
        Value::String(format!("{}k", thousands.round()))
    } else if count >= 1000 {
        Value::String(format!("{}k", (thousands * 10.0).round() / 10.0))
    } else {
        Value::Uint(u64::from(count))
    }
}

#[cfg(test)]
mod tests;
