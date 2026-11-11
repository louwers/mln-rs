// Ports include/mln/gfx/rendering_stats.hpp (the counters).
//
// TODO(port): `RenderingStats::toString`/`toJSONString`/`eval`
// (src/mln/gfx/rendering_stats.cpp) arrive with their first user.

use std::collections::{HashMap, HashSet};

use mln_tile_id::OverscaledTileID;

/// `RenderingStats::NDCBound`: a bounding box in normalized device
/// coordinates, positive up and right. The default is empty (inverted), so
/// that [`include`](Self::include) of any bound yields that bound.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NDCBound {
    pub min_x: f64,
    pub max_x: f64,
    pub min_y: f64,
    pub max_y: f64,
}

impl Default for NDCBound {
    fn default() -> Self {
        NDCBound { min_x: f64::MAX, max_x: f64::MIN, min_y: f64::MAX, max_y: f64::MIN }
    }
}

impl NDCBound {
    /// Expand to include the other bounds.
    pub fn include(&mut self, other: &NDCBound) {
        self.min_x = self.min_x.min(other.min_x);
        self.max_x = self.max_x.max(other.max_x);
        self.min_y = self.min_y.min(other.min_y);
        self.max_y = self.max_y.max(other.max_y);
    }
}

/// `RenderingStats::FeatureInfo`: where a feature was rendered in a frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FeatureInfo {
    /// The bounding box for this feature.
    pub ndc_bound: NDCBound,
    /// The tiles in which this item was rendered (including overdraw).
    pub tile_ids: HashSet<OverscaledTileID>,
}

impl FeatureInfo {
    pub fn merge_from(&mut self, bound: &NDCBound, ids: &HashSet<OverscaledTileID>) {
        self.ndc_bound.include(bound);
        self.tile_ids.extend(ids.iter().copied());
    }
}

/// `RenderingStats::SourceLayerID`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SourceLayerID {
    pub source_id: String,
    pub layer_id: String,
}

/// Rendered features of one layer, by feature ID.
pub type LayerFeaturesMap = HashMap<String, FeatureInfo>;
/// Rendered features of a frame, by source and layer.
pub type FrameRenderedFeaturesMap = HashMap<SourceLayerID, LayerFeaturesMap>;

/// `RenderLayer::Stats::addRenderedFeature`: add an entry or merge with an
/// existing one.
pub fn add_rendered_feature(
    into: &mut LayerFeaturesMap,
    feature_id: &str,
    bound: &NDCBound,
    tile_ids: HashSet<OverscaledTileID>,
) {
    if let Some(info) = into.get_mut(feature_id) {
        info.merge_from(bound, &tile_ids);
    } else {
        into.insert(feature_id.to_owned(), FeatureInfo { ndc_bound: *bound, tile_ids });
    }
}

/// `RenderLayer::Stats::merge`.
pub fn merge_layer_features(into: &mut LayerFeaturesMap, from: LayerFeaturesMap) {
    for (feature_id, info) in from {
        match into.entry(feature_id) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                entry.get_mut().merge_from(&info.ndc_bound, &info.tile_ids);
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(info);
            }
        }
    }
}

/// Counters of the rendering backend. The `num_*` and `mem_*` counters of
/// buffers, textures and frame buffers count the live resources; `total_*`,
/// `*_updates` and `*_bytes` grow over the context's life.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenderingStats {
    /// Frame CPU encoding time (seconds).
    pub encoding_time: f64,
    /// Frame CPU rendering time (seconds).
    pub rendering_time: f64,
    /// Number of frames rendered.
    pub num_frames: i32,
    /// Draw calls executed during the most recent frame.
    pub num_draw_calls: i32,
    /// Draw calls executed during all the frames.
    pub total_draw_calls: i32,
    /// Textures created.
    pub num_created_textures: i32,
    /// Live textures.
    pub num_active_textures: i32,
    /// Net texture bindings.
    pub num_texture_bindings: i32,
    /// Times a texture was updated.
    pub num_texture_updates: i32,
    /// Bytes written in texture updates.
    pub texture_update_bytes: usize,
    /// Buffers created.
    pub total_buffers: usize,
    /// Backend buffer objects created.
    pub total_buffer_objs: usize,
    /// Times a buffer was updated.
    pub buffer_updates: usize,
    /// Times a backend buffer object was updated.
    pub buffer_obj_updates: usize,
    /// Bytes written in buffer updates.
    pub buffer_update_bytes: usize,
    /// Live buffers of every kind.
    pub num_buffers: i32,
    /// Live offscreen frame buffers.
    pub num_frame_buffers: i32,
    /// Live index buffers.
    pub num_index_buffers: i32,
    /// Bytes written in index buffer updates.
    pub index_update_bytes: usize,
    /// Live vertex buffers.
    pub num_vertex_buffers: i32,
    /// Bytes written in vertex buffer updates.
    pub vertex_update_bytes: usize,
    /// Live uniform and storage buffers.
    pub num_uniform_buffers: i32,
    /// Times a uniform or storage buffer was updated.
    pub num_uniform_updates: i32,
    /// Bytes written in uniform and storage buffer updates.
    pub uniform_update_bytes: usize,
    /// Bytes of the live textures.
    pub mem_textures: i32,
    /// Bytes of the live buffers of every kind.
    pub mem_buffers: i32,
    /// Bytes of the live index buffers.
    pub mem_index_buffers: i32,
    /// Bytes of the live vertex buffers.
    pub mem_vertex_buffers: i32,
    /// Bytes of the live uniform and storage buffers.
    pub mem_uniform_buffers: i32,
    /// Stencil buffer clears.
    pub stencil_clears: i32,
    /// Stencil buffer updates.
    pub stencil_updates: i32,
    /// Collected feature information by layer, if enabled via
    /// `MapOptions::with_rendered_feature_info`.
    pub frame_rendered_features: FrameRenderedFeaturesMap,
}

impl RenderingStats {
    pub fn is_zero(&self) -> bool {
        *self == RenderingStats::default()
    }
}
