// Ports include/mln/renderer/layer_group.hpp, src/mln/renderer/layer_group.cpp
// and src/mln/renderer/tile_layer_group.cpp
//
// Moved down from the renderer (see the crate docs): the backends implement
// the layer groups. Differences from C++:
// - `layerTweakers` / `addLayerTweaker` / `runTweakers` stay in mln_renderer
//   (`LayerTweaker` needs style types); this crate has no tweaker list.
// - `preRender`/`render`/`postRender` drop the unused `RenderOrchestrator&`.
// - `visitDrawables` runs the callback without the C++ try/catch (which
//   removed drawables whose callback threw and reported to the observer):
//   Rust callbacks don't throw; a panic propagates.
// - `TileLayerGroup` stores drawables in a `BTreeMap` keyed by (draw
//   priority, id), the order of the C++ `sortedDrawables` set, with the
//   (pass, tile) key alongside; C++ keeps an unordered_multimap plus the set.

use std::any::Any;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::identity::SimpleIdentity;

use crate::context_observer::ContextObserver;
use crate::drawable::{Drawable, UniqueDrawable};
use crate::paint_parameters::PaintParameters;
use crate::renderer::render_pass::RenderPass;
use crate::uniform_buffer::UniformBufferArray;
use crate::upload_pass::UploadPass;

/// `LayerGroupBase::Type`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Type {
    LayerGroup = 0,
    TileLayerGroup,
    Invalid = 255,
}

/// The data members of `LayerGroupBase` (and `util::SimpleIdentifiable`).
pub struct LayerGroupBaseData {
    pub unique_id: SimpleIdentity,
    pub type_: Type,
    pub enabled: bool,
    pub layer_index: i32,
    pub name: String,
    pub observer: Option<Rc<dyn ContextObserver>>,
}

impl LayerGroupBaseData {
    #[must_use]
    pub fn new(layer_index: i32, name: String, type_: Type) -> Self {
        LayerGroupBaseData { unique_id: SimpleIdentity::new(), type_, enabled: true, layer_index, name, observer: None }
    }
}

/// `LayerGroupBase`: a layer-like group of drawables, not a group of layers.
pub trait LayerGroupBase: Any {
    fn base(&self) -> &LayerGroupBaseData;
    fn base_mut(&mut self) -> &mut LayerGroupBaseData;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// The `static_cast<TileLayerGroup&>` used by `visitLayerGroupDrawables`
    /// and the render layers.
    fn as_tile_layer_group(&self) -> Option<&TileLayerGroup> {
        None
    }
    fn as_tile_layer_group_mut(&mut self) -> Option<&mut TileLayerGroup> {
        None
    }
    fn as_layer_group(&self) -> Option<&LayerGroup> {
        None
    }
    fn as_layer_group_mut(&mut self) -> Option<&mut LayerGroup> {
        None
    }

    /// Get the number of drawables contained
    fn get_drawable_count(&self) -> usize;
    /// Clear the drawable collection
    fn clear_drawables(&mut self) -> usize;

    /// Called before starting each frame
    fn pre_render(&mut self, _parameters: &mut dyn PaintParameters) {}
    /// Called during the upload pass
    fn upload(&mut self, _upload_pass: &dyn UploadPass) {}
    /// Called during each render pass
    fn render(&mut self, _parameters: &mut dyn PaintParameters) {}
    /// Called at the end of each frame
    fn post_render(&mut self, _parameters: &mut dyn PaintParameters) {}

    /// Get the uniform buffers attached to this layer group
    fn get_uniform_buffers(&self) -> &UniformBufferArray;
    fn mutable_uniform_buffers(&mut self) -> &mut UniformBufferArray;

    /// Update the layer index to a new value
    fn update_layer_index(&mut self, value: i32) {
        self.base_mut().layer_index = value;
    }

    fn get_id(&self) -> SimpleIdentity {
        self.base().unique_id
    }
    fn get_enabled(&self) -> bool {
        self.base().enabled
    }
    fn set_enabled(&mut self, value: bool) {
        self.base_mut().enabled = value;
    }
    fn get_type(&self) -> Type {
        self.base().type_
    }
    fn get_name(&self) -> &str {
        &self.base().name
    }
    fn set_name(&mut self, value: String) {
        self.base_mut().name = value;
    }
    fn get_layer_index(&self) -> i32 {
        self.base().layer_index
    }
    fn empty(&self) -> bool {
        self.get_drawable_count() == 0
    }
    /// `setObserver(nullptr)` restores the null observer.
    fn set_observer(&mut self, observer: Option<Rc<dyn ContextObserver>>) {
        self.base_mut().observer = observer;
    }
}

/// `std::shared_ptr<LayerGroupBase>` (also used for `TileLayerGroupPtr` and
/// `LayerGroupPtr`; reach the concrete group with `as_tile_layer_group_mut`).
pub type LayerGroupBasePtr = Rc<RefCell<dyn LayerGroupBase>>;

/// `LayerGroupBase::addDrawable`: init the drawable's tweakers.
fn init_drawable_tweakers(drawable: &mut dyn Drawable) {
    let tweakers = drawable.get_tweakers().clone();
    for tweaker in tweakers {
        tweaker.init(drawable);
    }
}

/// `TileLayerGroupTileKey`
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileLayerGroupTileKey {
    pub render_pass: RenderPass,
    pub tile_id: OverscaledTileID,
}

/// The `DrawableLessByPriority` key: (draw priority, id).
type SortKey = (i64, SimpleIdentity);

fn sort_key(drawable: &dyn Drawable) -> SortKey {
    (drawable.get_draw_priority(), drawable.get_id())
}

/// `TileLayerGroup`: a layer group for tile-based drawables. Backends embed
/// it and implement `LayerGroupBase` (render/upload, uniform buffers).
pub struct TileLayerGroup {
    pub base: LayerGroupBaseData,
    /// When stencil clipping is enabled for the layer, this is the set of
    /// tile IDs that need to be rendered to the stencil buffer. (C++ keeps the
    /// `RenderTiles`; only their ids are used.)
    pub stencil_tiles: Option<Rc<Vec<UnwrappedTileID>>>,
    drawables: BTreeMap<SortKey, (TileLayerGroupTileKey, UniqueDrawable)>,
}

impl TileLayerGroup {
    #[must_use]
    pub fn new(layer_index: i32, _initial_capacity: usize, name: String) -> Self {
        TileLayerGroup {
            base: LayerGroupBaseData::new(layer_index, name, Type::TileLayerGroup),
            stencil_tiles: None,
            drawables: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn get_drawable_count(&self) -> usize {
        self.drawables.len()
    }

    #[must_use]
    pub fn get_drawable_count_for(&self, pass: RenderPass, id: &OverscaledTileID) -> usize {
        let key = TileLayerGroupTileKey { render_pass: pass, tile_id: *id };
        self.drawables.values().filter(|(k, _)| *k == key).count()
    }

    pub fn remove_drawables(&mut self, pass: RenderPass, id: &OverscaledTileID) -> Vec<UniqueDrawable> {
        let key = TileLayerGroupTileKey { render_pass: pass, tile_id: *id };
        let keys: Vec<SortKey> = self.drawables.iter().filter(|(_, (k, _))| *k == key).map(|(s, _)| *s).collect();
        keys.into_iter().filter_map(|s| self.drawables.remove(&s)).map(|(_, d)| d).collect()
    }

    pub fn add_drawable(&mut self, pass: RenderPass, id: &OverscaledTileID, mut drawable: UniqueDrawable) {
        init_drawable_tweakers(drawable.as_mut());
        let key = sort_key(drawable.as_ref());
        let previous =
            self.drawables.insert(key, (TileLayerGroupTileKey { render_pass: pass, tile_id: *id }, drawable));
        debug_assert!(previous.is_none());
    }

    /// Call `f` for each drawable, in draw-priority order.
    pub fn visit_drawables(&mut self, mut f: impl FnMut(&mut dyn Drawable)) -> usize {
        for (_, drawable) in self.drawables.values_mut() {
            f(drawable.as_mut());
        }
        self.drawables.len()
    }

    pub fn visit_drawables_ref(&self, mut f: impl FnMut(&dyn Drawable)) -> usize {
        for (_, drawable) in self.drawables.values() {
            f(drawable.as_ref());
        }
        self.drawables.len()
    }

    /// Remove the drawables for which `f` returns true; returns the count.
    pub fn remove_drawables_if(&mut self, mut f: impl FnMut(&mut dyn Drawable) -> bool) -> usize {
        let old_size = self.drawables.len();
        self.drawables.retain(|_, (_, drawable)| !f(drawable.as_mut()));
        old_size - self.drawables.len()
    }

    /// Call the provided function for each drawable for the given tile
    pub fn visit_drawables_for(
        &mut self,
        pass: RenderPass,
        tile_id: &OverscaledTileID,
        mut f: impl FnMut(&mut dyn Drawable),
    ) -> usize {
        let key = TileLayerGroupTileKey { render_pass: pass, tile_id: *tile_id };
        let mut count = 0;
        for (k, drawable) in self.drawables.values_mut() {
            if *k == key {
                f(drawable.as_mut());
                count += 1;
            }
        }
        count
    }

    pub fn clear_drawables(&mut self) -> usize {
        let count = self.drawables.len();
        self.drawables.clear();
        count
    }

    pub fn set_stencil_tiles(&mut self, tiles: Option<Rc<Vec<UnwrappedTileID>>>) {
        self.stencil_tiles = tiles;
    }
}

/// `LayerGroup`: a layer group for non-tile-based drawables.
pub struct LayerGroup {
    pub base: LayerGroupBaseData,
    drawables: BTreeMap<SortKey, UniqueDrawable>,
}

impl LayerGroup {
    #[must_use]
    pub fn new(layer_index: i32, _initial_capacity: usize, name: String) -> Self {
        LayerGroup { base: LayerGroupBaseData::new(layer_index, name, Type::LayerGroup), drawables: BTreeMap::new() }
    }

    #[must_use]
    pub fn get_drawable_count(&self) -> usize {
        self.drawables.len()
    }

    /// Declared but not defined in C++ (no callers); implemented by pass bit.
    #[must_use]
    pub fn get_drawable_count_for(&self, pass: RenderPass) -> usize {
        self.drawables.values().filter(|d| d.has_render_pass(pass)).count()
    }

    /// Declared but not defined in C++ (no callers); implemented by pass bit.
    pub fn remove_drawables(&mut self, pass: RenderPass) -> Vec<UniqueDrawable> {
        let keys: Vec<SortKey> =
            self.drawables.iter().filter(|(_, d)| d.has_render_pass(pass)).map(|(k, _)| *k).collect();
        keys.into_iter().filter_map(|k| self.drawables.remove(&k)).collect()
    }

    pub fn add_drawable(&mut self, mut drawable: UniqueDrawable) {
        init_drawable_tweakers(drawable.as_mut());
        let key = sort_key(drawable.as_ref());
        self.drawables.insert(key, drawable);
    }

    pub fn visit_drawables(&mut self, mut f: impl FnMut(&mut dyn Drawable)) -> usize {
        for drawable in self.drawables.values_mut() {
            f(drawable.as_mut());
        }
        self.drawables.len()
    }

    pub fn visit_drawables_ref(&self, mut f: impl FnMut(&dyn Drawable)) -> usize {
        for drawable in self.drawables.values() {
            f(drawable.as_ref());
        }
        self.drawables.len()
    }

    /// C++ re-inserts the kept drawables into a new set so that a raised
    /// priority is re-sorted; the port re-keys them the same way.
    pub fn remove_drawables_if(&mut self, mut f: impl FnMut(&mut dyn Drawable) -> bool) -> usize {
        let old_size = self.drawables.len();
        let old = std::mem::take(&mut self.drawables);
        for (_, mut drawable) in old {
            if !f(drawable.as_mut()) {
                let key = sort_key(drawable.as_ref());
                self.drawables.insert(key, drawable);
            }
        }
        old_size - self.drawables.len()
    }

    pub fn clear_drawables(&mut self) -> usize {
        let count = self.drawables.len();
        self.drawables.clear();
        count
    }
}

/// `visitLayerGroupDrawables(layerGroup, f)`
pub fn visit_layer_group_drawables(layer_group: &mut dyn LayerGroupBase, f: impl FnMut(&mut dyn Drawable)) {
    match layer_group.get_type() {
        Type::LayerGroup => {
            if let Some(group) = layer_group.as_layer_group_mut() {
                group.visit_drawables(f);
            }
        }
        Type::TileLayerGroup => {
            if let Some(group) = layer_group.as_tile_layer_group_mut() {
                group.visit_drawables(f);
            }
        }
        Type::Invalid => {
            // C++ logs "Unknown layer group type" in debug builds.
        }
    }
}

/// `removeLayerGroupDrawablesIf(layerGroup, f)`
pub fn remove_layer_group_drawables_if(
    layer_group: &mut dyn LayerGroupBase,
    f: impl FnMut(&mut dyn Drawable) -> bool,
) -> usize {
    match layer_group.get_type() {
        Type::LayerGroup => layer_group.as_layer_group_mut().map_or(0, |g| g.remove_drawables_if(f)),
        Type::TileLayerGroup => layer_group.as_tile_layer_group_mut().map_or(0, |g| g.remove_drawables_if(f)),
        Type::Invalid => 0,
    }
}
