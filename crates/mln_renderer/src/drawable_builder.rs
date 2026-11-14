use std::cell::RefCell;
use std::rc::Rc;

use geometry::Point;
use mln_gfx::color_mode::ColorMode;
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::draw_mode::DrawMode;
use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::index_vector::{IndexVectorBase, IndexVectorBasePtr};
use mln_gfx::polyline_generator::{PolylineGenerator, PolylineGeneratorOptions};
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{ID_LINE_DATA_VERTEX_ATTRIBUTE, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE};
use mln_gfx::shaders::shader_program::ShaderProgramPtr;
use mln_gfx::texture2d::Texture2DPtr;
use mln_gfx::types::DepthMaskType;
use mln_gfx::vertex_attribute::{VertexAttributeArray, VertexAttributeArrayPtr};
use mln_gfx::vertex_vector::{VertexVector, VertexVectorBasePtr};
use mln_tile_id::geometry_tile_data::GeometryCoordinates;
use mln_util::logging::{Event, Log};

use crate::drawable::{DrawPriority, DrawSegment, Drawable, Textures, UniqueDrawSegment, UniqueDrawable};
use crate::gfx::drawable_atlases_tweaker::DrawableAtlasesTweaker;
use crate::render_pass::RenderPass;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// Triangles and quads, added one by one.
    Primitives,
    /// Thick polylines.
    Polylines,
    /// Vertices and indexes the caller made.
    Custom,
}

type VT = [i16; 2];

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct LineLayoutVertex {
    pub(crate) a1: [i16; 2],
    pub(crate) a2: [u8; 4],
}

/// Builds drawables: it takes their state, then their geometry, and
/// [`flush`](Self::flush) makes a drawable of what it has so far.
pub(crate) struct DrawableBuilder {
    drawable_name: String,
    vertex_attr_id: usize,
    render_pass: RenderPass,
    enable_stencil: bool,
    enable_depth: bool,
    is_3d: bool,
    draw_priority: DrawPriority,
    sub_layer_index: i32,
    depth_type: DepthMaskType,
    shader: Option<ShaderProgramPtr>,
    current_drawable: Option<UniqueDrawable>,
    drawables: Vec<UniqueDrawable>,
    textures: Textures,
    atlases: Option<Rc<DrawableAtlasesTweaker>>,
    vertex_attrs: Option<VertexAttributeArrayPtr>,
    instance_attrs: Option<VertexAttributeArrayPtr>,
    origin: Option<Point<f64>>,
    vertices: VertexVector<VT>,
    raw_vertices: Vec<u8>,
    raw_vertices_count: usize,
    raw_vertices_type: AttributeDataType,
    build_indexes: Vec<u16>,
    shared_indexes: Option<IndexVectorBasePtr>,
    segments: Vec<UniqueDrawSegment>,
    color_mode: ColorMode,
    cull_face_mode: CullFaceMode,
    /// The kind of geometry the builder takes. It stays for the drawables
    /// after the first.
    mode: Mode,
    polyline_vertices: Vec<LineLayoutVertex>,
    polyline_indexes: Vec<u16>,
}

pub(crate) type UniqueDrawableBuilder = Box<DrawableBuilder>;

impl Default for DrawableBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl DrawableBuilder {
    #[must_use]
    pub(crate) fn new() -> Self {
        DrawableBuilder {
            drawable_name: String::new(),
            vertex_attr_id: 0,
            render_pass: RenderPass::Opaque,
            enable_stencil: false,
            enable_depth: true,
            is_3d: false,
            draw_priority: 0,
            sub_layer_index: 0,
            depth_type: DepthMaskType::ReadOnly,
            shader: None,
            current_drawable: None,
            drawables: Vec::new(),
            textures: Default::default(),
            atlases: None,
            vertex_attrs: None,
            instance_attrs: None,
            origin: None,
            vertices: VertexVector::default(),
            raw_vertices: Vec::new(),
            raw_vertices_count: 0,
            raw_vertices_type: AttributeDataType::Invalid,
            build_indexes: Vec::new(),
            shared_indexes: None,
            segments: Vec::new(),
            color_mode: ColorMode::disabled(),
            cull_face_mode: CullFaceMode::disabled(),
            mode: Mode::Custom,
            polyline_vertices: Vec::new(),
            polyline_indexes: Vec::new(),
        }
    }

    /// A new builder in a box, as the layers keep them.
    #[must_use]
    pub(crate) fn boxed() -> UniqueDrawableBuilder {
        Box::new(Self::new())
    }

    fn check_and_set_mode(&mut self, target: Mode) -> bool {
        if target != self.mode && self.cur_vertex_count() != 0 {
            debug_assert!(false, "DrawableBuilder mode mismatch. Target is {target:?}, current is {:?}", self.mode);
            return false;
        }
        self.mode = target;
        true
    }

    #[must_use]
    fn layout_vertex(p: Point<i16>, e: Point<f64>, round: bool, up: bool, dir: i8, linesofar: i32) -> LineLayoutVertex {
        // Scale the extrusion vector so that the normal length is this value.
        // Contains the "texture" normals (-1..1). This is distinct from the
        // extrude normals for line joins, because the x-value remains 0 for
        // the texture normal array, while the extrude normal actually moves
        // the vertex to create the acute/bevelled line join.
        const EXTRUDE_SCALE: f64 = 63.0;
        let dir_value = i32::from(dir.signum());
        LineLayoutVertex {
            a1: [((i32::from(p.x) * 2) | i32::from(round)) as i16, ((i32::from(p.y) * 2) | i32::from(up)) as i16],
            a2: [
                // add 128 to store a byte in an unsigned byte
                ((EXTRUDE_SCALE * e.x).round() + 128.0) as u8,
                ((EXTRUDE_SCALE * e.y).round() + 128.0) as u8,
                // Encode the -1/0/1 direction value into the first two bits
                // of .z of a_data. Combine it with the lower 6 bits of
                // `linesofar` (shifted by 2 bites to make room for the
                // direction value). The upper 8 bits of `linesofar` are
                // placed in the `w` component.
                ((dir_value + 1) | ((linesofar & 0x3F) << 2)) as u8,
                (linesofar >> 6) as u8,
            ],
        }
    }

    /// Makes a drawable of the geometry added since the last, if any, and
    /// starts the next.
    pub(crate) fn flush(&mut self) {
        if self.cur_vertex_count() != 0 {
            if self.mode == Mode::Polylines {
                self.setup_for_polylines();
            }
            let mut drawable =
                self.current_drawable.take().unwrap_or_else(|| Box::new(Drawable::new(self.drawable_name.clone())));
            drawable.enable_stencil = self.enable_stencil;
            drawable.render_pass = self.render_pass;
            drawable.draw_priority = self.draw_priority;
            drawable.sub_layer_index = self.sub_layer_index;
            drawable.enable_depth = self.enable_depth;
            drawable.depth_type = self.depth_type;
            drawable.is_3d = self.is_3d;
            drawable.set_color_mode(&self.color_mode);
            drawable.set_cull_face_mode(self.cull_face_mode);
            drawable.set_shader(self.shader.clone());
            drawable.textures = self.textures.clone();
            drawable.atlases = self.atlases.clone();
            drawable.origin = self.origin;
            let vertex_attrs = self.vertex_attrs.clone().unwrap_or_else(VertexAttributeArray::new_ptr);
            drawable.set_vertex_attributes(Some(vertex_attrs));
            if let Some(attrs) = &self.instance_attrs {
                drawable.instance_attributes = Some(Rc::clone(attrs));
            }
            self.init(&mut drawable);
            self.origin = None;
            self.current_drawable = Some(drawable);
        }
        if let Some(drawable) = self.current_drawable.take() {
            self.drawables.push(drawable);
        }
    }

    /// Gives `drawable` the builder's geometry.
    fn init(&mut self, drawable: &mut Drawable) {
        drawable.set_vertex_attr_id(self.vertex_attr_id);

        if self.raw_vertices_count != 0 && self.raw_vertices.is_empty() {
            // The vertex data is in the vertex attributes (fills).
            if drawable.vertex_attributes.is_some() && !self.segments.is_empty() {
                // The draw mode of the first segment; set_index_data below
                // sets the segments.
                let draw_mode = *self.segments[0].mode();
                let attrs = drawable.vertex_attributes.clone();
                drawable.update_vertex_attributes(
                    attrs,
                    self.raw_vertices_count,
                    draw_mode,
                    self.shared_indexes.clone(),
                    &[],
                );
            }
        } else if self.raw_vertices_count != 0 {
            let raw = std::mem::take(&mut self.raw_vertices);
            drawable.set_vertices(raw, self.raw_vertices_count, self.raw_vertices_type);
        } else if !self.vertices.empty() {
            let vertices = self.vertices.data();
            if !vertices.is_empty() {
                let raw: Vec<u8> = bytemuck::cast_slice(&vertices).to_vec();
                drawable.set_vertices(raw, vertices.len(), AttributeDataType::Short2);
            }
        }

        if self.shared_indexes.is_none() && !self.build_indexes.is_empty() {
            let indexes = std::mem::take(&mut self.build_indexes);
            self.shared_indexes = Some(Rc::new(RefCell::new(IndexVectorBase::new(indexes))));
        }
        if let Some(indexes) = self.shared_indexes.clone().filter(|i| i.borrow().elements() != 0) {
            let segments = std::mem::take(&mut self.segments);
            drawable.set_index_data(Some(indexes), segments);
        } else {
            Log::warning(Event::Render, format!("drawable {} has no indexes", drawable.name));
        }

        // The next drawable doesn't get this one's textures.
        for texture in &mut self.textures {
            *texture = None;
        }
    }

    /// Takes the drawables made so far.
    pub(crate) fn clear_drawables(&mut self) -> Vec<UniqueDrawable> {
        std::mem::take(&mut self.drawables)
    }

    pub(crate) fn set_render_pass(&mut self, value: RenderPass) {
        self.render_pass = value;
    }
    pub(crate) fn set_draw_priority(&mut self, value: DrawPriority) {
        self.draw_priority = value;
        if let Some(drawable) = self.current_drawable.as_mut() {
            drawable.draw_priority = value;
        }
    }
    pub(crate) fn set_sub_layer_index(&mut self, value: i32) {
        self.sub_layer_index = value;
    }
    pub(crate) fn set_enable_depth(&mut self, value: bool) {
        self.enable_depth = value;
    }
    pub(crate) fn set_depth_type(&mut self, value: DepthMaskType) {
        self.depth_type = value;
    }
    /// Makes the drawables 3D. They write depth unless the caller sets
    /// [`DepthMaskType::ReadOnly`] after this.
    pub(crate) fn set_is_3d(&mut self, value: bool) {
        self.is_3d = value;
        if value {
            self.depth_type = DepthMaskType::ReadWrite;
        }
    }
    pub(crate) fn set_enable_stencil(&mut self, value: bool) {
        self.enable_stencil = value;
    }
    pub(crate) fn set_color_mode(&mut self, value: &ColorMode) {
        self.color_mode = *value;
    }
    pub(crate) fn set_cull_face_mode(&mut self, value: CullFaceMode) {
        self.cull_face_mode = value;
    }
    pub(crate) fn set_shader(&mut self, value: Option<ShaderProgramPtr>) {
        self.shader = value;
    }
    /// Sets the name given to new drawables.
    pub(crate) fn set_drawable_name(&mut self, value: String) {
        self.drawable_name = value;
    }
    /// Sets the attribute id of the raw vertices.
    pub(crate) fn set_vertex_attr_id(&mut self, id: usize) {
        self.vertex_attr_id = id;
    }
    pub(crate) fn set_texture(&mut self, texture: Option<Texture2DPtr>, id: usize) {
        debug_assert!(id < self.textures.len());
        if let Some(slot) = self.textures.get_mut(id) {
            *slot = texture;
        }
    }
    /// Sets the tweaker that gives the drawables the symbol atlases'
    /// textures.
    pub(crate) fn set_atlases(&mut self, value: Option<Rc<DrawableAtlasesTweaker>>) {
        self.atlases = value;
    }
    pub(crate) fn set_vertex_attributes(&mut self, attrs: Option<VertexAttributeArrayPtr>) {
        self.vertex_attrs = attrs;
    }
    pub(crate) fn set_instance_attributes(&mut self, attrs: Option<VertexAttributeArrayPtr>) {
        self.instance_attrs = attrs;
    }

    /// Takes `count` vertices of `type_` in `data`, or, with no `data`, a
    /// vertex count for vertices in the vertex attributes.
    pub(crate) fn set_raw_vertices(&mut self, data: Vec<u8>, count: usize, type_: AttributeDataType) {
        self.raw_vertices = data;
        self.raw_vertices_count = count;
        self.raw_vertices_type = type_;
    }

    /// Sets indexes and segments
    pub(crate) fn set_segments(&mut self, mode: DrawMode, indexes: Vec<u16>, segments: &[SegmentBase]) {
        self.set_segments_shared(mode, Some(Rc::new(RefCell::new(IndexVectorBase::new(indexes)))), segments);
    }

    /// Sets shared indexes and segments
    pub(crate) fn set_segments_shared(
        &mut self,
        mode: DrawMode,
        indexes: Option<IndexVectorBasePtr>,
        segments: &[SegmentBase],
    ) {
        let Some(indexes) = indexes.filter(|i| i.borrow().elements() != 0) else {
            debug_assert!(false, "Missing segment indexes");
            return;
        };
        #[cfg(debug_assertions)]
        {
            use mln_gfx::DrawModeType;
            let vertex_count = self.cur_vertex_count();
            let shared = indexes.borrow();
            for seg in segments {
                if mode.type_ == DrawModeType::Triangles {
                    debug_assert_eq!(seg.index_length % 3, 0);
                } else if mode.type_ == DrawModeType::Lines {
                    debug_assert_eq!(seg.index_length % 2, 0);
                }
                debug_assert!(vertex_count == 0 || seg.vertex_offset + seg.vertex_length <= vertex_count);
                debug_assert!(seg.index_offset + seg.index_length <= shared.elements());
                // The CPU copy of a released index vector is freed once it is uploaded.
                if vertex_count != 0 && seg.index_offset + seg.index_length <= shared.vector().len() {
                    for j in 0..seg.index_length {
                        debug_assert!((shared.vector()[seg.index_offset + j] as usize) < vertex_count);
                    }
                }
            }
        }
        self.shared_indexes = Some(indexes);
        for seg in segments {
            self.segments.push(Box::new(DrawSegment::new(mode, seg.copy())));
        }
    }

    /// Add a triangle
    pub(crate) fn add_triangle(&mut self, x0: i16, y0: i16, x1: i16, y1: i16, x2: i16, y2: i16) {
        // check and set the current mode
        if !self.check_and_set_mode(Mode::Primitives) {
            return;
        }
        let needs_segment = self.segments.is_empty();
        let new_segment = if needs_segment {
            Some(Box::new(DrawSegment::new(DrawMode::Triangles(), SegmentBase::new(0, 0))))
        } else {
            None
        };
        let n = self.vertices.elements() as u16;
        self.vertices.emplace_back([x0, y0]);
        self.vertices.emplace_back([x1, y1]);
        self.vertices.emplace_back([x2, y2]);
        self.build_indexes.extend_from_slice(&[n, n.wrapping_add(1), n.wrapping_add(2)]);
        if let Some(segment) = new_segment {
            self.segments.push(segment);
        }
        let segment = self.segments.last_mut().unwrap().segment_mut();
        segment.vertex_length += 3;
        segment.index_length += 3;
    }

    /// Add another triangle based on the previous two points
    pub(crate) fn append_triangle(&mut self, x0: i16, y0: i16) {
        // check and set the current mode
        if !self.check_and_set_mode(Mode::Primitives) {
            return;
        }
        let n = self.vertices.elements() as u16;
        self.vertices.emplace_back([x0, y0]);
        self.build_indexes.extend_from_slice(&[n.wrapping_sub(2), n.wrapping_sub(1), n]);
        debug_assert!(!self.segments.is_empty());
        let segment = self.segments.last_mut().unwrap().segment_mut();
        segment.vertex_length += 1;
        segment.index_length += 3;
    }

    /// Add a rectangle consisting of two triangles
    pub(crate) fn add_quad(&mut self, x0: i16, y0: i16, x1: i16, y1: i16) {
        // check and set the current mode
        if !self.check_and_set_mode(Mode::Primitives) {
            return;
        }
        self.add_triangle(x0, y0, x1, y0, x0, y1);
        self.append_triangle(x1, y1);
    }

    /// Add a polyline. If the last point equals the first it will be
    /// closed, otherwise open
    pub(crate) fn add_polyline(&mut self, coordinates: &GeometryCoordinates, options: &PolylineGeneratorOptions) {
        fn segment(segment: &mut UniqueDrawSegment) -> &mut SegmentBase {
            segment.segment_mut()
        }
        // mark the current mode
        if !self.check_and_set_mode(Mode::Polylines) {
            return;
        }

        let create_segment = |vertex_offset: usize, index_offset: usize| {
            Box::new(DrawSegment::new(DrawMode::Triangles(), SegmentBase::new(vertex_offset, index_offset)))
        };
        let mut generator = PolylineGenerator::new(
            &mut self.polyline_vertices,
            Self::layout_vertex,
            &mut self.segments,
            &create_segment,
            segment,
            &mut self.polyline_indexes,
        );
        generator.generate(coordinates, options);
    }

    fn setup_for_polylines(&mut self) {
        // setup vertex attributes
        self.set_vertex_attr_id(ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE);
        let count = self.polyline_vertices.len();
        self.set_raw_vertices(Vec::new(), count, AttributeDataType::Short2);

        let attrs = VertexAttributeArray::new_ptr();
        let vertices = std::mem::take(&mut self.polyline_vertices);
        let verts: VertexVectorBasePtr = Rc::new(VertexVector::from_vec(vertices));

        {
            let mut attrs = attrs.borrow_mut();
            if let Some(attr) = attrs.set(ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
                attr.set_shared_raw_data(
                    Some(Rc::clone(&verts)),
                    0, // offsetof(LineLayoutVertex, a1)
                    /*vertexOffset=*/ 0,
                    size_of::<LineLayoutVertex>() as u32,
                    AttributeDataType::Short2,
                );
            }
            if let Some(attr) = attrs.set(ID_LINE_DATA_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
                attr.set_shared_raw_data(
                    Some(verts),
                    4, // offsetof(LineLayoutVertex, a2)
                    /*vertexOffset=*/ 0,
                    size_of::<LineLayoutVertex>() as u32,
                    AttributeDataType::UByte4,
                );
            }
        }

        self.set_vertex_attributes(Some(attrs));

        let indexes = std::mem::take(&mut self.polyline_indexes);
        self.shared_indexes = Some(Rc::new(RefCell::new(IndexVectorBase::new(indexes))));
    }

    /// return the current vertex count
    pub(crate) fn cur_vertex_count(&self) -> usize {
        self.raw_vertices_count.max(self.vertices.elements()).max(self.polyline_vertices.len())
    }
}
