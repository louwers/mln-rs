// Ports include/mln/gfx/drawable.hpp, include/mln/gfx/drawable_impl.hpp and
// src/mln/gfx/drawable.cpp
//
// C++ `gfx::Drawable` is an abstract class with data members. The port is a
// trait with a `DrawableBase` struct for the data: the non-virtual accessors
// are provided methods over `base()`, the C++ virtuals are trait methods
// (with the base-class behaviour as default where C++ has one).

use std::any::Any;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;

use geometry::Point;
use mln_tile_id::OverscaledTileID;
use mln_util::identity::SimpleIdentity;
use mln_util::time::Instant;

use crate::color_mode::ColorMode;
use crate::cull_face_mode::CullFaceMode;
use crate::draw_mode::DrawMode;
use crate::drawable_tweaker::DrawableTweakerPtr;
use crate::gfx_types::AttributeDataType;
use crate::index_vector::{IndexVectorBase, IndexVectorBasePtr};
use crate::paint_parameters::PaintParameters;
use crate::renderer::render_pass::RenderPass;
use crate::shaders::segment::SegmentBase;
use crate::shaders::shader_defines::MAX_TEXTURE_COUNT_PER_SHADER;
use crate::shaders::shader_program_base::ShaderProgramBasePtr;
use crate::texture2d::Texture2DPtr;
use crate::types::DepthMaskType;
use crate::uniform_buffer::UniformBufferArray;
use crate::vertex_attribute::VertexAttributeArrayPtr;

pub type DrawPriority = i64;

/// Array of textures to bind
pub type Textures = [Option<Texture2DPtr>; MAX_TEXTURE_COUNT_PER_SHADER];

/// `gfx::Drawable::DrawSegment` (`drawable_impl.hpp`)
#[derive(Debug)]
pub struct DrawSegment {
    mode: DrawMode,
    segment: SegmentBase,
}

impl DrawSegment {
    #[must_use]
    pub fn new(mode: DrawMode, segment: SegmentBase) -> Self {
        DrawSegment { mode, segment }
    }
    #[must_use]
    pub fn get_mode(&self) -> &DrawMode {
        &self.mode
    }
    #[must_use]
    pub fn get_segment(&self) -> &SegmentBase {
        &self.segment
    }
    pub fn get_segment_mut(&mut self) -> &mut SegmentBase {
        &mut self.segment
    }
}

pub type UniqueDrawSegment = Box<DrawSegment>;

/// The data members of `gfx::Drawable` (and of its private `Impl`).
pub struct DrawableBase {
    pub enabled: bool,
    pub enable_color: bool,
    pub enable_stencil: bool,
    pub enable_depth: bool,
    pub is_3d: bool,
    pub is_custom: bool,
    pub name: String,
    pub unique_id: SimpleIdentity,
    pub shader: Option<ShaderProgramBasePtr>,
    pub render_pass: RenderPass,
    pub tile_id: Option<OverscaledTileID>,
    pub draw_priority: DrawPriority,
    pub line_width: i32,
    pub sub_layer_index: i32,
    pub depth_type: DepthMaskType,
    /// `UniqueDrawableData` (layer-specific data).
    pub drawable_data: Option<Box<dyn Any>>,
    pub vertex_attributes: Option<VertexAttributeArrayPtr>,
    pub instance_attributes: Option<VertexAttributeArrayPtr>,
    pub attribute_update_time: Option<Instant>,
    pub textures: Textures,
    pub tweakers: Vec<DrawableTweakerPtr>,
    /// `LayerTweakerPtr`: the drawable only keeps the layer tweaker alive
    /// (C++: "used to manage the lifetime of the tweaker"). `LayerTweaker`
    /// lives in `mln_renderer`, so the handle is opaque here.
    pub layer_tweaker: Option<Rc<dyn Any>>,
    pub type_: usize,
    pub origin: Option<Point<f64>>,
    pub ubo_index: u32,
    pub create_time: Instant,
    // Drawable::Impl
    pub color_mode: ColorMode,
    pub cull_face_mode: CullFaceMode,
    /// `Impl::bucket` + `Impl::binders` (`setBinders(bucket, binders)`):
    /// buckets and binders are renderer types, so the slot is opaque here;
    /// the renderer stores its own handle and downcasts it in the tweakers.
    pub binders: Option<Rc<dyn Any>>,
    /// `Impl::renderTiles` + `Impl::renderTile` (`setRenderTile(owner,
    /// tile)`), opaque for the same reason.
    pub render_tile: Option<Rc<dyn Any>>,
}

impl DrawableBase {
    /// `Drawable(std::string name)`
    #[must_use]
    pub fn new(name: String) -> Self {
        DrawableBase {
            enabled: true,
            enable_color: true,
            enable_stencil: false,
            enable_depth: true,
            is_3d: false,
            is_custom: false,
            name,
            unique_id: SimpleIdentity::new(),
            shader: None,
            render_pass: RenderPass::Opaque,
            tile_id: None,
            draw_priority: 0,
            line_width: 1,
            sub_layer_index: 0,
            depth_type: DepthMaskType::ReadOnly,
            drawable_data: None,
            vertex_attributes: None,
            instance_attributes: None,
            attribute_update_time: None,
            textures: Default::default(),
            tweakers: Vec::new(),
            layer_tweaker: None,
            type_: 0,
            origin: None,
            ubo_index: 0,
            create_time: Instant::now(),
            color_mode: ColorMode::disabled(),
            cull_face_mode: CullFaceMode::disabled(),
            binders: None,
            render_tile: None,
        }
    }
}

pub trait Drawable: Any {
    fn base(&self) -> &DrawableBase;
    fn base_mut(&mut self) -> &mut DrawableBase;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;

    // --- pure virtuals ---

    /// Draw the drawable. (C++ `draw(PaintParameters&) const`; the backends
    /// mutate their impl state, so `&mut self` here.)
    fn draw(&mut self, parameters: &mut dyn PaintParameters);

    /// Update vertices, indices, and segments
    fn update_vertex_attributes(
        &mut self,
        vertices: Option<VertexAttributeArrayPtr>,
        vertex_count: usize,
        mode: DrawMode,
        indexes: Option<IndexVectorBasePtr>,
        segments: &[SegmentBase],
    );

    /// Provide raw data for vertices. Incompatible with adding primitives
    fn set_vertices(&mut self, data: Vec<u8>, count: usize, type_: AttributeDataType);

    /// Set shared indexes and segments
    fn set_index_data(&mut self, indexes: Option<IndexVectorBasePtr>, segments: Vec<UniqueDrawSegment>);

    /// Get the uniform buffers attached to this drawable
    fn get_uniform_buffers(&self) -> &UniformBufferArray;
    fn mutable_uniform_buffers(&mut self) -> &mut UniformBufferArray;

    // --- virtuals with a base implementation ---

    fn set_shader(&mut self, value: Option<ShaderProgramBasePtr>) {
        self.base_mut().shader = value;
    }
    fn set_line_width(&mut self, value: i32) {
        self.base_mut().line_width = value;
    }
    fn set_textures(&mut self, textures: Textures) {
        self.base_mut().textures = textures;
    }
    /// Attach the given texture to this drawable at the given internal ID.
    fn set_texture(&mut self, texture: Option<Texture2DPtr>, id: usize) {
        debug_assert!(id < self.base().textures.len());
        if id >= self.base().textures.len() {
            return;
        }
        self.base_mut().textures[id] = texture;
    }
    fn set_enable_color(&mut self, value: bool) {
        self.base_mut().enable_color = value;
    }
    fn set_enable_stencil(&mut self, value: bool) {
        self.base_mut().enable_stencil = value;
    }
    fn set_enable_depth(&mut self, value: bool) {
        self.base_mut().enable_depth = value;
    }
    fn set_sub_layer_index(&mut self, value: i32) {
        self.base_mut().sub_layer_index = value;
    }
    fn set_depth_type(&mut self, value: DepthMaskType) {
        self.base_mut().depth_type = value;
    }
    fn set_cull_face_mode(&mut self, value: &CullFaceMode) {
        self.base_mut().cull_face_mode = *value;
    }
    fn set_color_mode(&mut self, value: &ColorMode) {
        self.base_mut().color_mode = *value;
    }

    // --- non-virtual members ---

    fn get_id(&self) -> SimpleIdentity {
        self.base().unique_id
    }
    fn get_name(&self) -> &str {
        &self.base().name
    }
    fn set_name(&mut self, value: String) {
        self.base_mut().name = value;
    }
    fn get_shader(&self) -> &Option<ShaderProgramBasePtr> {
        &self.base().shader
    }
    fn get_render_pass(&self) -> RenderPass {
        self.base().render_pass
    }
    fn set_render_pass(&mut self, value: RenderPass) {
        self.base_mut().render_pass = value;
    }
    /// Test whether to draw this drawable in a given render pass.
    fn has_render_pass(&self, value: RenderPass) -> bool {
        (self.base().render_pass.underlying_type() & value.underlying_type()) != 0
    }
    /// If multiple render pass bits are set, all must be present.
    fn has_all_render_passes(&self, value: RenderPass) -> bool {
        let v = value.underlying_type();
        (self.base().render_pass.underlying_type() & v) == v
    }
    fn get_line_width(&self) -> i32 {
        self.base().line_width
    }
    fn get_texture(&self, id: usize) -> Option<&Texture2DPtr> {
        self.base().textures.get(id).and_then(Option::as_ref)
    }
    fn get_enabled(&self) -> bool {
        self.base().enabled
    }
    fn set_enabled(&mut self, value: bool) {
        self.base_mut().enabled = value;
    }
    fn get_enable_color(&self) -> bool {
        self.base().enable_color
    }
    fn get_enable_stencil(&self) -> bool {
        self.base().enable_stencil
    }
    fn get_draw_priority(&self) -> DrawPriority {
        self.base().draw_priority
    }
    fn set_draw_priority(&mut self, value: DrawPriority) {
        self.base_mut().draw_priority = value;
    }
    fn get_enable_depth(&self) -> bool {
        self.base().enable_depth
    }
    fn get_sub_layer_index(&self) -> i32 {
        self.base().sub_layer_index
    }
    fn get_depth_type(&self) -> DepthMaskType {
        self.base().depth_type
    }
    fn get_is_3d(&self) -> bool {
        self.base().is_3d
    }
    fn set_is_3d(&mut self, value: bool) {
        self.base_mut().is_3d = value;
    }
    fn get_is_custom(&self) -> bool {
        self.base().is_custom
    }
    fn set_is_custom(&mut self, value: bool) {
        self.base_mut().is_custom = value;
    }
    fn get_tile_id(&self) -> Option<OverscaledTileID> {
        self.base().tile_id
    }
    fn set_tile_id(&mut self, value: OverscaledTileID) {
        self.base_mut().tile_id = Some(value);
    }
    fn get_cull_face_mode(&self) -> &CullFaceMode {
        &self.base().cull_face_mode
    }
    fn get_color_mode(&self) -> &ColorMode {
        &self.base().color_mode
    }
    fn get_vertex_attributes(&self) -> Option<&VertexAttributeArrayPtr> {
        self.base().vertex_attributes.as_ref()
    }
    fn set_vertex_attributes(&mut self, value: Option<VertexAttributeArrayPtr>) {
        let base = self.base_mut();
        base.vertex_attributes = value;
        // The attribute bindings need to be rebuilt, we can't rely on the
        // update time check as these new values may not have been modified
        // recently.
        base.attribute_update_time = None;
    }
    fn get_instance_attributes(&self) -> Option<&VertexAttributeArrayPtr> {
        self.base().instance_attributes.as_ref()
    }
    fn set_instance_attributes(&mut self, value: Option<VertexAttributeArrayPtr>) {
        self.base_mut().instance_attributes = value;
    }
    /// Provide raw indexes and segments
    fn set_index_data_vec(&mut self, indexes: Vec<u16>, segments: Vec<UniqueDrawSegment>) {
        self.set_index_data(Some(Rc::new(RefCell::new(IndexVectorBase::new(indexes)))), segments);
    }
    fn get_tweakers(&self) -> &Vec<DrawableTweakerPtr> {
        &self.base().tweakers
    }
    fn add_tweaker(&mut self, value: DrawableTweakerPtr) {
        self.base_mut().tweakers.push(value);
    }
    fn set_tweakers(&mut self, value: Vec<DrawableTweakerPtr>) {
        self.base_mut().tweakers = value;
    }
    fn clear_tweakers(&mut self) {
        self.base_mut().tweakers.clear();
    }
    fn get_data(&self) -> Option<&dyn Any> {
        self.base().drawable_data.as_deref()
    }
    fn set_data(&mut self, value: Option<Box<dyn Any>>) {
        self.base_mut().drawable_data = value;
    }
    fn set_type(&mut self, type_: usize) {
        self.base_mut().type_ = type_;
    }
    fn get_type(&self) -> usize {
        self.base().type_
    }
    fn set_ubo_index(&mut self, ubo_index: u32) {
        self.base_mut().ubo_index = ubo_index;
    }
    fn get_ubo_index(&self) -> u32 {
        self.base().ubo_index
    }
    /// Associate the drawable with a layer tweaker (lifetime management).
    fn set_layer_tweaker(&mut self, tweaker: Option<Rc<dyn Any>>) {
        self.base_mut().layer_tweaker = tweaker;
    }
    fn get_layer_tweaker(&self) -> Option<&Rc<dyn Any>> {
        self.base().layer_tweaker.as_ref()
    }
    /// `setBinders(std::shared_ptr<Bucket>, PaintPropertyBindersBase*)`.
    fn set_binders(&mut self, binders: Option<Rc<dyn Any>>) {
        self.base_mut().binders = binders;
    }
    /// `getBinders()` (+ `getBucket()`), as the renderer stored them.
    fn get_binders(&self) -> Option<&Rc<dyn Any>> {
        self.base().binders.as_ref()
    }
    /// `setRenderTile(Immutable<std::vector<RenderTile>>, const RenderTile*)`.
    fn set_render_tile(&mut self, render_tile: Option<Rc<dyn Any>>) {
        self.base_mut().render_tile = render_tile;
    }
    /// `getRenderTile()`.
    fn get_render_tile(&self) -> Option<&Rc<dyn Any>> {
        self.base().render_tile.as_ref()
    }
    fn get_origin(&self) -> Option<Point<f64>> {
        self.base().origin
    }
    fn set_origin(&mut self, p: Option<Point<f64>>) {
        self.base_mut().origin = p;
    }
    fn get_attribute_update_time(&self) -> Option<Instant> {
        self.base().attribute_update_time
    }
}

pub type UniqueDrawable = Box<dyn Drawable>;

/// `DrawableLessByPriority`: primarily by draw priority, then by id.
pub fn drawable_less_by_priority(left: &dyn Drawable, right: &dyn Drawable, descending: bool) -> bool {
    drawable_cmp_by_priority(left, right, descending) == Ordering::Less
}

/// The ordering behind `DrawableLessByPriority`.
pub fn drawable_cmp_by_priority(left: &dyn Drawable, right: &dyn Drawable, descending: bool) -> Ordering {
    let (a, b) = if descending { (right, left) } else { (left, right) };
    if a.get_draw_priority() != b.get_draw_priority() {
        return a.get_draw_priority().cmp(&b.get_draw_priority());
    }
    a.get_id().cmp(&b.get_id())
}
