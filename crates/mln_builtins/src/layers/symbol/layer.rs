// A tile's draws read the bucket's layout vertices and copies of what
// placement lays out. When placement, sorting or feature state change the
// bucket (the changes the tile's entry in the layer's `TileMap` collects),
// the draw update replaces just those attributes (opacity, dynamic
// positions, binders) and the triangle indices of the tile's draws.

use glam::dcamera::rh::proj::opengl::orthographic;
use glam::{DMat4, DVec3};
use mln_engine::camera::{Projection, ProjectionData};
use mln_engine::draw::ground::FrameGround;
use mln_engine::draw::projection::{TileTransform, tile_projection};
use mln_engine::draw::{
    Draw, Draws, LayerFrame, Mesh, MeshDesc, RenderPass, RenderTile, Resources, Uniforms, VertexAttribute,
    VertexBinding, VertexBuffer,
};
use mln_engine::layer::binders::{PaintAttributes, PaintBinders as _};
use mln_engine::layer::{
    BucketChanges, BucketGeometry, EvaluateCx, EvaluatedLayer, Evaluation, Features, FrameTile, Layer, LayerValues,
    OnTerrain, PrepareCx, TileOrder, bucket_vertices, paint_vertices,
};
use mln_engine::tile::{OverscaledTileID, UnwrappedTileID};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode};
use mln_style::StyleValue as _;
use mln_style::color::Color;
use mln_style::constants::EXTENT;
use mln_style::hash::HashMap;
use mln_style::layer::ErasedLayerStyle as _;
use mln_style::layers::Symbol;
use mln_style::layers::symbol_layer::SymbolLayerPaintPropertyOverrides;
use mln_style::layers::symbol_layer_properties::{SymbolLayoutProperties, SymbolPaintProperties};
use mln_style::types::{AlignmentType, IconTextFitType, SymbolPlacementType, TextRotationAlignmentType};

use crate::layers::atlas_binding::AtlasBinding;
use crate::layers::segments::SegmentBase;
use crate::layers::symbol::bucket::{
    Buffer, PaintProperties, SymbolBucket, SymbolDynamicLayoutVertex, SymbolOpacityVertex,
};
use crate::layers::symbol::layout::SymbolLayout;
use crate::layers::symbol::programs;
use crate::layers::symbol::projection::{gl_coord_matrix, label_plane_matrix};
use crate::layers::symbol::ubo::{SymbolDrawableUBO, SymbolEvaluatedPropsUBO, SymbolTilePropsUBO};

/// The slots of the programs' textures that the draws bind.
const GROUND_DEPTH_SLOT: usize = programs::SYMBOL_SDF.texture_slot("ground_depth");
const TEXTURE_SLOT: usize = programs::SYMBOL_SDF.texture_slot("texture");
const TEXTURE_ICON_SLOT: usize = programs::SYMBOL_TEXT_AND_ICON.texture_slot("texture_icon");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SymbolType {
    Text,
    IconRGBA,
    IconSDF,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SymbolPropertyValues {
    pub(crate) pitch_alignment: AlignmentType,
    /// `Map` when the symbols rotate with the map, otherwise `Viewport`.
    pub(crate) rotation_alignment: AlignmentType,
    /// Whether the symbols' glyphs or icons are placed along their lines.
    pub(crate) along_line: bool,
}

/// What a symbol draw draws, which its uniforms read.
#[derive(Clone, Copy, Debug)]
struct SymbolDrawData {
    is_halo: bool,
    bucket_variable_placement: bool,
    symbol_type: SymbolType,
    pitch_alignment: AlignmentType,
    rotation_alignment: AlignmentType,
    along_line: bool,
    text_fit: IconTextFitType,
    is_offset: bool,
    /// The bucket segment the draw draws, for a layer that sorts its
    /// features by key, or all of them.
    segment: Option<usize>,
}

#[must_use]
pub(crate) fn icon_property_values(layout: &SymbolLayoutProperties::PossiblyEvaluated) -> SymbolPropertyValues {
    let rotation_alignment = *layout.icon_rotation_alignment.uniform();
    SymbolPropertyValues {
        pitch_alignment: *layout.icon_pitch_alignment.uniform(),
        rotation_alignment,
        along_line: rotation_alignment == AlignmentType::Map
            && *layout.symbol_placement.uniform() != SymbolPlacementType::Point,
    }
}

#[must_use]
pub(crate) fn text_property_values(layout: &SymbolLayoutProperties::PossiblyEvaluated) -> SymbolPropertyValues {
    let rotation_alignment = *layout.text_rotation_alignment.uniform();
    SymbolPropertyValues {
        pitch_alignment: *layout.text_pitch_alignment.uniform(),
        rotation_alignment: if rotation_alignment == TextRotationAlignmentType::Map {
            AlignmentType::Map
        } else {
            AlignmentType::Viewport
        },
        along_line: rotation_alignment != TextRotationAlignmentType::Viewport
            && *layout.symbol_placement.uniform() != SymbolPlacementType::Point,
    }
}

/// The tile matrix of screen-space symbols.
pub(crate) fn screen_matrix(translation: [f32; 2]) -> DMat4 {
    let extent = f64::from(EXTENT);
    orthographic(0.0, extent, -extent, 0.0, 0.0, 1.0)
        * DMat4::from_translation(DVec3::new(f64::from(translation[0]), f64::from(translation[1]) - extent, 0.0))
}

/// The attributes of the layout vertices ([`SymbolLayoutVertex`]).
///
/// [`SymbolLayoutVertex`]: crate::layers::symbol::bucket::SymbolLayoutVertex
const LAYOUT: &[VertexAttribute] = &[
    VertexAttribute { name: "pos_offset", format: AttributeDataType::Short4, offset: 0 },
    VertexAttribute { name: "data", format: AttributeDataType::UShort4, offset: 8 },
    VertexAttribute { name: "pixeloffset", format: AttributeDataType::Short4, offset: 16 },
    VertexAttribute { name: "tex_origin", format: AttributeDataType::UShort2, offset: 24 },
];
/// The symbols' heights: the layout vertices' `symbol-height-offset`s, or
/// those placement gave them over the ground, an `f32` a vertex.
const HEIGHT_OFFSET: &[VertexAttribute] =
    &[VertexAttribute { name: "height_offset", format: AttributeDataType::Float, offset: 28 }];
const ELEVATION: &[VertexAttribute] =
    &[VertexAttribute { name: "height_offset", format: AttributeDataType::Float, offset: 0 }];
const PROJECTED_POS: &[VertexAttribute] =
    &[VertexAttribute { name: "projected_pos", format: AttributeDataType::Float4, offset: 0 }];
const FADE_OPACITY: &[VertexAttribute] =
    &[VertexAttribute { name: "fade_opacity", format: AttributeDataType::Float, offset: 0 }];

/// The size of a layout vertex.
const LAYOUT_SIZE: u32 = size_of::<crate::layers::symbol::bucket::SymbolLayoutVertex>() as u32;

/// The vertices a draw of a tile reads, which the draws of the tile that
/// read the same attributes share: of the bucket's paint values and layout
/// vertices, and those that placement lays out, of the symbols' positions
/// along lines or at variable anchors, of their opacities and of their
/// heights.
#[derive(Clone, Default)]
struct SymbolVertices {
    paint: Option<VertexBinding>,
    layout: Option<VertexBinding>,
    /// The heights: the layout vertices', or `elevation`'s.
    height: Option<VertexBinding>,
    projected: Option<VertexBinding>,
    opacity: Option<VertexBinding>,
    /// The heights over the ground, where placement gives them.
    elevation: Option<VertexBuffer>,
}

impl SymbolVertices {
    /// The vertices of `buffer`'s symbols, of the layer `layer_id`, through
    /// `res`, with the uniforms of the paint properties the program reads
    /// from them.
    fn new(
        res: &mut Resources<'_>,
        geometry: &BucketGeometry,
        buffer: &Buffer,
        layer_id: &str,
        is_text: bool,
        paint_props: &PaintProperties,
        evaluated: &SymbolPaintProperties::PossiblyEvaluated,
    ) -> (SymbolVertices, Uniforms) {
        let paint = binder_attributes(res, geometry, is_text, paint_props, evaluated);
        let mut vertices = SymbolVertices { paint: paint.vertices, ..SymbolVertices::default() };
        if let Some(layout_data) = bucket_vertices(geometry, &buffer.attribute_data) {
            vertices.layout = Some(VertexBinding::new(&layout_data, LAYOUT_SIZE, LAYOUT));
            vertices.height = Some(VertexBinding::new(&layout_data, LAYOUT_SIZE, HEIGHT_OFFSET));
        }
        vertices.set_projected_pos(res, buffer, layer_id);
        vertices.set_fade_opacity(res, buffer);
        vertices.set_elevation(res, geometry, buffer);
        (vertices, paint.uniforms)
    }

    /// What the draws bind of the vertices.
    fn bindings(&self) -> impl Iterator<Item = &VertexBinding> {
        [&self.paint, &self.layout, &self.height, &self.projected, &self.opacity].into_iter().flatten()
    }

    /// Sets the symbols' heights to the heights placement gave them over the
    /// ground ([`Buffer::elevation_attribute_data`]), or, where it gave none,
    /// to their `symbol-height-offset`s.
    fn set_elevation(&mut self, res: &mut Resources<'_>, geometry: &BucketGeometry, buffer: &Buffer) {
        let elevations = &buffer.elevation_attribute_data;
        if !elevations.is_empty() {
            if let Some(elevation) = place(res, elevations, &mut self.elevation) {
                self.height = Some(VertexBinding::new(&elevation, size_of::<f32>() as u32, ELEVATION));
            }
        } else if self.elevation.take().is_some()
            && let Some(layout_data) = bucket_vertices(geometry, &buffer.attribute_data)
        {
            self.height = Some(VertexBinding::new(&layout_data, LAYOUT_SIZE, HEIGHT_OFFSET));
        }
    }

    /// Sets the dynamic layout vertices, which placement updates: the
    /// layer's own when it has them.
    fn set_projected_pos(&mut self, res: &mut Resources<'_>, buffer: &Buffer, layer_id: &str) {
        let mut placed = self.projected.as_ref().map(|binding| binding.buffer.clone());
        if let Some(projected) = place(res, buffer.dynamic_attribute_data_for(layer_id), &mut placed) {
            let size = size_of::<SymbolDynamicLayoutVertex>() as u32;
            self.projected = Some(VertexBinding::new(&projected, size, PROJECTED_POS));
        }
    }

    /// Sets the opacity vertices, which placement updates.
    fn set_fade_opacity(&mut self, res: &mut Resources<'_>, buffer: &Buffer) {
        let mut placed = self.opacity.as_ref().map(|binding| binding.buffer.clone());
        if let Some(opacity) = place(res, &buffer.opacity_attribute_data, &mut placed) {
            let size = size_of::<SymbolOpacityVertex>() as u32;
            self.opacity = Some(VertexBinding::new(&opacity, size, FADE_OPACITY));
        }
    }
}

/// Puts a copy of `vertices`, which placement updates, into `placed`, the
/// vertices that a draw reads, where they are as many, so that the upload
/// writes them over their buffer, or else into a buffer of their own, which
/// `placed` then holds, and which this returns.
fn place<V: bytemuck::Pod>(
    res: &mut Resources<'_>,
    vertices: &[V],
    placed: &mut Option<VertexBuffer>,
) -> Option<VertexBuffer> {
    if placed.as_ref().is_some_and(|buffer| res.update_vertices(buffer, vertices)) {
        return None;
    }
    let buffer = res.dynamic_vertices(vertices);
    *placed = Some(buffer.clone());
    Some(buffer)
}

/// The attributes of the data-driven paint properties.
fn binder_attributes(
    res: &mut Resources<'_>,
    geometry: &BucketGeometry,
    is_text: bool,
    paint_props: &PaintProperties,
    evaluated: &SymbolPaintProperties::PossiblyEvaluated,
) -> PaintAttributes {
    if is_text {
        let binders = &paint_props.text_binders;
        let buffer = &binders.interleaved_vertex_buffer;
        let shared = paint_vertices(res, geometry, buffer, binders.depends_on_feature_state());
        binders.paint(evaluated, shared.as_ref())
    } else {
        let binders = &paint_props.icon_binders;
        let buffer = &binders.interleaved_vertex_buffer;
        let shared = paint_vertices(res, geometry, buffer, binders.depends_on_feature_state());
        binders.paint(evaluated, shared.as_ref())
    }
}

/// A draw the layer keeps of a tile: the halo or the fill of the text or the
/// icons of one of the bucket's buffers, or of one of its segments.
struct SymbolDraw {
    mesh: Mesh,
    /// Gives the mesh the textures of its tile's atlases before each frame
    /// draws it.
    atlases: AtlasBinding,
    /// The vertices it reads.
    vertices: SymbolVertices,
    data: SymbolDrawData,
    /// Whether placement shows any of the draw's symbols. A draw that shows
    /// none doesn't draw, and doesn't copy the bucket's attributes again
    /// until it shows some.
    visible: bool,
}

/// What tells apart the draws of a tile that read the same attributes: a
/// halo and a fill of one symbol type, and of one segment or all of them.
type AttributesKey = (SymbolType, Option<usize>);

impl SymbolDraw {
    fn attributes_key(&self) -> AttributesKey {
        (self.data.symbol_type, self.data.segment)
    }
}

/// What the layer keeps of a tile: its draws, as they were built for the
/// tile's bucket.
#[derive(Default)]
pub(crate) struct SymbolTile {
    draws: Vec<SymbolDraw>,
}

/// The size of the texture `mesh` binds at `slot`, zero for none.
fn texture_size(res: &Resources<'_>, mesh: &Mesh, slot: usize) -> [f32; 2] {
    mesh.texture(res, slot).map_or([0.0, 0.0], |texture| {
        let size = res.texture_size(texture);
        [size.width as f32, size.height as f32]
    })
}

/// Copies again what placement, sorting or feature state changed of the
/// bucket into the draws of a tile, `draws`, all of which read the same
/// attributes (a halo and a fill): their triangles, and the attributes,
/// which the first of them that is visible copies and the others then read.
fn refresh_draws(
    res: &mut Resources<'_>,
    draws: &mut [SymbolDraw],
    geometry: &BucketGeometry,
    bucket: &SymbolBucket,
    layer_id: &str,
    evaluated: &SymbolPaintProperties::PossiblyEvaluated,
    refresh: BucketChanges,
) {
    let mut source = None;
    for (index, draw) in draws.iter_mut().enumerate() {
        if refresh_draw(res, draw, geometry, bucket, layer_id, evaluated, refresh, source.is_none()) {
            source = Some(index);
        }
    }
    let Some(source) = source else { return };
    let (before, rest) = draws.split_at_mut(source);
    let Some((source, after)) = rest.split_first_mut() else { return };
    for draw in before.iter_mut().chain(after) {
        draw.mesh.set_vertices(res, source.vertices.bindings());
        draw.vertices = source.vertices.clone();
    }
}

/// Copies again what placement, sorting or feature state changed of the
/// bucket into a draw of it: its triangles, and where `copy_attributes`, its
/// attributes. Returns whether it copied the attributes.
#[expect(clippy::too_many_arguments, reason = "the draw and its tile's bucket, what changed of it, and the resources")]
fn refresh_draw(
    res: &mut Resources<'_>,
    draw: &mut SymbolDraw,
    geometry: &BucketGeometry,
    bucket: &SymbolBucket,
    layer_id: &str,
    evaluated: &SymbolPaintProperties::PossiblyEvaluated,
    refresh: BucketChanges,
    copy_attributes: bool,
) -> bool {
    let is_text = draw.data.symbol_type == SymbolType::Text;
    let sdf_icons = draw.data.symbol_type == SymbolType::IconSDF;

    let state = &bucket.state;

    // This property can be set after the initial appearance of the tile, as
    // part of the layout process.
    draw.data.bucket_variable_placement = state.has_variable_placement;

    let buffer = if is_text {
        &state.text
    } else if sdf_icons {
        &state.sdf_icon
    } else {
        &state.icon
    };

    let was_visible = draw.visible;
    if refresh.opacity || refresh.sort {
        draw.visible = buffer.segment_has_visible_vertices(draw.data.segment);
    }
    if !draw.visible {
        return false;
    }
    let refresh = if was_visible { refresh } else { BucketChanges::ALL };

    if !refresh.any() {
        return false;
    }

    if copy_attributes {
        let vertices = &mut draw.vertices;
        if refresh.dynamic {
            vertices.set_projected_pos(res, buffer, layer_id);
        }
        if refresh.opacity {
            vertices.set_fade_opacity(res, buffer);
            vertices.set_elevation(res, geometry, buffer);
        }
        if refresh.paint
            && let Some(paint_props) = bucket.paint_properties.get(layer_id)
            && let Some(paint) = binder_attributes(res, geometry, is_text, paint_props, evaluated).vertices
        {
            vertices.paint = Some(paint);
        }
        draw.mesh.set_vertices(res, vertices.bindings());
    }

    if refresh.sort && !buffer.triangles.is_empty() {
        let segments = match draw.data.segment {
            Some(i) => buffer.segments.get(i..=i).unwrap_or_default(),
            None => &buffer.segments,
        };
        let ranges = segments.iter().map(SegmentBase::range).collect();
        let indexes = res.indexes(&buffer.triangles);
        draw.mesh.replace_indexes(res, indexes, ranges);
    }
    copy_attributes
}

/// One segment, or all the segments, of a buffer of a tile's bucket, which
/// the update builds draws of in the order of [`SegmentGroup::less`].
struct SegmentGroup {
    sort_key: f32,
    type_: SymbolType,
    /// The tile's index among the layer's tiles of the frame.
    tile_index: usize,
    tile_id: UnwrappedTileID,
    overscaled_z: u8,
    /// One segment, for a layer that sorts its features by key, or all
    /// segments of the buffer.
    segment: Option<usize>,
}

impl SegmentGroup {
    fn less(lhs: &SegmentGroup, rhs: &SegmentGroup) -> bool {
        // Sort renderable segments by a sort key.
        if lhs.sort_key < rhs.sort_key {
            return true;
        }

        // In cases when sort key is the same, sort by the type of a segment
        // (text over icons), and for segments of the same type with the same
        // sort key, sort by a tile id.
        if lhs.sort_key == rhs.sort_key {
            if lhs.type_ != SymbolType::Text && rhs.type_ == SymbolType::Text {
                return true;
            }

            if lhs.type_ == rhs.type_ {
                return lhs.tile_id < rhs.tile_id;
            }
        }

        false
    }
}

pub(crate) struct RenderSymbolLayer {
    /// The segments of the tiles the update builds draws of, as it sorts
    /// them, kept from frame to frame for their memory.
    renderable_segments: Vec<SegmentGroup>,

    icon_size: f32,
    text_size: f32,

    /// The passes the layer's draws were built for.
    draws_passes: RenderPass,
}

impl RenderSymbolLayer {
    /// The blocks and the projection of `draw`, of the tile `tile_id` and
    /// its `bucket`, of the layer `layer_id` with the values `layer`, in
    /// `frame`, with the textures of `res`.
    fn uniforms(
        layer_id: &str,
        layer: &EvaluatedLayer<Symbol>,
        tile_id: &OverscaledTileID,
        bucket: &SymbolBucket,
        draw: &SymbolDraw,
        frame: &LayerFrame<'_>,
        res: &Resources<'_>,
    ) -> Option<((SymbolDrawableUBO, SymbolTilePropsUBO), ProjectionUBO)> {
        let state = frame.view;
        let evaluated = &layer.paint;
        let zoom = state.zoom() as f32;
        let paint_properties = bucket.paint_properties.get(layer_id)?;

        let data = &draw.data;
        let tile_id = tile_id.to_unwrapped();
        let is_text = data.symbol_type == SymbolType::Text;
        let screen_space_prop = &layer.style.layout.symbol_screen_space;
        let is_screen_space = screen_space_prop
            .as_constant()
            .copied()
            .unwrap_or_else(SymbolLayoutProperties::Defaults::symbol_screen_space);

        let translate = if is_text { &evaluated.text_translate } else { &evaluated.icon_translate };
        let anchor = if is_text { &evaluated.text_translate_anchor } else { &evaluated.icon_translate_anchor };
        let mut projection: ProjectionData = if is_screen_space {
            Projection::Mercator.projection_data(state, &tile_id, &screen_matrix(*translate.uniform()))
        } else {
            tile_projection(&tile_id, frame, TileTransform { anchor: *anchor.uniform(), ..TileTransform::NONE })
        };
        // The translation stays out of the matrices on every projection: the
        // shader adds it to point anchors, and placement to the positions it
        // lays out (variable anchors, line labels), so it applies once.
        let tile_translation: [f32; 2] = if is_screen_space {
            [0.0, 0.0]
        } else {
            RenderTile::tile_unit_translation(&tile_id, *translate.uniform(), *anchor.uniform(), state)
        };
        projection.translate = glam::DVec2::ZERO;
        let matrix = projection.fallback_matrix;

        let pixels_to_tile_units = tile_id.pixels_to_tile_units(1.0, zoom);
        let pitch_with_map = data.pitch_alignment == AlignmentType::Map;
        let rotate_with_map = data.rotation_alignment == AlignmentType::Map;
        let along_line = data.along_line;
        let has_variable_placement =
            data.bucket_variable_placement && (is_text || data.text_fit != IconTextFitType::None);
        let label_plane_matrix = if along_line || has_variable_placement {
            DMat4::IDENTITY
        } else {
            label_plane_matrix(pitch_with_map, rotate_with_map, state, pixels_to_tile_units)
        };
        let gl_coord_matrix = gl_coord_matrix(pitch_with_map, rotate_with_map, state, pixels_to_tile_units);
        let gamma_scale =
            if pitch_with_map { (state.pitch().cos() * state.camera_to_center_distance()) as f32 } else { 1.0 };

        // Line labels rotate as their placement projects them along their
        // lines (`reproject_line_labels`). Pitched point labels rotate with
        // the label plane matrix's projection. Unpitched point labels need
        // their rotation applied after projection.
        let rotate_in_shader = rotate_with_map && !pitch_with_map && !along_line;

        let size_binder = if is_text { &bucket.text_size_binder } else { &bucket.icon_size_binder };
        let size = size_binder.evaluate_for_zoom(zoom);

        let (fill_color_t, halo_color_t, opacity_t, halo_width_t, halo_blur_t) = if is_text {
            let b = &paint_properties.text_binders;
            (
                b.text_color.interpolation_factor(zoom),
                b.text_halo_color.interpolation_factor(zoom),
                b.text_opacity.interpolation_factor(zoom),
                b.text_halo_width.interpolation_factor(zoom),
                b.text_halo_blur.interpolation_factor(zoom),
            )
        } else {
            let b = &paint_properties.icon_binders;
            (
                b.icon_color.interpolation_factor(zoom),
                b.icon_halo_color.interpolation_factor(zoom),
                b.icon_opacity.interpolation_factor(zoom),
                b.icon_halo_width.interpolation_factor(zoom),
                b.icon_halo_blur.interpolation_factor(zoom),
            )
        };

        let drawable = SymbolDrawableUBO {
            matrix: matrix.as_mat4().to_cols_array(),
            label_plane_matrix: label_plane_matrix.as_mat4().to_cols_array(),
            coord_matrix: gl_coord_matrix.as_mat4().to_cols_array(),
            texsize: texture_size(res, &draw.mesh, TEXTURE_SLOT),
            texsize_icon: texture_size(res, &draw.mesh, TEXTURE_ICON_SLOT),
            is_text_prop: i32::from(is_text),
            rotate_symbol: i32::from(rotate_in_shader),
            pitch_with_map: i32::from(pitch_with_map),
            is_size_zoom_constant: i32::from(size.kind.is_zoom_constant()),
            is_size_feature_constant: i32::from(size.kind.is_feature_constant()),
            is_offset: i32::from(data.is_offset),
            size_t: size.size_t,
            size: size.size,
            fill_color_t,
            halo_color_t,
            opacity_t,
            halo_width_t,
            halo_blur_t,
            is_along_line: i32::from(along_line),
            is_variable_anchor: i32::from(has_variable_placement),
            pitched_scale: state.projection().circle_radius_correction(state) as f32,
            translation: tile_translation,
            gamma_scale,
            is_halo: i32::from(data.is_halo),
        };
        let tile_props =
            SymbolTilePropsUBO { is_text: i32::from(is_text), is_halo: i32::from(data.is_halo), pad1: 0.0, pad2: 0.0 };
        Some(((drawable, tile_props), projection.ubo()))
    }

    /// Pushes into `out` the frame's draws of the layer's draws of `tiles`
    /// in `cx.tiles` that show any symbols, in `passes`, in the order their
    /// meshes were made, which hide behind the ground in front of them
    /// where they draw with a variant that stands on terrain (`stands`).
    fn push_draws(
        cx: &mut PrepareCx<'_, '_, Self>,
        tiles: &[FrameTile<'_, RenderSymbolLayer>],
        passes: RenderPass,
        stands: bool,
        out: &mut Draws,
    ) {
        let layer_id = cx.base.style.base().id.as_str();
        let layer = &*cx.values.evaluated;
        let frame = cx.frame;
        let ground = frame.ground.as_deref().filter(|_| stands);
        let depth = ground.and_then(FrameGround::depth);
        for tile in tiles {
            let tile_id = tile.id();
            let Some(cached) = cx.tiles.get_mut(&tile_id) else {
                continue;
            };
            let bucket = tile.data().map(|bucket| bucket.bucket);
            debug_assert!(bucket.is_some() || cached.draws.is_empty());
            for draw in cached.draws.iter().filter(|draw| draw.visible) {
                let Some((blocks, projection)) =
                    bucket.and_then(|bucket| Self::uniforms(layer_id, layer, &tile_id, bucket, draw, frame, &cx.res))
                else {
                    continue;
                };
                if let Some(depth) = depth {
                    draw.mesh.set_texture(&mut cx.res, GROUND_DEPTH_SLOT, Some(depth));
                }
                let opaque = passes & RenderPass::Opaque != RenderPass::None;
                let frame_draw = Draw { order: draw.mesh.seq(), opaque, ..Draw::DEFAULT };
                draw.atlases.bind(&draw.mesh, &mut cx.res, frame);
                out.main().push_at(&draw.mesh, tile_id, &projection, frame_draw, blocks);
            }
        }

        out.main().block(&props(&layer.paint));
    }
}

/// The block that the frame's draws of a layer of the paint values
/// `evaluated` share.
fn props(evaluated: &SymbolPaintProperties::PossiblyEvaluated) -> SymbolEvaluatedPropsUBO {
    SymbolEvaluatedPropsUBO {
        text_fill_color: evaluated.text_color.uniform().to_float_array(),
        text_halo_color: evaluated.text_halo_color.uniform().to_float_array(),
        text_opacity: *evaluated.text_opacity.uniform(),
        text_halo_width: *evaluated.text_halo_width.uniform(),
        text_halo_blur: *evaluated.text_halo_blur.uniform(),
        pad1: 0.0,
        icon_fill_color: evaluated.icon_color.uniform().to_float_array(),
        icon_halo_color: evaluated.icon_halo_color.uniform().to_float_array(),
        icon_opacity: *evaluated.icon_opacity.uniform(),
        icon_halo_width: *evaluated.icon_halo_width.uniform(),
        icon_halo_blur: *evaluated.icon_halo_blur.uniform(),
        pad2: 0.0,
    }
}

impl Layer for RenderSymbolLayer {
    type Style = Symbol;
    type Input = Features<SymbolLayout>;
    type Tile = SymbolTile;

    /// Symbols draw the lower of two tiles' overlapping symbols over the
    /// higher's.
    const TILE_ORDER: TileOrder = TileOrder::ByY;

    /// Symbols fade out with the tiles they were placed on.
    const FADES_TILES: bool = true;
    const PLACES: bool = true;

    /// Over terrain, the symbols stand on the ground under their anchors,
    /// which placement gives them from the surface the frame drew, and hide
    /// behind the ground in front.
    const TERRAIN: OnTerrain = OnTerrain::Stands { hides_behind_ground: true };

    fn new(_values: &LayerValues<Symbol>) -> Self {
        RenderSymbolLayer {
            renderable_segments: Vec::new(),
            icon_size: 1.0,
            text_size: 16.0,
            draws_passes: RenderPass::None,
        }
    }

    fn evaluate(&mut self, cx: EvaluateCx<'_, Self>) -> Evaluation {
        let parameters = cx.parameters;
        if SymbolLayerPaintPropertyOverrides::has_overrides(&cx.values.style.layout.text_field) {
            mln_style::layers::symbol_layer::set_overrides(
                &cx.values.style.layout,
                &mut cx.evaluated.paint,
                &cx.values.unevaluated,
                &parameters.global_state,
            );
        }
        let evaluated = &cx.evaluated.paint;
        let has_icon_opacity = evaluated.icon_color.constant_or(&SymbolPaintProperties::Defaults::icon_color()).a > 0.0
            || evaluated.icon_halo_color.constant_or(&SymbolPaintProperties::Defaults::icon_halo_color()).a > 0.0;
        let has_text_opacity = evaluated.text_color.constant_or(&SymbolPaintProperties::Defaults::text_color()).a > 0.0
            || evaluated.text_halo_color.constant_or(&SymbolPaintProperties::Defaults::text_halo_color()).a > 0.0;

        if (evaluated.icon_opacity.constant_or(&1.0) > 0.0 && has_icon_opacity && self.icon_size > 0.0)
            || (evaluated.text_opacity.constant_or(&1.0) > 0.0 && has_text_opacity && self.text_size > 0.0)
        {
            Evaluation::TRANSLUCENT
        } else {
            Evaluation::NONE
        }
    }

    /// Pushes the frame's draws of the layer: those of each of its tiles,
    /// which it builds for a tile it keeps none of and refreshes where the
    /// tile's bucket changed. It builds them here rather than in
    /// [`tile`](Layer::tile): their meshes, whose
    /// [`seq`](Mesh::seq) orders the frame's draws, are made in the order of
    /// the segments sorted across the tiles.
    fn prepare(
        &mut self,
        cx: &mut PrepareCx<'_, '_, Self>,
        render_tiles: &[FrameTile<'_, RenderSymbolLayer>],
        out: &mut Draws,
    ) {
        /// The atlases' textures given to one tile's draws.
        #[derive(Default)]
        struct TileAtlases {
            text: Option<AtlasBinding>,
            icon: Option<AtlasBinding>,
        }

        let transform = cx.frame.view;
        let passes = cx.base.passes();
        if passes != self.draws_passes {
            cx.tiles.clear();
            self.draws_passes = passes;
        }

        let layout = &cx.values.style.layout;
        let sort_features_by_key = layout.symbol_sort_key.is_expression();
        let constant_sort_key = layout.symbol_sort_key.is_constant();
        // Screen-space symbols are drawn over the view, not projected onto
        // the map.
        let screen_space_prop = &layout.symbol_screen_space;
        let is_screen_space = screen_space_prop
            .as_constant()
            .copied()
            .unwrap_or_else(SymbolLayoutProperties::Defaults::symbol_screen_space);
        let symbol_projection_variant =
            if is_screen_space { ProjectionVariant::Mercator } else { cx.base.projection_variant };
        let is_offset = !layout.icon_offset.is_undefined();

        let current_zoom = transform.zoom() as f32;

        let mut renderable_segments = std::mem::take(&mut self.renderable_segments);
        renderable_segments.clear();

        for (tile_index, tile) in render_tiles.iter().enumerate() {
            let tile_id = tile.id();

            let Some(mut entry) = cx.tiles.entry(&tile_id) else {
                continue;
            };
            let render_data = tile.data_for_pass(passes);
            let Some(render_data) = render_data else {
                *entry.value = None;
                continue;
            };
            let bucket = render_data.bucket;
            let layer_id = render_data.layer.style.base().id.as_str();

            // What changed since the draws were built or last refreshed.
            let refresh = entry.take_changes();

            debug_assert!(bucket.paint_properties.contains_key(layer_id));
            if !bucket.paint_properties.contains_key(layer_id) {
                continue;
            }

            let evaluated = &render_data.layer.paint;

            // A tile the layer keeps draws of refreshes them.
            if let Some(cached) = entry.value.as_mut().filter(|cached| !cached.draws.is_empty()) {
                // The draws that read the same attributes are next to each
                // other, as the update builds them.
                let (res, geometry) = (&mut cx.res, render_data.geometry());
                for draws in cached.draws.chunk_by_mut(|a, b| a.attributes_key() == b.attributes_key()) {
                    refresh_draws(res, draws, geometry, bucket, layer_id, evaluated, refresh);
                }
                continue;
            }

            let mut serial_key = 1.0_f32;
            let state = &bucket.state;
            // A bucket laid out before a layout change can still have the
            // previous sort-key expression until it is rebuilt.
            let sort_segments =
                sort_features_by_key || (constant_sort_key && !bucket.layout.symbol_sort_key.is_constant());
            let mut add_renderables = |buffer: &Buffer, type_: SymbolType| {
                if sort_segments {
                    // Features need to be rendered in a specific order, so we
                    // add each segment individually
                    for (i, segment) in buffer.segments.iter().enumerate() {
                        renderable_segments.push(SegmentGroup {
                            sort_key: segment.sort_key,
                            type_,
                            tile_index,
                            tile_id: tile.render_tile().id(),
                            overscaled_z: tile_id.overscaled_z,
                            segment: Some(i),
                        });
                    }
                } else if !buffer.segments.is_empty() {
                    // Features can be rendered in the order produced, and as
                    // grouped by the bucket. With an explicit constant key,
                    // the segment's key keeps text above icons across tiles.
                    let sort_key = if constant_sort_key { buffer.segments[0].sort_key } else { serial_key };
                    renderable_segments.push(SegmentGroup {
                        sort_key,
                        type_,
                        tile_index,
                        tile_id: tile.render_tile().id(),
                        overscaled_z: tile_id.overscaled_z,
                        segment: None,
                    });
                    serial_key += 1.0;
                }
            };

            let atlases = render_data.atlas_textures();
            let has_icon_texture = atlases.icon().is_some();
            let has_glyph_texture = atlases.glyph().is_some();

            if !state.icon.segments.is_empty() && has_icon_texture {
                add_renderables(&state.icon, SymbolType::IconRGBA);
            }
            if !state.sdf_icon.segments.is_empty() && has_icon_texture {
                add_renderables(&state.sdf_icon, SymbolType::IconSDF);
            }
            if !state.text.segments.is_empty() && has_glyph_texture {
                add_renderables(&state.text, SymbolType::Text);
            }
        }

        // A stable sort keeps segments of the same order in the order they
        // were added.
        renderable_segments.sort_by(|a, b| {
            if SegmentGroup::less(a, b) {
                std::cmp::Ordering::Less
            } else if SegmentGroup::less(b, a) {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        });

        // Renderables come across tiles, out of order, so the atlases' textures
        // given to a tile's draws are kept by tile.
        let mut tile_atlases: HashMap<UnwrappedTileID, TileAtlases> = HashMap::default();
        let color_mode = if (passes.underlying_type() & RenderPass::Translucent.underlying_type()) != 0 {
            ColorMode::alpha_blended()
        } else {
            ColorMode::unblended()
        };

        for group in &renderable_segments {
            let is_text = group.type_ == SymbolType::Text;
            let sdf_icons = group.type_ == SymbolType::IconSDF;

            let Some(tile) = render_tiles.get(group.tile_index) else {
                continue;
            };
            let tile_id = tile.render_tile().id().overscale_to(group.overscaled_z);
            let Some(render_data) = tile.data_for_pass(passes) else {
                continue;
            };
            let bucket = render_data.bucket;
            let state = &bucket.state;
            let buffer = if is_text {
                &state.text
            } else if sdf_icons {
                &state.sdf_icon
            } else {
                &state.icon
            };

            if buffer.triangles.is_empty() {
                continue;
            }
            let visible = buffer.segment_has_visible_vertices(group.segment);

            let evaluated = &render_data.layer.paint;
            let layer_id = render_data.layer.style.base().id.as_str();
            let Some(bucket_paint_properties) = bucket.paint_properties.get(layer_id) else {
                continue;
            };

            let bucket_layout = &*bucket.layout;
            let values =
                if is_text { text_property_values(bucket_layout) } else { icon_property_values(bucket_layout) };

            let atlases = render_data.atlas_textures();

            let (vertices, uniforms) = SymbolVertices::new(
                &mut cx.res,
                render_data.geometry(),
                buffer,
                layer_id,
                is_text,
                bucket_paint_properties,
                evaluated,
            );

            let text_halo = evaluated.text_halo_color.constant_or(&Color::black()).a > 0.0
                && evaluated.text_halo_width.constant_or(&1.0) != 0.0;
            let text_fill = evaluated.text_color.constant_or(&Color::black()).a > 0.0;

            let icon_halo = evaluated.icon_halo_color.constant_or(&Color::black()).a > 0.0
                && evaluated.icon_halo_width.constant_or(&1.0) != 0.0;
            let icon_fill = evaluated.icon_color.constant_or(&Color::black()).a > 0.0;

            // The programs, halo first, with whether each is a halo's and
            // draws.
            let halo_and_fill = if is_text {
                let program =
                    if bucket.icons_in_text { &programs::SYMBOL_TEXT_AND_ICON } else { &programs::SYMBOL_SDF };
                vec![(program, true, text_halo), (program, false, text_fill)]
            } else if sdf_icons {
                vec![(&programs::SYMBOL_SDF, true, icon_halo), (&programs::SYMBOL_SDF, false, icon_fill)]
            } else {
                vec![(&programs::SYMBOL_ICON, false, true)]
            };

            let segments = match group.segment {
                Some(i) => std::slice::from_ref(&buffer.segments[i]),
                None => &buffer.segments,
            };
            for (program, is_halo, _) in halo_and_fill.into_iter().filter(|(_, _, drawn)| *drawn) {
                // The draws of a tile share the atlases' textures.
                let tile_atlases = tile_atlases.entry(tile.render_tile().id()).or_default();
                let draw_atlases = if is_text {
                    tile_atlases.text.get_or_insert_with(|| {
                        let text_size_is_zoom_constant =
                            bucket.text_size_binder.evaluate_for_zoom(current_zoom).kind.is_zoom_constant();
                        AtlasBinding::new(
                            atlases,
                            Some(TEXTURE_ICON_SLOT),
                            Some(TEXTURE_SLOT),
                            is_text,
                            false,
                            values.rotation_alignment,
                            false,
                            text_size_is_zoom_constant,
                        )
                    })
                } else {
                    tile_atlases.icon.get_or_insert_with(|| {
                        let icon_scaled = bucket_layout.icon_size.constant_or(&1.0) != 1.0 || bucket.icons_need_linear;
                        AtlasBinding::new(
                            atlases,
                            Some(TEXTURE_ICON_SLOT),
                            Some(TEXTURE_SLOT),
                            is_text,
                            sdf_icons,
                            values.rotation_alignment,
                            icon_scaled,
                            false,
                        )
                    })
                };

                // The halo and the fill read the same attributes.
                let res = &mut cx.res;
                let shader = res.shader_key(program, uniforms, symbol_projection_variant);
                let textures = draw_atlases.textures(&shader);
                let indexes = res.indexes(&buffer.triangles);
                let mesh = res.mesh(MeshDesc {
                    shader,
                    color_mode,
                    cull_face_mode: CullFaceMode::disabled(),
                    mode: DrawMode::Triangles(),
                    vertices: vertices.bindings().cloned().collect(),
                    vertex_count: buffer.attribute_data.len(),
                    instances: None,
                    indexes,
                    ranges: segments.iter().map(SegmentBase::range).collect(),
                    textures,
                });

                let draw = SymbolDraw {
                    mesh,
                    atlases: draw_atlases.clone(),
                    vertices: vertices.clone(),
                    data: SymbolDrawData {
                        is_halo,
                        bucket_variable_placement: state.has_variable_placement,
                        symbol_type: group.type_,
                        pitch_alignment: values.pitch_alignment,
                        rotation_alignment: values.rotation_alignment,
                        along_line: values.along_line,
                        text_fit: *bucket_layout.icon_text_fit.uniform(),
                        is_offset,
                        segment: group.segment,
                    },
                    visible,
                };
                if let Some(entry) = cx.tiles.entry(&tile_id) {
                    entry.value.get_or_insert_with(SymbolTile::default).draws.push(draw);
                }
            }
        }

        self.renderable_segments = renderable_segments;
        Self::push_draws(cx, render_tiles, passes, symbol_projection_variant.stands(), out);
    }
}
