use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use mln_gfx::bind_group::BindGroup;
use mln_gfx::context::Context;
use mln_gfx::depth_mode::DepthMode;
use mln_gfx::gpu::BindGroupId;
use mln_gfx::stencil_mode::StencilMode;
use mln_gfx::types::DepthMaskType;
use mln_gfx::uniform_buffer::UniformBufferArray;
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::identity::SimpleIdentity;
use mln_util::logging::{Event, Log};

use crate::drawable::{Drawable, UniqueDrawable};
use crate::layers::render_custom_layer::CustomLayerDraw;
use crate::paint_parameters::PaintParameters;
use crate::render_pass::RenderPass;

/// How a layer group draws.
pub(crate) enum LayerGroupKind {
    /// The drawables of tiles: 2D ones clip to their tiles' masks, 3D ones
    /// share the group's depth and stencil.
    Tile,
    /// Drawables not tied to tiles.
    Plain,
    /// A custom layer, whose host draws with objects of its own.
    Custom(CustomLayerDraw),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct TileKey {
    render_pass: RenderPass,
    tile_id: OverscaledTileID,
}

/// The order drawables draw in: by draw priority, then by id.
type SortKey = (i64, SimpleIdentity);

fn sort_key(drawable: &Drawable) -> SortKey {
    (drawable.draw_priority, drawable.unique_id)
}

/// A layer-like group of drawables, not a group of layers.
pub struct LayerGroup {
    pub(crate) id: SimpleIdentity,
    pub(crate) enabled: bool,
    pub(crate) layer_index: i32,
    pub(crate) name: String,
    kind: LayerGroupKind,
    /// Of a tile group whose layer clips to tiles, the tiles whose masks the
    /// stencil buffer holds when its 2D drawables draw.
    pub(crate) stencil_tiles: Option<Rc<Vec<UnwrappedTileID>>>,
    /// The drawables, with the pass and tile of a tile group's.
    drawables: BTreeMap<SortKey, (Option<TileKey>, UniqueDrawable)>,
    /// The buffers the group binds for its drawables, and their group.
    uniform_buffers: UniformBufferArray,
    bind_group: BindGroup,
}

pub(crate) type LayerGroupPtr = Rc<RefCell<LayerGroup>>;

impl LayerGroup {
    #[must_use]
    pub(crate) fn new(layer_index: i32, name: impl Into<String>, kind: LayerGroupKind) -> LayerGroupPtr {
        Rc::new(RefCell::new(LayerGroup {
            id: SimpleIdentity::new(),
            enabled: true,
            layer_index,
            name: name.into(),
            kind,
            stencil_tiles: None,
            drawables: BTreeMap::new(),
            uniform_buffers: UniformBufferArray::new(),
            bind_group: BindGroup::default(),
        }))
    }

    /// A shared group of tiles' drawables.
    #[must_use]
    pub(crate) fn tiles(layer_index: i32, name: impl Into<String>) -> LayerGroupPtr {
        Self::new(layer_index, name, LayerGroupKind::Tile)
    }

    /// A shared group of drawables not tied to tiles.
    #[must_use]
    pub(crate) fn plain(layer_index: i32, name: impl Into<String>) -> LayerGroupPtr {
        Self::new(layer_index, name, LayerGroupKind::Plain)
    }

    /// The number of drawables, counting a custom layer's host as one.
    #[must_use]
    pub(crate) fn drawable_count(&self) -> usize {
        self.drawables.len() + usize::from(matches!(self.kind, LayerGroupKind::Custom(_)))
    }

    #[must_use]
    pub(crate) fn empty(&self) -> bool {
        self.drawable_count() == 0
    }

    /// The number of drawables of `tile_id` in `pass`.
    #[must_use]
    pub(crate) fn drawable_count_for(&self, pass: RenderPass, tile_id: &OverscaledTileID) -> usize {
        let key = Some(TileKey { render_pass: pass, tile_id: *tile_id });
        self.drawables.values().filter(|(k, _)| *k == key).count()
    }

    /// Adds a drawable of `tile_id` in `pass`.
    pub(crate) fn add_tile_drawable(&mut self, pass: RenderPass, tile_id: &OverscaledTileID, drawable: UniqueDrawable) {
        self.insert(Some(TileKey { render_pass: pass, tile_id: *tile_id }), drawable);
    }

    /// Adds a drawable not tied to a tile.
    pub(crate) fn add_drawable(&mut self, drawable: UniqueDrawable) {
        self.insert(None, drawable);
    }

    fn insert(&mut self, key: Option<TileKey>, mut drawable: UniqueDrawable) {
        if let Some(atlases) = drawable.atlases.clone() {
            atlases.init(&mut drawable);
        }
        let previous = self.drawables.insert(sort_key(&drawable), (key, drawable));
        debug_assert!(previous.is_none());
    }

    /// Takes out the drawables of `tile_id` in `pass`.
    pub(crate) fn remove_drawables(&mut self, pass: RenderPass, tile_id: &OverscaledTileID) -> Vec<UniqueDrawable> {
        let key = Some(TileKey { render_pass: pass, tile_id: *tile_id });
        let (removed, kept) = std::mem::take(&mut self.drawables).into_iter().partition(|(_, (k, _))| *k == key);
        self.drawables = kept;
        removed.into_values().map(|(_, drawable)| drawable).collect()
    }

    /// Removes the drawables for which `f` returns true, and returns how
    /// many.
    pub(crate) fn remove_drawables_if(&mut self, mut f: impl FnMut(&mut Drawable) -> bool) -> usize {
        let count = self.drawables.len();
        self.drawables.retain(|_, (_, drawable)| !f(drawable));
        count - self.drawables.len()
    }

    /// Calls `f` for each drawable, in drawing order.
    pub(crate) fn visit_drawables(&mut self, mut f: impl FnMut(&mut Drawable)) {
        for (_, drawable) in self.drawables.values_mut() {
            f(drawable);
        }
    }

    /// Calls `f` for each drawable of `tile_id` in `pass`, and returns how
    /// many.
    pub(crate) fn visit_drawables_for(
        &mut self,
        pass: RenderPass,
        tile_id: &OverscaledTileID,
        mut f: impl FnMut(&mut Drawable),
    ) -> usize {
        let key = Some(TileKey { render_pass: pass, tile_id: *tile_id });
        let mut count = 0;
        for (k, drawable) in self.drawables.values_mut() {
            if *k == key {
                f(drawable);
                count += 1;
            }
        }
        count
    }

    /// Removes the drawables, and returns how many there were.
    pub(crate) fn clear_drawables(&mut self) -> usize {
        let count = self.drawables.len();
        self.drawables.clear();
        count
    }

    /// The buffers the group binds for its drawables, by id.
    pub(crate) fn mutable_uniform_buffers(&mut self) -> &mut UniformBufferArray {
        &mut self.uniform_buffers
    }

    /// Uploads the enabled drawables, before the frame draws them.
    pub(crate) fn upload(&mut self, context: &dyn Context) {
        if !self.enabled {
            return;
        }
        for (_, drawable) in self.drawables.values_mut() {
            if drawable.enabled {
                drawable.upload(context);
            }
        }
    }

    /// Draws the drawables of the pass.
    pub(crate) fn render(&mut self, parameters: &mut PaintParameters<'_>) {
        if !self.enabled || parameters.pass_target().is_none() {
            return;
        }
        if let LayerGroupKind::Custom(custom) = &self.kind {
            custom.render(parameters);
        } else if !self.drawables.is_empty() {
            if matches!(self.kind, LayerGroupKind::Tile) {
                self.render_tiles(parameters);
            } else {
                self.render_plain(parameters);
            }
        }
    }

    /// The group of the buffers, `None` where there are none.
    fn bind_group(&mut self, context: &dyn Context) -> Result<Option<BindGroupId>, ()> {
        let entries = self.uniform_buffers.entries();
        if entries.is_empty() {
            return Ok(None);
        }
        self.bind_group.get(context, &self.name, entries).map(Some).map_err(|error| {
            Log::warning(Event::Render, format!("Binding the buffers of layer group {} failed: {error}", self.name));
        })
    }

    /// Draws a tile group's drawables of the pass. 3D drawables share the
    /// group's depth mode, and each takes a stencil value of its own, or,
    /// where the group clips to tiles, one the group's drawables share. 2D
    /// drawables clip to their tiles' masks, which this draws first.
    fn render_tiles(&mut self, parameters: &mut PaintParameters<'_>) {
        let pass = parameters.pass;
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

        let Ok(layer) = self.bind_group(parameters.context) else {
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

    /// Draws a plain group's drawables of the pass. A 3D drawable draws with
    /// the 3D depth mode and no stencil test.
    fn render_plain(&mut self, parameters: &PaintParameters<'_>) {
        let pass = parameters.pass;
        let Ok(layer) = self.bind_group(parameters.context) else {
            return;
        };
        for (_, drawable) in self.drawables.values_mut() {
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
}

/// Gives the drawable the symbol atlases' textures where it shows them, and
/// draws it.
fn draw(drawable: &mut Drawable, parameters: &PaintParameters<'_>, layer: Option<BindGroupId>) {
    if let Some(atlases) = drawable.atlases.clone() {
        atlases.execute(drawable, parameters);
    }
    drawable.draw(parameters, layer);
}
