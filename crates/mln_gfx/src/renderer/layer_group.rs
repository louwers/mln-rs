use std::any::Any;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::identity::SimpleIdentity;
use mln_util::logging::{Event, Log};

use crate::bind_group::BindGroup;
use crate::context::Context;
use crate::depth_mode::DepthMode;
use crate::drawable::{Drawable, UniqueDrawable};
use crate::gpu::BindGroupId;
use crate::paint_parameters::PaintParameters;
use crate::renderer::render_pass::RenderPass;
use crate::stencil_mode::StencilMode;
use crate::types::DepthMaskType;
use crate::uniform_buffer::UniformBufferArray;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Type {
    LayerGroup = 0,
    TileLayerGroup,
    Invalid = 255,
}

/// The data members of `LayerGroupBase` (and `util::SimpleIdentifiable`).
pub struct LayerGroupBaseData {
    pub(crate) unique_id: SimpleIdentity,
    pub(crate) type_: Type,
    pub enabled: bool,
    pub(crate) layer_index: i32,
    pub name: String,
}

impl LayerGroupBaseData {
    #[must_use]
    pub fn new(layer_index: i32, name: String, type_: Type) -> Self {
        LayerGroupBaseData { unique_id: SimpleIdentity::new(), type_, enabled: true, layer_index, name }
    }
}

/// A layer-like group of drawables, not a group of layers.
pub trait LayerGroupBase: Any {
    fn base(&self) -> &LayerGroupBaseData;
    fn base_mut(&mut self) -> &mut LayerGroupBaseData;

    /// The `static_cast<TileLayerGroup&>` used by `visitLayerGroupDrawables`
    /// and the render layers.
    fn as_tile_layer_group(&self) -> Option<&TileLayerGroup> {
        None
    }
    fn as_tile_layer_group_mut(&mut self) -> Option<&mut TileLayerGroup> {
        None
    }
    fn as_layer_group_mut(&mut self) -> Option<&mut LayerGroup> {
        None
    }

    /// Get the number of drawables contained
    fn get_drawable_count(&self) -> usize;
    /// Clear the drawable collection
    fn clear_drawables(&mut self) -> usize;

    /// Uploads the layer group's drawables, before the frame draws them.
    fn upload(&mut self, _context: &dyn Context) {}
    /// Called during each render pass
    fn render(&mut self, _parameters: &mut dyn PaintParameters) {}
    /// Called at the end of each frame
    fn post_render(&mut self, _parameters: &mut dyn PaintParameters) {}

    /// The uniform buffers attached to this layer group, to write
    fn mutable_uniform_buffers(&mut self) -> &mut UniformBufferArray;

    /// Update the layer index to a new value
    fn update_layer_index(&mut self, value: i32) {
        self.base_mut().layer_index = value;
    }

    fn get_id(&self) -> SimpleIdentity {
        self.base().unique_id
    }
    fn set_enabled(&mut self, value: bool) {
        self.base_mut().enabled = value;
    }
    fn get_type(&self) -> Type {
        self.base().type_
    }
    fn get_layer_index(&self) -> i32 {
        self.base().layer_index
    }
    fn empty(&self) -> bool {
        self.get_drawable_count() == 0
    }
}

/// A shared layer group of either kind. Reach the concrete group with
/// `as_tile_layer_group_mut`.
pub type LayerGroupBasePtr = Rc<RefCell<dyn LayerGroupBase>>;

/// Initializes the tweakers of a drawable added to a group.
fn init_drawable_tweakers(drawable: &mut Drawable) {
    let tweakers = drawable.get_tweakers().clone();
    for tweaker in tweakers {
        tweaker.init(drawable);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct TileLayerGroupTileKey {
    pub(crate) render_pass: RenderPass,
    pub(crate) tile_id: OverscaledTileID,
}

/// The `DrawableLessByPriority` key: (draw priority, id).
type SortKey = (i64, SimpleIdentity);

fn sort_key(drawable: &Drawable) -> SortKey {
    (drawable.get_draw_priority(), drawable.get_id())
}

/// The buffers a layer group binds for its drawables, and their group.
#[derive(Default)]
struct LayerBuffers {
    uniform_buffers: UniformBufferArray,
    bind_group: BindGroup,
}

impl LayerBuffers {
    /// The group of the buffers, `None` where there are none.
    fn bind_group(&mut self, context: &dyn Context, name: &str) -> Result<Option<BindGroupId>, ()> {
        let entries = self.uniform_buffers.entries();
        if entries.is_empty() {
            return Ok(None);
        }
        self.bind_group.get(context, name, entries).map(Some).map_err(|error| {
            Log::warning(Event::Render, format!("Binding the buffers of layer group {name} failed: {error}"));
        })
    }
}

/// Runs a drawable's tweakers and draws it.
fn draw(drawable: &mut Drawable, parameters: &mut dyn PaintParameters, layer: Option<BindGroupId>) {
    let tweakers = drawable.get_tweakers().clone();
    for tweaker in tweakers {
        tweaker.execute(drawable, parameters);
    }
    drawable.draw(parameters, layer);
}

/// A layer group for tile-based drawables.
pub struct TileLayerGroup {
    pub base: LayerGroupBaseData,
    /// When stencil clipping is enabled for the layer, this is the set of
    /// tile IDs that need to be rendered to the stencil buffer.
    pub stencil_tiles: Option<Rc<Vec<UnwrappedTileID>>>,
    drawables: BTreeMap<SortKey, (TileLayerGroupTileKey, UniqueDrawable)>,
    buffers: LayerBuffers,
}

impl TileLayerGroup {
    #[must_use]
    pub fn new(layer_index: i32, name: String) -> Self {
        TileLayerGroup {
            base: LayerGroupBaseData::new(layer_index, name, Type::TileLayerGroup),
            stencil_tiles: None,
            drawables: BTreeMap::new(),
            buffers: LayerBuffers::default(),
        }
    }

    /// A shared group.
    #[must_use]
    pub fn new_ptr(layer_index: i32, name: impl Into<String>) -> LayerGroupBasePtr {
        Rc::new(RefCell::new(Self::new(layer_index, name.into())))
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
    pub fn visit_drawables(&mut self, mut f: impl FnMut(&mut Drawable)) -> usize {
        for (_, drawable) in self.drawables.values_mut() {
            f(drawable.as_mut());
        }
        self.drawables.len()
    }

    pub fn visit_drawables_ref(&self, mut f: impl FnMut(&Drawable)) -> usize {
        for (_, drawable) in self.drawables.values() {
            f(drawable.as_ref());
        }
        self.drawables.len()
    }

    /// Remove the drawables for which `f` returns true; returns the count.
    pub fn remove_drawables_if(&mut self, mut f: impl FnMut(&mut Drawable) -> bool) -> usize {
        let old_size = self.drawables.len();
        self.drawables.retain(|_, (_, drawable)| !f(drawable.as_mut()));
        old_size - self.drawables.len()
    }

    /// Call the provided function for each drawable for the given tile
    pub fn visit_drawables_for(
        &mut self,
        pass: RenderPass,
        tile_id: &OverscaledTileID,
        mut f: impl FnMut(&mut Drawable),
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

/// A layer group for drawables that are not tied to tiles.
pub struct LayerGroup {
    pub base: LayerGroupBaseData,
    drawables: BTreeMap<SortKey, UniqueDrawable>,
    buffers: LayerBuffers,
}

impl LayerGroup {
    #[must_use]
    pub fn new(layer_index: i32, name: String) -> Self {
        LayerGroup {
            base: LayerGroupBaseData::new(layer_index, name, Type::LayerGroup),
            drawables: BTreeMap::new(),
            buffers: LayerBuffers::default(),
        }
    }

    /// A shared group.
    #[must_use]
    pub fn new_ptr(layer_index: i32, name: impl Into<String>) -> LayerGroupBasePtr {
        Rc::new(RefCell::new(Self::new(layer_index, name.into())))
    }

    #[must_use]
    pub fn get_drawable_count(&self) -> usize {
        self.drawables.len()
    }

    pub fn add_drawable(&mut self, mut drawable: UniqueDrawable) {
        init_drawable_tweakers(drawable.as_mut());
        let key = sort_key(drawable.as_ref());
        self.drawables.insert(key, drawable);
    }

    pub fn visit_drawables(&mut self, mut f: impl FnMut(&mut Drawable)) -> usize {
        for drawable in self.drawables.values_mut() {
            f(drawable.as_mut());
        }
        self.drawables.len()
    }

    pub fn clear_drawables(&mut self) -> usize {
        let count = self.drawables.len();
        self.drawables.clear();
        count
    }
}

impl LayerGroupBase for TileLayerGroup {
    fn base(&self) -> &LayerGroupBaseData {
        &self.base
    }
    fn base_mut(&mut self) -> &mut LayerGroupBaseData {
        &mut self.base
    }
    fn as_tile_layer_group(&self) -> Option<&TileLayerGroup> {
        Some(self)
    }
    fn as_tile_layer_group_mut(&mut self) -> Option<&mut TileLayerGroup> {
        Some(self)
    }

    fn get_drawable_count(&self) -> usize {
        self.drawables.len()
    }

    fn clear_drawables(&mut self) -> usize {
        TileLayerGroup::clear_drawables(self)
    }

    fn upload(&mut self, context: &dyn Context) {
        if !self.base.enabled {
            return;
        }
        for (_, drawable) in self.drawables.values_mut() {
            if drawable.enabled {
                drawable.upload(context);
            }
        }
    }

    /// Draws the drawables of the pass. 3D drawables share the group's depth
    /// mode, and each takes a stencil value of its own, or, where the group
    /// clips to tiles, one the group's drawables share. 2D drawables clip to
    /// their tiles' masks, which this draws first.
    fn render(&mut self, parameters: &mut dyn PaintParameters) {
        if !self.base.enabled || self.drawables.is_empty() || parameters.render_pass().is_none() {
            return;
        }
        let pass = parameters.pass();
        let stencil_tiles = self.stencil_tiles.clone().filter(|tiles| !tiles.is_empty());
        let has_stencil_tiles = stencil_tiles.is_some();

        // `stencil_mode_for_3d` hands out a new value each call, so the
        // values are taken up front: one slot for each 3D stencilled
        // drawable that takes a value of its own, by its place in the
        // drawing order.
        let mut features_3d = false;
        let mut stencil_3d = false;
        let mut own_stencil_slots: Vec<Option<usize>> = Vec::with_capacity(self.drawables.len());
        let mut own_stencil_modes: Vec<StencilMode> = Vec::new();
        for (_, drawable) in self.drawables.values() {
            let mut slot = None;
            if drawable.enabled && drawable.is_3d && drawable.has_render_pass(pass) {
                features_3d = true;
                if drawable.enable_stencil {
                    stencil_3d = true;
                    if !has_stencil_tiles {
                        slot = Some(own_stencil_modes.len());
                        own_stencil_modes.push(StencilMode::default());
                    }
                }
            }
            own_stencil_slots.push(slot);
        }
        let own_stencil_modes_taken = parameters.stencil_modes_for_3d(&mut own_stencil_modes);

        let mut depth_mode_3d = DepthMode::disabled();
        let mut stencil_mode_3d = None;
        if features_3d {
            depth_mode_3d = parameters.depth_mode_for_3d(DepthMaskType::ReadWrite);
            if stencil_3d && has_stencil_tiles {
                stencil_mode_3d = Some(parameters.stencil_mode_for_3d());
            }
        } else if let Some(tiles) = &stencil_tiles {
            parameters.render_tile_clipping_masks(tiles);
        }

        let Ok(layer) = self.buffers.bind_group(parameters.context(), &self.base.name) else {
            return;
        };
        for ((_, drawable), own_slot) in self.drawables.values_mut().zip(own_stencil_slots) {
            if !drawable.enabled || !drawable.has_render_pass(pass) {
                continue;
            }
            if features_3d {
                let depth = if drawable.enable_depth { depth_mode_3d } else { DepthMode::disabled() };
                if let Some(slot) = own_slot
                    && !own_stencil_modes_taken
                {
                    // There were more drawables than stencil values: each
                    // takes its value as it draws, so a stencil clear falls
                    // between two draws.
                    own_stencil_modes[slot] = parameters.stencil_mode_for_3d();
                }
                let stencil = match (drawable.enable_stencil, own_slot, stencil_mode_3d) {
                    (true, Some(slot), _) => own_stencil_modes[slot],
                    (true, None, Some(mode)) => mode,
                    _ => StencilMode::disabled(),
                };
                drawable.set_3d_modes(depth, stencil);
            }
            draw(drawable, parameters, layer);
        }
    }

    fn mutable_uniform_buffers(&mut self) -> &mut UniformBufferArray {
        &mut self.buffers.uniform_buffers
    }
}

impl LayerGroupBase for LayerGroup {
    fn base(&self) -> &LayerGroupBaseData {
        &self.base
    }
    fn base_mut(&mut self) -> &mut LayerGroupBaseData {
        &mut self.base
    }
    fn as_layer_group_mut(&mut self) -> Option<&mut LayerGroup> {
        Some(self)
    }

    fn get_drawable_count(&self) -> usize {
        self.drawables.len()
    }

    fn clear_drawables(&mut self) -> usize {
        LayerGroup::clear_drawables(self)
    }

    fn upload(&mut self, context: &dyn Context) {
        if !self.base.enabled {
            return;
        }
        for drawable in self.drawables.values_mut() {
            if drawable.enabled {
                drawable.upload(context);
            }
        }
    }

    /// Draws the drawables of the pass. A 3D drawable draws with the 3D
    /// depth mode and no stencil test.
    fn render(&mut self, parameters: &mut dyn PaintParameters) {
        if !self.base.enabled || self.drawables.is_empty() || parameters.render_pass().is_none() {
            return;
        }
        let pass = parameters.pass();
        let Ok(layer) = self.buffers.bind_group(parameters.context(), &self.base.name) else {
            return;
        };
        for drawable in self.drawables.values_mut() {
            if !drawable.enabled || !drawable.has_render_pass(pass) {
                continue;
            }
            if drawable.is_3d {
                let depth = if drawable.enable_depth {
                    parameters.depth_mode_for_3d(drawable.depth_type)
                } else {
                    DepthMode::disabled()
                };
                drawable.set_3d_modes(depth, StencilMode::disabled());
            }
            draw(drawable, parameters, layer);
        }
    }

    fn mutable_uniform_buffers(&mut self) -> &mut UniformBufferArray {
        &mut self.buffers.uniform_buffers
    }
}

pub fn visit_layer_group_drawables(layer_group: &mut dyn LayerGroupBase, f: impl FnMut(&mut Drawable)) {
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
