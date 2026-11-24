//! What the renderer keeps over its backend's `Gpu`: its caps, the caches
//! its draws share, among them the keys of the programs' variants, the
//! stores of the textures, indexes and vertices they share and of the draws'
//! GPU objects (meshes), the buffers
//! every draw binds, and the device that makes its
//! buffers and textures, with the rendering stats and the queue of what it
//! dropped. The renderer owns it, and lends it
//! to a frame's encode, which changes it through `&mut`. The context holds
//! no `Gpu`: each call that makes, writes or destroys a resource takes the
//! one the renderer was made for.

use std::collections::BTreeMap;
use std::rc::Rc;

use mln_gfx::ColorMode;
use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::gpu::{
    BufferId, BufferUsage, Caps, ColorAttachment, DepthStencilAttachment, Gpu, GpuError, LoadOp, PipelineId,
    PipelineKey, ResourceId, SamplerId, ShaderId, ShaderKey, TargetFormats, TextureFormat, TextureId,
};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::rendering_stats::RenderingStats;
use mln_gfx::shaders::block::DrawBlocks;
use mln_gfx::shaders::program::{Program, ProgramParameters};
use mln_gfx::texture2d::{SamplerState, texture_format};
use mln_gfx::types::{TextureChannelDataType, TexturePixelType};
use mln_tile_id::CanonicalTileID;
use mln_util::Color;
use mln_util::constants::EXTENT;
use mln_util::hash::HashMap;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::after_gpu::{AfterGpuQueue, GpuEnd};
use crate::draw_objects::{DrawObjects, Mesh};
use crate::gfx::buffer::{self, Buffer};
use crate::gfx::buffer_store::{BufferStore, SharedBuffer, StoredBuffer};
use crate::gfx::mesh::Indexed;
use crate::gfx::owned::{Held, Owned};
use crate::gfx::release::{Release, destroy_order};
use crate::gfx::resources::{Device, SharedGpuTexture};
use crate::gfx::store::{MeshKey, SharedTexture, Store, StoreKey, TextureKey};
use crate::gfx::texture2d::{Texture2D, TextureMut};
use crate::gfx::uniform_buffer::UniformBufferArray;
use crate::gfx::vertex_attribute::Uniforms;
use crate::handles::{ImageVertex, TileSurface};
use crate::paint_parameters::Frame;
use crate::paint_property_binder::MeshPaint;
use crate::render_static_data::RenderStaticData;
use crate::tile::raster::MaskedGeometry;
use crate::tile_mask::TileMask;

/// What a tile's grid on the globe depends on: its zoom, whether it touches
/// the north and the south pole, and the parts of it that its mask keeps.
type GridKey = (u8, bool, bool, TileMask);

/// The renderer's GPU resources: the buffers, textures and meshes that
/// layers make and keep handles of, with what the device allows
/// ([`caps`](Self::caps)). The renderer owns them and lends them to a
/// layer's hooks. Writes to a resource reach the GPU in the frame's encode,
/// before its passes.
pub struct Resources {
    device: Device,
    caps: Caps,
    global_uniform_buffers: UniformBufferArray,
    /// The sampler of each state a texture is sampled with.
    samplers: HashMap<SamplerState, SamplerId>,
    /// What the programs are compiled with, and whether the draws show the
    /// overdraw inspector.
    program_parameters: ProgramParameters,
    /// The key of each variant a draw used.
    shader_keys: HashMap<(&'static Program, Uniforms, ProjectionVariant), ShaderKey>,
    /// The shader of each key a draw used.
    shaders: HashMap<ShaderKey, ShaderId>,
    /// The pipeline of each key a draw used.
    pipelines: HashMap<PipelineKey, PipelineId>,
    /// The buffers of default attribute values, one per format, with the
    /// vertex count each covers.
    default_vertex_buffers: HashMap<AttributeDataType, (usize, Rc<Buffer>)>,
    /// The renderer's textures, which their owners and the draws that bind
    /// them hold as [`SharedTexture`]s.
    textures: Store<TextureKey, Texture2D>,
    /// The vertex and index buffers that draws share, which they hold as
    /// [`SharedBuffer`].
    buffers: BufferStore,
    /// The draws' GPU objects, which their makers hold as [`Mesh`]es. Boxed,
    /// so that a draw takes its objects out of the store cheaply while it
    /// uploads and draws them with the context.
    meshes: Store<MeshKey, Box<DrawObjects>>,
    /// The `seq` of the next mesh.
    next_mesh_seq: u64,
    /// The tiles' surfaces: the extent's quad, and the globe's grid of each
    /// zoom, by whether it touches the north and the south pole. These, the
    /// image quad, the masked grids and the quads stay while a mesh holds
    /// them ([`drop_unused_surfaces`](Self::drop_unused_surfaces)).
    surfaces: HashMap<Option<(u8, bool, bool)>, TileSurface>,
    /// The surface that a whole tile's image drapes over on Mercator.
    image_quad: Option<TileSurface<ImageVertex>>,
    /// The globe's grids of the parts of tiles that their masks keep, each
    /// with the last of `grid_uses` it served.
    masked_grids: BTreeMap<GridKey, (TileSurface<ImageVertex>, u64)>,
    grid_uses: u64,
    /// The indexes of quads, each four vertices after the last
    /// ([`quads`](Self::quads)), once a layer asked for them.
    quads: Option<crate::draw::Buffer<u16>>,
    /// What a [`destroy_released`](Self::destroy_released) takes from the
    /// queue, and what it destroys, kept for their storage.
    releases: Vec<Release>,
    doomed: Vec<ResourceId>,
    /// Whether [`tear_down`](Self::tear_down) destroyed what the context
    /// made.
    torn_down: bool,
    /// The textures of the targets that the frame being encoded drew into
    /// without a failure, until its submission
    /// ([`submitted`](Self::submitted)).
    drawn_targets: Vec<crate::draw::Texture>,
    /// The work that waits for the GPU to finish what the context gave it.
    pub(crate) after_gpu: AfterGpuQueue,
}

impl Resources {
    /// The renderer's state over a `Gpu` of `caps`, for draws at
    /// `pixel_ratio` device pixels per logical pixel.
    #[must_use]
    pub(crate) fn new(caps: Caps, pixel_ratio: f32) -> Resources {
        Resources {
            device: Device::default(),
            caps,
            global_uniform_buffers: UniformBufferArray::default(),
            samplers: HashMap::default(),
            program_parameters: ProgramParameters { pixel_ratio, overdraw: false },
            shader_keys: HashMap::default(),
            shaders: HashMap::default(),
            pipelines: HashMap::default(),
            default_vertex_buffers: HashMap::default(),
            textures: Store::default(),
            buffers: BufferStore::default(),
            meshes: Store::default(),
            next_mesh_seq: 0,
            surfaces: HashMap::default(),
            image_quad: None,
            masked_grids: BTreeMap::new(),
            grid_uses: 0,
            quads: None,
            releases: Vec::new(),
            doomed: Vec::new(),
            torn_down: false,
            drawn_targets: Vec::new(),
            after_gpu: AfterGpuQueue::new(),
        }
    }

    /// Resources that no GPU has seen, which a render layer made only to
    /// evaluate its values joins.
    #[cfg(any(test, feature = "bench"))]
    pub(crate) fn detached() -> Resources {
        let caps = Caps {
            buffer_offset_alignment: 256,
            max_texture_size: 4096,
            half_float_render_targets: false,
            clip_depth: mln_util::clip_depth::ClipDepthRange::ZeroToOne,
            render_target_depth: None,
            clear_stencil_with_pass: false,
            cpu_rasterizer: false,
            max_line_width: 1.0,
            binds_vertex_buffers: false,
            indexes_storage_buffers: true,
        };
        Resources::new(caps, 1.0)
    }

    /// What the renderer makes its buffers and textures with.
    pub(crate) fn device(&mut self) -> &mut Device {
        &mut self.device
    }

    /// What the backend's device allows, and how the renderer draws on it.
    #[must_use]
    pub fn caps(&self) -> &Caps {
        &self.caps
    }

    /// A buffer of `data`.
    pub fn buffer<V: bytemuck::Pod>(&mut self, data: &[V]) -> crate::draw::Buffer<V> {
        crate::draw::Buffer::new(self.add_buffer(data.to_vec()))
    }

    /// An index buffer of `indexes`, one segment of them over the vertices
    /// they index.
    pub fn index_buffer(&mut self, indexes: &[u16]) -> crate::draw::IndexBuffer {
        let vertices = indexes.iter().max().map_or(0, |&last| usize::from(last) + 1);
        let segments = vec![mln_gfx::shaders::segment::SegmentBase::with_lengths(0, 0, vertices, indexes.len())];
        crate::draw::IndexBuffer { buffer: self.buffer(indexes), segments }
    }

    /// A texture, which `setup` gives its format and first image.
    pub fn texture(&mut self, setup: impl FnOnce(&mut TextureMut<'_>)) -> crate::draw::Texture {
        crate::draw::Texture(self.add_texture_with(setup))
    }

    /// `texture`, to change: the meshes that sample it see the change.
    /// `None` for a target's texture
    /// ([`Target::texture`](crate::draw::Target::texture)), which only its
    /// draws change.
    pub fn texture_mut(&mut self, texture: &crate::draw::Texture) -> Option<TextureMut<'_>> {
        let texture = self.textures.get_mut(&texture.0).filter(|texture| !texture.is_render_target())?;
        Some(TextureMut::new(texture))
    }

    /// A target of `size` texels with RGBA channels of `channel`, or of
    /// unsigned bytes for half floats that the device can't draw into
    /// ([`Caps::half_float_render_targets`]), cleared to `clear` before the
    /// draws into it.
    pub fn target(&mut self, size: Size, channel: TextureChannelDataType, clear: Color) -> crate::draw::Target {
        let format = texture_format(TexturePixelType::RGBA, self.render_target_type(channel));
        let texture = self.add_texture_with(|texture| {
            texture.set_size(size);
            texture.set_format(format);
            texture.set_render_target();
        });
        crate::draw::Target { texture: crate::draw::Texture(texture), clear }
    }

    /// The pass that draws into `target`, which clears it to its color and
    /// its depth to 1, making their textures through `gpu`: the depth and
    /// stencil `depth_stencil` of its format, whose stencil it clears to 0
    /// too, or else the target's own depth, if the device draws targets with
    /// one.
    pub(crate) fn target_pass(
        &mut self,
        gpu: &mut dyn Gpu,
        target: &crate::draw::Target,
        depth_stencil: Option<(TextureId, TextureFormat)>,
    ) -> Result<crate::paint_parameters::OpenPass, GpuError> {
        let depth_format = self.caps.render_target_depth;
        let (texture, device) = self.texture_entry(&target.texture.0)?;
        let color = texture.create(gpu, device)?;
        let (depth, stencil) = if let Some(depth_stencil) = depth_stencil {
            (Some(depth_stencil), LoadOp::Clear(0))
        } else {
            let depth = depth_format.map(|format| texture.depth(gpu, device, format)).transpose()?;
            (depth.zip(depth_format), LoadOp::Load)
        };
        Ok(crate::paint_parameters::OpenPass {
            label: "render target",
            color: Some(ColorAttachment { texture: color, load: LoadOp::Clear(target.clear) }),
            depth_stencil: depth.map(|(depth, _)| DepthStencilAttachment {
                texture: depth,
                depth: LoadOp::Clear(1.0),
                stencil,
            }),
            size: texture.size(),
            targets: TargetFormats { color: Some(texture.format()), depth_stencil: depth.map(|(_, format)| format) },
        })
    }

    /// Whether `target` holds what a frame drew into it: its pass, its
    /// draws and the frame's submission ended without a failure, and it
    /// kept its texture since. What it holds stays until the frame draws
    /// into it again ([`Draws::target`](crate::draw::Draws::target)), so a
    /// layer whose draws into a target don't change from frame to frame
    /// draws into it until it holds them.
    #[must_use]
    pub fn is_drawn(&self, target: &crate::draw::Target) -> bool {
        self.textures.get(&target.texture.0).is_some_and(Texture2D::is_drawn)
    }

    /// Says whether the frame's pass and draws into `target` ended without
    /// a failure; one that did holds them once the frame's submission
    /// doesn't fail either ([`submitted`](Self::submitted)).
    pub(crate) fn set_drawn(&mut self, target: &crate::draw::Target, drawn: bool) {
        if let Some(texture) = self.textures.get_mut(&target.texture.0) {
            texture.set_drawn(drawn);
            if drawn {
                self.drawn_targets.push(target.texture.clone());
            }
        }
    }

    /// Ends the submission of the frame being encoded, which failed unless
    /// `ok`: then the targets that the frame drew into hold nothing, and
    /// the layers draw into them again.
    pub(crate) fn submitted(&mut self, ok: bool) {
        let Resources { textures, drawn_targets, .. } = self;
        for texture in drawn_targets.drain(..) {
            if !ok && let Some(texture) = textures.get_mut(&texture.0) {
                texture.set_drawn(false);
            }
        }
    }

    /// Makes `target` `size` texels from its next draws on, unless it
    /// already is that size. What it held goes, and its meshes sample the
    /// new texture.
    pub fn resize(&mut self, target: &crate::draw::Target, size: Size) {
        if let Some(texture) = self.textures.get_mut(&target.texture.0) {
            TextureMut::new(texture).set_size(size);
        }
    }

    /// The surface of `tile` in `frame`'s projection, which every tile
    /// alike shares.
    pub fn tile_surface(&mut self, tile: crate::layer::FrameTile, frame: &Frame<'_>) -> TileSurface {
        let canonical = tile.id().canonical;
        self.surface(frame.camera.is_globe().then_some(&canonical))
    }

    /// The surface of a tile's extent on Mercator, which every tile shares
    /// ([`tile_surface`](Self::tile_surface)).
    pub(crate) fn square_surface(&mut self) -> TileSurface {
        self.surface(None)
    }

    /// The surface of `globe_tile` on the globe, or of a tile on Mercator.
    fn surface(&mut self, globe_tile: Option<&CanonicalTileID>) -> TileSurface {
        let key = globe_tile.map(|tile| (tile.z, tile.y == 0, u64::from(tile.y) == (1u64 << tile.z) - 1));
        if let Some(surface) = self.surfaces.get(&key) {
            return surface.clone();
        }
        let surface = if let Some(tile) = globe_tile {
            let grid = crate::globe::tile_grid(tile);
            TileSurface {
                vertices: self.buffer(&grid.vertices),
                triangles: self.index_buffer(grid.triangles.as_flattened()),
            }
        } else {
            TileSurface {
                vertices: self.buffer(&RenderStaticData::tile_vertices()),
                triangles: self.index_buffer(&RenderStaticData::quad_triangle_indices()),
            }
        };
        self.surfaces.insert(key, surface.clone());
        surface
    }

    /// The surface that the image of the tile `tile`, which `geometry`
    /// masks, drapes over in a frame on the globe or not: the parts of the
    /// tile that the mask keeps.
    pub(crate) fn image_surface(
        &mut self,
        tile: &CanonicalTileID,
        geometry: &MaskedGeometry,
        globe: bool,
    ) -> TileSurface<ImageVertex> {
        if globe {
            return self.masked_grid(tile, geometry.mask());
        }
        if let Some(quads) = geometry.quads() {
            return TileSurface::of_quads(quads);
        }
        if let Some(quad) = &self.image_quad {
            return quad.clone();
        }
        let quad = TileSurface {
            vertices: self.buffer(&RenderStaticData::raster_vertices()),
            triangles: self.index_buffer(&RenderStaticData::quad_triangle_indices()),
        };
        self.image_quad.insert(quad).clone()
    }

    /// The globe's grid of the parts of the tile `tile` that `mask` keeps,
    /// whose texture positions stay inside the tile, so that the rows out
    /// to a pole sample the tile's edge. Tiles that load go through many
    /// masks: the grid that served longest ago makes room past 64, so that
    /// those every frame draws stay.
    fn masked_grid(&mut self, tile: &CanonicalTileID, mask: &TileMask) -> TileSurface<ImageVertex> {
        const MAX_GRIDS: usize = 64;
        let key = (tile.z, tile.y == 0, u64::from(tile.y) == (1u64 << tile.z) - 1, mask.clone());
        self.grid_uses += 1;
        if let Some((grid, used)) = self.masked_grids.get_mut(&key) {
            *used = self.grid_uses;
            return grid.clone();
        }
        if self.masked_grids.len() >= MAX_GRIDS
            && let Some(oldest) =
                self.masked_grids.iter().min_by_key(|(_, (_, used))| *used).map(|(key, _)| key.clone())
        {
            self.masked_grids.remove(&oldest);
        }
        let (mut vertices, mut indexes, mut segments) = (Vec::new(), Vec::new(), Vec::new());
        for grid in crate::globe::masked_tile_grids(tile, mask) {
            let first = vertices.len();
            let clamp = |value: i16| i32::from(value).clamp(0, EXTENT) as u16;
            vertices.extend(
                grid.vertices.iter().map(|&[x, y]| ImageVertex { position: [x, y], texture: [clamp(x), clamp(y)] }),
            );
            let mut triangles = Indexed { indexes: &mut indexes, segments: &mut segments };
            triangles.add(&mut vertices, first, grid.triangles.iter().map(|t| t.map(u32::from)));
        }
        let buffer = self.buffer(&indexes);
        let grid =
            TileSurface { vertices: self.buffer(&vertices), triangles: crate::draw::IndexBuffer { buffer, segments } };
        self.masked_grids.insert(key, (grid.clone(), self.grid_uses));
        grid
    }

    /// How many grids of masked tiles the resources keep.
    #[cfg(test)]
    pub(crate) fn masked_grids(&self) -> usize {
        self.masked_grids.len()
    }

    /// Drops the shared surfaces and quads that no mesh holds any more.
    fn drop_unused_surfaces(&mut self) {
        fn unused<V>(surface: &TileSurface<V>) -> bool {
            surface.vertices.is_only() && surface.triangles.buffer.is_only()
        }
        self.surfaces.retain(|_, surface| !unused(surface));
        self.image_quad.take_if(|surface| unused(surface));
        self.masked_grids.retain(|_, (surface, _)| !unused(surface));
        self.quads.take_if(|quads| quads.is_only());
    }

    /// The indexes of `count` quads, each the four vertices after the
    /// last's, of the corners 0 and 1 above 2 and 3: the counter-clockwise
    /// triangles 0, 2, 1 and 1, 2, 3 of each, in as many segments as their
    /// vertices need. Every such index buffer shares one buffer.
    pub fn quads(&mut self, count: usize) -> crate::draw::IndexBuffer {
        let buffer = if let Some(buffer) = &self.quads {
            buffer.clone()
        } else {
            let buffer = self.buffer(&RenderStaticData::quad_indices());
            self.quads.insert(buffer).clone()
        };
        crate::draw::IndexBuffer { buffer, segments: RenderStaticData::quad_segments(count) }
    }

    /// The size of `texture`, in texels.
    #[must_use]
    pub fn size(&self, texture: &crate::draw::Texture) -> Size {
        self.textures.get(&texture.0).map_or_else(|| Size::new(0, 0), Texture2D::size)
    }

    /// The channel type of a render target's color texture, which draws
    /// draw into and then sample, for one asked for of `type_`: unsigned
    /// bytes for half floats where the `Gpu` can't draw into them.
    pub(crate) fn render_target_type(&self, type_: TextureChannelDataType) -> TextureChannelDataType {
        if type_ == TextureChannelDataType::HalfFloat && !self.caps.half_float_render_targets {
            TextureChannelDataType::UnsignedByte
        } else {
            type_
        }
    }

    /// Bindings or geometry `id` of the context's `Gpu`, made of `held`,
    /// which the context destroys once the returned value drops, before
    /// what they were made of.
    pub(crate) fn own<T: Copy + Into<ResourceId>>(&self, id: T, held: Vec<Held>) -> Owned<T> {
        Owned::new(id, held, &self.device.queue)
    }

    /// The rendering stats, to count what the renderer makes and does.
    pub(crate) fn rendering_stats(&mut self) -> &mut RenderingStats {
        &mut self.device.stats
    }

    /// Logs `message`, why something of the frame being encoded failed on
    /// the GPU, and keeps it as the frame's failure unless one came first
    /// ([`Device::fail`]).
    pub(crate) fn fail(&mut self, message: String) {
        self.device.fail(message);
    }

    /// Whether something failed since the frame's failure was last taken
    /// ([`Device::take_failure`]).
    pub(crate) fn failed(&self) -> bool {
        self.device.failed()
    }

    /// How many failures there were so far, to tell whether something
    /// failed between two calls.
    pub(crate) fn failures(&self) -> u64 {
        self.device.failures()
    }

    /// The rendering stats now. What the renderer released counts until the
    /// next [`destroy_released`](Self::destroy_released) destroys it.
    pub(crate) fn stats(&self) -> RenderingStats {
        self.device.stats.clone()
    }

    /// Destroys the resources the renderer dropped, first taking the store
    /// entries whose last handles dropped out of their stores, and the
    /// buffers and textures out of the rendering stats. Bindings and
    /// geometry go before the buffers and textures they were made of. What
    /// dropped keeps its GPU memory until then. Not while a pass is open on
    /// `gpu`.
    pub(crate) fn destroy_released(&mut self, gpu: &mut dyn Gpu) {
        self.drop_unused_surfaces();
        let Resources { device, textures, buffers, meshes, releases, doomed, .. } = self;
        loop {
            // The bindings made of buffers that arrays replaced go in this
            // batch too, in one pass over the meshes per round.
            if !device.retired_epochs.is_empty() {
                for objects in meshes.values_mut() {
                    objects.drop_retired_bindings(&device.retired_epochs);
                }
                device.retired_epochs.clear();
            }
            device.queue.take_into(releases);
            if releases.is_empty() {
                break;
            }
            for release in releases.drain(..) {
                match release {
                    Release::Resource(resource) => doomed.push(resource),
                    Release::Epoch(epoch) => device.retired_epochs.push(epoch),
                    Release::Entry(StoreKey::Texture(key)) => drop(textures.remove(key)),
                    Release::Entry(StoreKey::Buffer(key)) => drop(buffers.remove(key)),
                    // What the objects hold goes to the queue as they drop,
                    // and goes in this batch.
                    Release::Entry(StoreKey::Mesh(key)) => drop(meshes.remove(key)),
                    Release::Buffer(id, usage, allocated) => {
                        buffer::count(&mut device.stats, usage, allocated, -1);
                        doomed.push(id.into());
                    }
                    Release::Texture(id, bytes) => {
                        device.stats.num_active_textures -= 1;
                        device.stats.mem_textures -= bytes as i32;
                        doomed.push(id.into());
                    }
                }
            }
        }
        if doomed.is_empty() {
            return;
        }
        doomed.sort_unstable_by_key(destroy_order);
        for resource in doomed.drain(..) {
            if let Err(error) = gpu.destroy(resource) {
                Log::error(Event::Render, format!("Destroying {resource:?} failed: {error}"));
                debug_assert!(false, "destroying {resource:?} failed: {error}");
            }
        }
    }

    /// Keeps `objects`, the GPU objects of a draw, while the returned mesh
    /// lasts, made after every mesh the context made before it
    /// ([`Mesh::seq`]), whose draws' blocks the engine writes `paint` into.
    pub(crate) fn add_mesh<B: DrawBlocks>(&mut self, objects: DrawObjects, paint: Option<MeshPaint>) -> Mesh<B> {
        let seq = self.next_mesh_seq;
        self.next_mesh_seq += 1;
        let program = objects.program();
        Mesh::new(self.meshes.add(Box::new(objects), &self.device.queue), seq, program, paint)
    }

    /// Takes the GPU objects of the mesh `key` out of the store, for a draw
    /// to upload or draw them with the context, until
    /// [`restore_mesh`](Self::restore_mesh) puts them back. `None` for a
    /// mesh that went.
    pub(crate) fn take_mesh(&mut self, key: MeshKey) -> Option<Box<DrawObjects>> {
        self.meshes.take(key)
    }

    /// Puts back the GPU objects that [`take_mesh`](Self::take_mesh) took.
    pub(crate) fn restore_mesh(&mut self, key: MeshKey, objects: Box<DrawObjects>) {
        self.meshes.restore(key, objects);
    }

    /// Keeps `vertices` for the draws that share them, while a clone of the
    /// returned handle lasts.
    pub(crate) fn add_buffer<V: bytemuck::Pod>(&mut self, vertices: Vec<V>) -> SharedBuffer {
        self.buffers.add(vertices, &self.device.queue)
    }

    /// Keeps the vertices of the type `V` in the JS array `bytes`, as
    /// [`add_buffer`](Self::add_buffer) does.
    #[cfg(target_family = "wasm")]
    pub(crate) fn add_host_buffer<V: bytemuck::Pod>(&mut self, bytes: mln_util::host_bytes::HostBytes) -> SharedBuffer {
        self.buffers.add_host::<V>(bytes, &self.device.queue)
    }

    /// The vertices of `shared`.
    #[must_use]
    pub(crate) fn stored_buffer(&self, shared: &SharedBuffer) -> Option<&StoredBuffer> {
        self.buffers.get(shared)
    }

    /// Whether the store has the vertices `key`.
    #[cfg(test)]
    pub(crate) fn has_buffer(&self, key: crate::gfx::store::BufferKey) -> bool {
        self.buffers.contains(key)
    }

    /// Replaces the bytes of `shared` with `bytes`, as many, which the next
    /// upload writes over its buffer's. Returns false, leaving it be, for
    /// another number of bytes.
    pub(crate) fn rewrite(&mut self, shared: &SharedBuffer, bytes: &[u8]) -> bool {
        self.buffers.rewrite(shared, bytes)
    }

    /// The vertex buffer of `shared`, made of them by their first upload
    /// and written over after an update, as [`StoredBuffer::upload`] says. The
    /// engine uploads the vertices that draws bind.
    pub(crate) fn upload_buffer(
        &mut self,
        gpu: &mut dyn Gpu,
        shared: &SharedBuffer,
    ) -> Result<Option<BufferId>, GpuError> {
        self.buffers.upload(gpu, &mut self.device, shared, BufferUsage::Vertex)
    }

    /// The index buffer of `shared`, `u16`s, as
    /// [`upload_buffer`](Self::upload_buffer) makes a vertex buffer.
    pub(crate) fn upload_index_buffer(
        &mut self,
        gpu: &mut dyn Gpu,
        shared: &SharedBuffer,
    ) -> Result<Option<BufferId>, GpuError> {
        self.buffers.upload(gpu, &mut self.device, shared, BufferUsage::Index)
    }

    /// A texture without a size, made on the GPU by its first upload or the
    /// first pass that draws into it, which the context keeps while a clone
    /// of the returned handle lasts.
    #[cfg(test)]
    pub(crate) fn add_texture(&mut self) -> SharedTexture {
        self.add_texture_with(|_| {})
    }

    /// A texture that `setup` sets up, as [`add_texture`](Self::add_texture)
    /// makes it.
    pub(crate) fn add_texture_with(&mut self, setup: impl FnOnce(&mut TextureMut<'_>)) -> SharedTexture {
        let mut texture = Texture2D::new();
        setup(&mut TextureMut::new(&mut texture));
        self.textures.add(texture, &self.device.queue)
    }

    /// The texture of `handle`.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn stored_texture(&self, handle: &SharedTexture) -> Option<&Texture2D> {
        self.textures.get(handle)
    }

    /// The texture of `handle`, to set up.
    #[cfg(test)]
    pub(crate) fn stored_texture_mut(&mut self, handle: &SharedTexture) -> Option<TextureMut<'_>> {
        Some(TextureMut::new(self.textures.get_mut(handle)?))
    }

    /// The texture of `handle` and what changes it, or the error of a handle
    /// without one.
    fn texture_entry(&mut self, handle: &SharedTexture) -> Result<(&mut Texture2D, &mut Device), GpuError> {
        let Resources { device, textures, .. } = self;
        Ok((textures.get_mut(handle).ok_or(GpuError::UnknownId("texture"))?, device))
    }

    /// Makes the backend's texture of `handle`'s size and format, unless it
    /// is made. Fails for an empty size.
    pub(crate) fn create_texture(&mut self, gpu: &mut dyn Gpu, handle: &SharedTexture) -> Result<TextureId, GpuError> {
        let (texture, device) = self.texture_entry(handle)?;
        texture.create(gpu, device)
    }

    /// Writes `data`, the texels of a region of `size` in rows from the top,
    /// at `x`, `y` of the texture of `handle`, first making it.
    pub(crate) fn upload_texture_region(
        &mut self,
        gpu: &mut dyn Gpu,
        handle: &SharedTexture,
        data: &[u8],
        size: Size,
        x: u16,
        y: u16,
    ) -> Result<(), GpuError> {
        let (texture, device) = self.texture_entry(handle)?;
        texture.upload_sub_region(gpu, device, data, size, x, y)
    }

    /// Writes the image the texture of `handle` was given, if any.
    pub(crate) fn upload_texture(&mut self, gpu: &mut dyn Gpu, handle: &SharedTexture) -> Result<(), GpuError> {
        let (texture, device) = self.texture_entry(handle)?;
        if !texture.has_image() {
            return Ok(());
        }
        texture.upload(gpu, device)
    }

    /// What a draw binds of the texture of `handle`, through `gpu`: the
    /// texture, made and holding its image, and the sampler of `state`.
    pub(crate) fn texture_binding(
        &mut self,
        gpu: &mut dyn Gpu,
        handle: &SharedTexture,
        state: SamplerState,
    ) -> Result<(TextureId, SamplerId), GpuError> {
        let Resources { device, textures, samplers, .. } = self;
        let texture = textures.get_mut(handle).ok_or(GpuError::UnknownId("texture"))?;
        Ok((texture.bind(gpu, device)?, sampler(samplers, gpu, state)?))
    }

    /// The backend's texture of the texture of `handle`, once made, which
    /// bindings made of it hold.
    pub(crate) fn gpu_texture(&self, handle: &SharedTexture) -> Option<&SharedGpuTexture> {
        self.textures.get(handle)?.gpu_texture()
    }

    /// The key of the variant of `program` that reads its properties of
    /// `uniforms` from their uniforms, for `projection`, at the context's
    /// pixel ratio.
    pub(crate) fn shader_key(
        &mut self,
        program: &'static Program,
        uniforms: Uniforms,
        projection: ProjectionVariant,
    ) -> ShaderKey {
        // Only the program's own properties tell its variants apart.
        let read = program.properties.iter().enumerate().filter(move |&(index, _)| uniforms.contains(index));
        let uniforms = read.clone().map(|(index, _)| index).collect();
        let read = read.map(|(_, property)| property);
        let parameters = &self.program_parameters;
        self.shader_keys
            .entry((program, uniforms, projection))
            .or_insert_with(|| program.variant_key(parameters, read, projection))
            .clone()
    }

    /// Makes the keys of later variants at `pixel_ratio`, and with the
    /// overdraw inspector's define if `overdraw`. The shaders of the earlier
    /// keys stay until [`release_shaders`](Self::release_shaders).
    pub(crate) fn set_program_parameters(&mut self, pixel_ratio: f32, overdraw: bool) {
        self.program_parameters = ProgramParameters { pixel_ratio, overdraw };
        self.shader_keys.clear();
    }

    /// Whether the draws show the overdraw inspector.
    pub(crate) fn overdraw(&self) -> bool {
        self.program_parameters.overdraw
    }

    /// The color mode of a draw into the frame that blends with `mode`: the
    /// overdraw inspector's when it's on.
    #[must_use]
    pub(crate) fn color_mode(&self, mode: ColorMode) -> ColorMode {
        if self.program_parameters.overdraw { ColorMode::overdraw() } else { mode }
    }

    /// The shader of `key`, made through `gpu` on first use.
    pub(crate) fn shader(&mut self, gpu: &mut dyn Gpu, key: &ShaderKey) -> Result<ShaderId, GpuError> {
        if let Some(&shader) = self.shaders.get(key) {
            return Ok(shader);
        }
        let shader = gpu.create_shader(key)?;
        self.shaders.insert(key.clone(), shader);
        Ok(shader)
    }

    /// The buffers every draw binds, at the ids they bind at, which the
    /// context replaces only at its tear-down: the draws' bindings bind
    /// them.
    pub(crate) fn global_uniform_buffers(&self) -> &UniformBufferArray {
        &self.global_uniform_buffers
    }

    /// Writes `value` to the global uniform buffer `id`.
    pub(crate) fn write_global_uniform_buffer<T: bytemuck::Pod>(&mut self, gpu: &mut dyn Gpu, id: usize, value: &T) {
        let replaced = self.global_uniform_buffers.write_pod(gpu, id, value, &mut self.device);
        debug_assert!(!replaced, "the global uniform buffer {id} written with another type");
    }

    /// The pipeline of `key`, made through `gpu` on first use.
    pub(crate) fn pipeline(&mut self, gpu: &mut dyn Gpu, key: &PipelineKey) -> Result<PipelineId, GpuError> {
        if let Some(&pipeline) = self.pipelines.get(key) {
            return Ok(pipeline);
        }
        let pipeline = gpu.create_pipeline(&key.desc())?;
        self.pipelines.insert(key.clone(), pipeline);
        Ok(pipeline)
    }

    /// A vertex buffer of at least `vertex_count` default values of
    /// `format`, for an attribute a draw has no data for: zeros, or ones
    /// for `Float2` and `Float4`. The context keeps one per format, grown
    /// geometrically; geometry made of a replaced smaller one keeps it.
    pub(crate) fn default_vertex_buffer(
        &mut self,
        gpu: &mut dyn Gpu,
        format: AttributeDataType,
        vertex_count: usize,
    ) -> Result<Rc<Buffer>, GpuError> {
        let cached_count = match self.default_vertex_buffers.get(&format) {
            Some((count, buffer)) if *count >= vertex_count => return Ok(Rc::clone(buffer)),
            Some((count, _)) => *count,
            None => 0,
        };
        let count = vertex_count.max(2 * cached_count).max(1);
        let mut data = vec![0u8; format.stride() * count];
        if matches!(format, AttributeDataType::Float4 | AttributeDataType::Float2) {
            for value in data.as_chunks_mut::<4>().0 {
                value.copy_from_slice(&1.0f32.to_ne_bytes());
            }
        }
        let buffer = Rc::new(self.device.buffer(gpu, BufferUsage::Vertex, &data)?);
        self.default_vertex_buffers.insert(format, (count, Rc::clone(&buffer)));
        Ok(buffer)
    }
}

/// The sampler of `state`, with an anisotropy of at least 1, in
/// `samplers`, made through `gpu` on first use.
fn sampler(
    samplers: &mut HashMap<SamplerState, SamplerId>,
    gpu: &mut dyn Gpu,
    state: SamplerState,
) -> Result<SamplerId, GpuError> {
    let state = SamplerState { max_anisotropy: state.max_anisotropy.max(1), ..state };
    if let Some(&sampler) = samplers.get(&state) {
        return Ok(sampler);
    }
    let sampler = gpu.create_sampler(&state)?;
    samplers.insert(state, sampler);
    Ok(sampler)
}

impl Resources {
    /// Lets go of the shaders and pipelines the draws made, which the next
    /// [`destroy_released`](Self::destroy_released) destroys after the
    /// bindings and geometry released before it: for a new pixel ratio,
    /// once the draws made with them went. Draws made later make new ones.
    pub(crate) fn release_shaders(&mut self) {
        let queue = &self.device.queue;
        for (_, pipeline) in self.pipelines.drain() {
            queue.push(Release::Resource(pipeline.into()));
        }
        for (_, shader) in self.shaders.drain() {
            queue.push(Release::Resource(shader.into()));
        }
    }

    /// Destroys what the context made and what the renderer released, and
    /// tells the layers' hosts that it stopped. What drops later
    /// still goes to the queue, and goes with the `Gpu`.
    pub(crate) fn tear_down(&mut self, gpu: &mut dyn Gpu) {
        self.global_uniform_buffers = UniformBufferArray::new();
        self.default_vertex_buffers.clear();
        // The meshes first, which hold entries of the other stores.
        self.meshes.clear();
        self.surfaces.clear();
        self.quads = None;
        self.textures.clear();
        self.buffers.clear();
        self.release_shaders();
        let queue = &self.device.queue;
        for (_, sampler) in self.samplers.drain() {
            queue.push(Release::Resource(sampler.into()));
        }
        self.destroy_released(gpu);
        self.torn_down = true;
    }

    /// Runs the work after the GPU ([`AfterGpu`](crate::layer::AfterGpu))
    /// once `gpu` finished what the context gave it, if any waits.
    pub(crate) fn finish_after_gpu(&mut self, gpu: &mut dyn Gpu) {
        let work = self.after_gpu.take();
        if work.is_empty() {
            return;
        }
        if let Err(error) = gpu.flush(true) {
            Log::error(Event::Render, format!("Waiting for the GPU before the work after it failed: {error}"));
        }
        for work in work {
            work(GpuEnd::Finished);
        }
    }
}

impl Drop for Resources {
    /// A context dropped without its renderer's
    /// [`tear_down`](crate::Renderer::tear_down) leaves what it made to the
    /// `Gpu`, which frees it when it drops, and logs that it did.
    fn drop(&mut self) {
        if !self.torn_down && !std::thread::panicking() {
            Log::warning(
                Event::Render,
                "A renderer was dropped without its tear-down; its GPU objects stay until the backend goes",
            );
        }
    }
}
