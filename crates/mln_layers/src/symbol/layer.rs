use std::cmp::Ordering;

use glam::DMat4;
use mln_gfx::color_mode::ColorMode;
use mln_gfx::draw_mode::DrawMode;
use mln_gfx::shaders::program::Program;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_render::draw::{Buffer, Depth, Draw, Draws, IndexBuffer, Mesh, MeshDesc, Resources, Sampler};
use mln_render::layer::{
    EvaluateCx, Evaluation, Features, FrameTile, Layer, LayerBucket, NewCx, PrepareCx, TileProjection, TileTransform,
    UpdateCx,
};
use mln_style::StyleValue as _;
use mln_style::evaluated::Evaluated;
use mln_style_types::{AlignmentType, IconTextFitType, SymbolPlacementType, TextRotationAlignmentType};
use mln_tile_id::UnwrappedTileID;
use mln_util::Color;

use super::bucket::{Buffer as SymbolBuffer, CollisionBuffer, SymbolBucket, SymbolDynamicLayoutVertex};
use super::format_section_override::{SymbolLayerPaintPropertyOverrides, set_overrides};
use super::layout::SymbolLayout;
use super::programs::{COLLISION_BOX, COLLISION_CIRCLE, SYMBOL_ICON, SYMBOL_SDF, SYMBOL_TEXT_AND_ICON};
use super::projection::{gl_coord_matrix, label_plane_matrix};
use super::ubo::{
    CollisionDrawableUBO, CollisionTilePropsUBO, SymbolDrawableUBO, SymbolEvaluatedPropsUBO, SymbolTilePropsUBO,
};
use super::{Symbol, SymbolLayoutProperties, SymbolPaintProperties};

/// A symbol layer: the draws of its tiles' text and icons where placement
/// shows them and, over them where the map's debug options show them, the
/// boxes and circles they collide with.
pub struct SymbolLayer {
    /// How the icon atlas samples in the frame where an image drawn at its
    /// own size may stay sharp ([`IconFilter::NearestWhenStill`]).
    still: Sampler,
}

/// What the layer keeps of a tile: the draws of its bucket's text and
/// icons, and the meshes of its collision boxes and circles.
pub struct SymbolTile {
    draws: Vec<SymbolDraw>,
    collision: Vec<CollisionMesh>,
}

/// A draw of a tile's text or icons: their halo or their fill, of one of
/// the buffer's segments or of all of them.
struct SymbolDraw {
    mesh: Mesh<(SymbolDrawableUBO, SymbolTilePropsUBO)>,
    /// Where the draw is among the layer's: the draws of a tile draw over
    /// those of the tiles made before it, and in the order of their
    /// segments among those made with it.
    order: i64,
    /// The positions the mesh reads: those of the bucket's leader, or the
    /// layer's own once placement moves them for it.
    positions: Buffer<SymbolDynamicLayoutVertex>,
    kind: Kind,
    halo: bool,
    segment: Option<usize>,
    /// How the mesh samples the tile's icon atlas, if it does.
    icon_sampler: Option<Sampler>,
}

/// A mesh of a tile's collision boxes or circles, of its text or its icons.
struct CollisionMesh {
    mesh: Mesh<(CollisionDrawableUBO, CollisionTilePropsUBO)>,
    text: bool,
}

/// What a draw draws: one of a bucket's buffers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Text,
    Icon,
    SdfIcon,
}

impl Kind {
    fn buffer(self, bucket: &SymbolBucket) -> &SymbolBuffer {
        match self {
            Kind::Text => &bucket.state.text,
            Kind::Icon => &bucket.state.icon,
            Kind::SdfIcon => &bucket.state.sdf_icon,
        }
    }
}

/// How a draw filters the tile's icon atlas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IconFilter {
    Linear,
    /// Linearly while the camera moves or the images don't lie flat on the
    /// screen: the view is pitched, or they turn with the map. Otherwise
    /// nearest, so that an image drawn at its own size stays sharp.
    NearestWhenStill {
        map_aligned: bool,
    },
}

impl IconFilter {
    /// The sampler of the icon atlas, which is `still` where an image may
    /// stay sharp.
    fn sampler(self, still: &Sampler) -> Sampler {
        match self {
            IconFilter::NearestWhenStill { map_aligned: false } => still.clone(),
            IconFilter::Linear | IconFilter::NearestWhenStill { map_aligned: true } => GLYPHS.into(),
        }
    }
}

/// How a draw samples the tile's glyph atlas.
const GLYPHS: SamplerState = SamplerState {
    filter: TextureFilterType::Linear,
    wrap_u: TextureWrapType::Clamp,
    wrap_v: TextureWrapType::Clamp,
    max_anisotropy: 1,
    mipmapped: false,
};

/// How a buffer's quads face the camera.
struct Alignment {
    pitch_with_map: bool,
    rotate_with_map: bool,
    /// Whether each glyph or icon is placed along its line.
    along_line: bool,
}

impl Alignment {
    fn of(kind: Kind, layout: &SymbolLayoutProperties::PossiblyEvaluated) -> Alignment {
        let line = *layout.symbol_placement.uniform() != SymbolPlacementType::Point;
        if kind == Kind::Text {
            let rotation = *layout.text_rotation_alignment.uniform();
            return Alignment {
                pitch_with_map: *layout.text_pitch_alignment.uniform() == AlignmentType::Map,
                rotate_with_map: rotation == TextRotationAlignmentType::Map,
                along_line: rotation != TextRotationAlignmentType::Viewport && line,
            };
        }
        let rotate_with_map = *layout.icon_rotation_alignment.uniform() == AlignmentType::Map;
        Alignment {
            pitch_with_map: *layout.icon_pitch_alignment.uniform() == AlignmentType::Map,
            rotate_with_map,
            along_line: rotate_with_map && line,
        }
    }
}

/// One segment, or all the segments, of a buffer of a tile's bucket, of
/// which the update makes draws in the order of [`SegmentGroup::order`].
struct SegmentGroup {
    sort_key: f32,
    kind: Kind,
    tile: usize,
    tile_id: UnwrappedTileID,
    segment: Option<usize>,
}

impl SegmentGroup {
    /// By sort key, then icons before text, then by tile; others in the
    /// order they were added.
    fn order(a: &SegmentGroup, b: &SegmentGroup) -> Ordering {
        let less = |a: &SegmentGroup, b: &SegmentGroup| {
            a.sort_key < b.sort_key
                || (a.sort_key == b.sort_key
                    && ((a.kind != Kind::Text && b.kind == Kind::Text) || (a.kind == b.kind && a.tile_id < b.tile_id)))
        };
        if less(a, b) {
            Ordering::Less
        } else if less(b, a) {
            Ordering::Greater
        } else {
            Ordering::Equal
        }
    }
}

impl Layer for SymbolLayer {
    type Style = Symbol;
    type Input = Features<SymbolLayout>;
    type Tile = SymbolTile;

    fn new(_cx: &mut NewCx<'_, Self>) -> Self {
        SymbolLayer { still: Sampler::new(GLYPHS) }
    }

    /// The text of a `text-field` with format sections takes the values its
    /// sections override.
    fn evaluate(&mut self, cx: EvaluateCx<'_, Self>) -> Evaluation {
        let layer = cx.layer;
        if SymbolLayerPaintPropertyOverrides::has_overrides(&layer.style.layout.text_field) {
            let global_state = cx.parameters.global_state.as_ref();
            set_overrides(&layer.style.layout, &mut cx.evaluated.paint, &layer.unevaluated, global_state);
        }
        let paint = &cx.evaluated.paint;
        let colored = |color: &Evaluated<Color>, default: Color| color.constant_or(&default).a > 0.0;
        let icon = colored(&paint.icon_color, SymbolPaintProperties::Defaults::icon_color())
            || colored(&paint.icon_halo_color, SymbolPaintProperties::Defaults::icon_halo_color());
        let text = colored(&paint.text_color, SymbolPaintProperties::Defaults::text_color())
            || colored(&paint.text_halo_color, SymbolPaintProperties::Defaults::text_halo_color());
        Evaluation::translucent_if(
            (paint.icon_opacity.constant_or(&1.0) > 0.0 && icon)
                || (paint.text_opacity.constant_or(&1.0) > 0.0 && text),
        )
    }

    /// Makes the draws of the tiles that have none, of all of them in the
    /// order of their segments, which they then draw in, over the draws
    /// made before. A kept draw reads the positions its layer now has of its
    /// own, and filters the icon atlas as the frame's camera asks.
    fn update(&mut self, cx: &mut UpdateCx<'_, Self>) {
        self.still.set(SamplerState { filter: cx.frame.image_filter(), ..GLYPHS });
        let style = &cx.layer.style;
        let screen_space = screen_space(&style.layout);
        let sort_by_key = style.layout.symbol_sort_key.is_expression();
        let constant_key = style.layout.symbol_sort_key.is_constant();
        let mut groups = Vec::new();
        for (index, (tile, laid, value)) in cx.tiles.iter_mut().enumerate() {
            let Some(laid) = laid.filter(|laid| laid.bucket.has_data()) else { continue };
            if let Some(kept) = value.as_mut().filter(|kept| !kept.draws.is_empty()) {
                for draw in &mut kept.draws {
                    let buffer = draw.kind.buffer(laid.bucket);
                    let positions = laid.buffer(buffer.dynamic_attribute_data_for(&style.id));
                    if *positions != draw.positions {
                        let parts = MeshParts::new(laid, draw.kind, draw.segment, screen_space);
                        draw.mesh = cx.res.mesh(&parts.desc(draw.icon_sampler.as_ref()));
                        draw.positions = positions.clone();
                    }
                }
                continue;
            }
            if value.is_none() {
                *value = Some(SymbolTile { draws: Vec::new(), collision: collision_meshes(cx.res, laid) });
            }
            let bucket = laid.bucket;
            // A bucket laid out before a layout change can still have the
            // previous sort-key expression until it is laid out again.
            let by_segment = sort_by_key || (constant_key && !bucket.layout.symbol_sort_key.is_constant());
            let mut serial_key = 1.0;
            let atlases =
                [(Kind::Icon, laid.icon_atlas()), (Kind::SdfIcon, laid.icon_atlas()), (Kind::Text, laid.glyph_atlas())];
            for (kind, atlas) in atlases {
                let triangles = &kind.buffer(bucket).triangles;
                if triangles.is_empty() || atlas.is_none() {
                    continue;
                }
                let group = |sort_key, segment| SegmentGroup {
                    sort_key,
                    kind,
                    tile: index,
                    tile_id: tile.unwrapped_id(),
                    segment,
                };
                if by_segment {
                    let segments = triangles.segments().iter().enumerate();
                    groups.extend(segments.map(|(i, segment)| group(segment.sort_key, Some(i))));
                } else {
                    // With a constant key, the segments' key keeps text over
                    // icons across tiles.
                    groups.push(group(if constant_key { triangles.segments()[0].sort_key } else { serial_key }, None));
                    serial_key += 1.0;
                }
            }
        }
        groups.sort_by(SegmentGroup::order);

        // The text draws of a tile's place made in one frame filter its icon
        // atlas as the first of them does, and so do its icon draws.
        let mut first_filters: Vec<(UnwrappedTileID, bool, IconFilter)> = Vec::new();
        let mut first_filter = |tile, text, own| {
            if let Some(&(_, _, first)) = first_filters.iter().find(|&&(at, of, _)| at == tile && of == text) {
                return first;
            }
            first_filters.push((tile, text, own));
            own
        };
        let zoom = cx.frame.camera.zoom() as f32;
        for group in &groups {
            let (_, laid, value) = cx.tiles.get_mut(group.tile);
            let (Some(laid), Some(value)) = (laid, value.as_mut()) else { continue };
            let (bucket, paint) = (laid.bucket, &laid.laid_with.paint);
            let sharp =
                IconFilter::NearestWhenStill { map_aligned: Alignment::of(group.kind, &bucket.layout).rotate_with_map };
            let colored = |color: &Evaluated<Color>| color.constant_or(&Color::black()).a > 0.0;
            let (halo, fill, own) = match group.kind {
                Kind::Text => {
                    let halo = colored(&paint.text_halo_color) && paint.text_halo_width.constant_or(&1.0) != 0.0;
                    // Text whose size changes with the zoom draws scaled.
                    let constant = bucket.text_size_binder.evaluate_for_zoom(zoom).is_zoom_constant;
                    (halo, colored(&paint.text_color), if constant { sharp } else { IconFilter::Linear })
                }
                kind => {
                    let scaled = bucket.layout.icon_size.constant_or(&1.0) != 1.0 || bucket.flags.icons_need_linear;
                    let own = if kind == Kind::SdfIcon || scaled { IconFilter::Linear } else { sharp };
                    if kind == Kind::Icon {
                        (false, true, own)
                    } else {
                        let halo = colored(&paint.icon_halo_color) && paint.icon_halo_width.constant_or(&1.0) != 0.0;
                        (halo, colored(&paint.icon_color), own)
                    }
                }
            };
            if !halo && !fill {
                continue;
            }
            let filter = first_filter(group.tile_id, group.kind == Kind::Text, own);
            let parts = MeshParts::new(laid, group.kind, group.segment, screen_space);
            let icon_sampler = parts.icon_slot.map(|_| filter.sampler(&self.still));
            for (is_halo, _) in [(true, halo), (false, fill)].into_iter().filter(|&(_, drawn)| drawn) {
                let mesh = cx.res.mesh(&parts.desc(icon_sampler.as_ref()));
                value.draws.push(SymbolDraw {
                    order: mesh.seq() as i64,
                    mesh,
                    positions: parts.positions.clone(),
                    kind: group.kind,
                    halo: is_halo,
                    segment: group.segment,
                    icon_sampler: icon_sampler.clone(),
                });
            }
        }
    }

    fn prepare(&self, cx: &PrepareCx<'_, Self>, out: &mut Draws) {
        let paint = &cx.layer.evaluated.paint;
        let main = out.main();
        main.block(&SymbolEvaluatedPropsUBO {
            text_fill_color: paint.text_color.uniform().to_float_array(),
            text_halo_color: paint.text_halo_color.uniform().to_float_array(),
            text_opacity: *paint.text_opacity.uniform(),
            text_halo_width: *paint.text_halo_width.uniform(),
            text_halo_blur: *paint.text_halo_blur.uniform(),
            icon_fill_color: paint.icon_color.uniform().to_float_array(),
            icon_halo_color: paint.icon_halo_color.uniform().to_float_array(),
            icon_opacity: *paint.icon_opacity.uniform(),
            icon_halo_width: *paint.icon_halo_width.uniform(),
            icon_halo_blur: *paint.icon_halo_blur.uniform(),
            fade_change: cx.frame.placement_fade,
            ..SymbolEvaluatedPropsUBO::default()
        });
        for (tile, laid, value) in cx.tiles() {
            for draw in &value.draws {
                if draw.kind.buffer(laid.bucket).segment_has_visible_vertices(draw.segment) {
                    let (at, blocks) = draw.blocks(cx, tile, laid);
                    main.push_at(&draw.mesh, &at, Draw { order: draw.order, ..Draw::DEFAULT }, blocks);
                }
            }
        }
        // A tile's collision boxes and circles draw over those of the tiles
        // before it.
        let zoom = cx.frame.camera.zoom();
        for (tile, _, value) in cx.tiles() {
            for collision in &value.collision {
                let at = tile.project(cx.frame, &transform(paint, collision.text));
                let id = tile.id();
                let scale = at.pixels_to_tile_units * 2f64.powf(zoom - f64::from(id.overscaled_z)) as f32;
                let tile_props = CollisionTilePropsUBO {
                    extrude_scale: cx.frame.pixels_to_clip.map(|pixels| pixels / scale),
                    overscale_factor: id.overscale_factor() as f32,
                    ..CollisionTilePropsUBO::default()
                };
                let draw = Draw { order: tile.index() as i64, depth: Depth::Off, ..Draw::DEFAULT };
                let blocks = (CollisionDrawableUBO::default(), tile_props);
                out.overlay().push_at(&collision.mesh, &at, draw, blocks);
            }
        }
    }
}

/// Whether the layer's symbols draw over the viewport rather than on the
/// map, by its `symbol-screen-space`.
fn screen_space(layout: &SymbolLayoutProperties::Unevaluated) -> bool {
    layout
        .symbol_screen_space
        .as_constant()
        .copied()
        .unwrap_or_else(SymbolLayoutProperties::Defaults::symbol_screen_space)
}

/// How the layer's `paint` places a tile's text, or its icons: moved by
/// their translate.
fn transform(paint: &SymbolPaintProperties::PossiblyEvaluated, text: bool) -> TileTransform {
    if text {
        TileTransform::translated(*paint.text_translate.uniform(), *paint.text_translate_anchor.uniform())
    } else {
        TileTransform::translated(*paint.icon_translate.uniform(), *paint.icon_translate_anchor.uniform())
    }
}

/// What a mesh of a draw of `kind` of a tile's bucket is made of.
struct MeshParts<'a> {
    program: &'static Program,
    laid: LayerBucket<'a, Symbol, SymbolBucket>,
    kind: Kind,
    /// The indexes of the segment drawn, or of all of them.
    indexes: IndexBuffer,
    positions: &'a Buffer<SymbolDynamicLayoutVertex>,
    /// Where the program samples the tile's icon atlas, if it does.
    icon_slot: Option<usize>,
    screen_space: bool,
}

impl<'a> MeshParts<'a> {
    fn new(
        laid: LayerBucket<'a, Symbol, SymbolBucket>,
        kind: Kind,
        segment: Option<usize>,
        screen_space: bool,
    ) -> Self {
        let buffer = kind.buffer(laid.bucket);
        let program = match kind {
            Kind::Text if laid.bucket.flags.icons_in_text => &SYMBOL_TEXT_AND_ICON,
            Kind::Text | Kind::SdfIcon => &SYMBOL_SDF,
            Kind::Icon => &SYMBOL_ICON,
        };
        let indexes = laid.indexes(&buffer.triangles);
        MeshParts {
            program,
            laid,
            kind,
            indexes: segment.map_or_else(|| indexes.clone(), |segment| indexes.segment(segment)),
            positions: laid.buffer(buffer.dynamic_attribute_data_for(&laid.laid_with.style.id)),
            icon_slot: match kind {
                Kind::Text => (program.textures.len() > 1).then_some(1),
                Kind::Icon | Kind::SdfIcon => Some(0),
            },
            screen_space,
        }
    }

    /// The mesh, which samples the icon atlas with `icon_sampler`.
    fn desc(&self, icon_sampler: Option<&Sampler>) -> MeshDesc<'_> {
        let (laid, buffer) = (self.laid, self.kind.buffer(self.laid.bucket));
        let mut desc = MeshDesc::new(self.program, &self.indexes);
        desc.vertices(laid.buffer(&buffer.attribute_data))
            .vertices(self.positions)
            .vertices(laid.buffer(&buffer.opacity_attribute_data));
        let icon_sampler = icon_sampler.map_or_else(|| GLYPHS.into(), Sampler::clone);
        if self.kind == Kind::Text {
            desc.paint(laid.binders(&laid.bucket.text_binders));
            if let Some(glyphs) = laid.glyph_atlas() {
                desc.texture(glyphs, GLYPHS);
            }
            if self.icon_slot.is_some()
                && let Some(icons) = laid.icon_atlas()
            {
                desc.texture(icons, &icon_sampler);
            }
        } else {
            desc.paint(laid.binders(&laid.bucket.icon_binders));
            if let Some(icons) = laid.icon_atlas() {
                desc.texture(icons, icon_sampler);
            }
        }
        if self.screen_space {
            desc.screen_space();
        }
        desc
    }
}

impl SymbolDraw {
    /// Where the draw draws `tile`, of `laid`, and its blocks.
    fn blocks(
        &self,
        cx: &PrepareCx<'_, SymbolLayer>,
        tile: FrameTile,
        laid: LayerBucket<'_, Symbol, SymbolBucket>,
    ) -> (TileProjection, (SymbolDrawableUBO, SymbolTilePropsUBO)) {
        let (camera, bucket, paint) = (cx.frame.camera, laid.bucket, &cx.layer.evaluated.paint);
        let layout = &cx.layer.style.layout;
        let screen_space = screen_space(layout);
        let is_text = self.kind == Kind::Text;
        let how = transform(paint, is_text);
        // The translation stays out of the matrices on every projection: the
        // shader adds it to point anchors, and placement to the positions it
        // lays out (variable anchors, line labels), so it applies once. A
        // screen-space symbol takes it in its screen matrix.
        let (at, translation) = if screen_space {
            (tile.project(cx.frame, &TileTransform { screen_space: true, ..how }), [0.0, 0.0])
        } else {
            (tile.project(cx.frame, &TileTransform::default()), how.tile_units(&tile.unwrapped_id(), camera))
        };
        let alignment = Alignment::of(self.kind, &bucket.layout);
        let (pitch_with_map, rotate_with_map, along_line) =
            (alignment.pitch_with_map, alignment.rotate_with_map, alignment.along_line);
        let text_fit = *bucket.layout.icon_text_fit.uniform();
        let variable_anchor = bucket.state.has_variable_placement && (is_text || text_fit != IconTextFitType::None);
        let pixels_to_tile_units = at.pixels_to_tile_units;
        let label_plane = if along_line || variable_anchor {
            DMat4::IDENTITY
        } else {
            label_plane_matrix(pitch_with_map, rotate_with_map, camera, pixels_to_tile_units)
        };
        let gl_coord = gl_coord_matrix(pitch_with_map, rotate_with_map, camera, pixels_to_tile_units);
        let size_binder = if is_text { &bucket.text_size_binder } else { &bucket.icon_size_binder };
        let size = size_binder.evaluate_for_zoom(camera.zoom() as f32);
        let texture_size = |slot: usize| {
            let texture = match (self.kind, slot) {
                (Kind::Text, 0) => laid.glyph_atlas(),
                (Kind::Text, _) => laid.icon_atlas().filter(|_| self.icon_sampler.is_some()),
                (_, 0) => laid.icon_atlas(),
                _ => None,
            };
            texture.map_or([0.0, 0.0], |texture| {
                let size = cx.res.size(texture);
                [size.width as f32, size.height as f32]
            })
        };
        let drawable = SymbolDrawableUBO {
            label_plane_matrix: label_plane.as_mat4().to_cols_array(),
            coord_matrix: gl_coord.as_mat4().to_cols_array(),
            texsize: texture_size(0),
            texsize_icon: texture_size(1),
            is_text_prop: is_text.into(),
            // Line labels rotate as their placement projects them along their
            // lines, and pitched point labels with the label plane matrix's
            // projection; unpitched point labels rotate after projection.
            rotate_symbol: (rotate_with_map && !pitch_with_map && !along_line).into(),
            pitch_with_map: pitch_with_map.into(),
            is_size_zoom_constant: size.is_zoom_constant.into(),
            is_size_feature_constant: size.is_feature_constant.into(),
            is_offset: (!layout.icon_offset.is_undefined()).into(),
            size_t: size.size_t,
            size: size.size,
            is_along_line: along_line.into(),
            is_variable_anchor: variable_anchor.into(),
            pitched_scale: camera.projection().circle_radius_correction(camera) as f32,
            translation,
            gamma_scale: if pitch_with_map {
                (camera.pitch().cos() * camera.camera_to_center_distance()) as f32
            } else {
                1.0
            },
            is_halo_pass: self.halo.into(),
            ..SymbolDrawableUBO::default()
        };
        let tile_props =
            SymbolTilePropsUBO { is_text: is_text.into(), is_halo: self.halo.into(), ..Default::default() };
        (at, (drawable, tile_props))
    }
}

/// The meshes of the collision boxes and circles of `laid`'s bucket, of its
/// icons, then of its text.
fn collision_meshes(res: &mut Resources, laid: LayerBucket<'_, Symbol, SymbolBucket>) -> Vec<CollisionMesh> {
    let state = &laid.bucket.state;
    let buffers = [
        (&state.icon_collision_box, false, &COLLISION_BOX, DrawMode::Lines { width: 1.0 }),
        (&state.icon_collision_circle, false, &COLLISION_CIRCLE, DrawMode::Triangles),
        (&state.text_collision_box, true, &COLLISION_BOX, DrawMode::Lines { width: 1.0 }),
        (&state.text_collision_circle, true, &COLLISION_CIRCLE, DrawMode::Triangles),
    ];
    let mut meshes = Vec::new();
    for (buffer, text, program, primitive) in buffers {
        let Some(buffer) = buffer.as_ref().filter(|buffer: &&CollisionBuffer| buffer.has_data()) else { continue };
        let mut desc = MeshDesc::new(program, laid.indexes(&buffer.indices));
        desc.vertices(laid.buffer(&buffer.vertices))
            .vertices(laid.buffer(&buffer.dynamic_vertices))
            .primitive(primitive)
            .blend(ColorMode::alpha_blended());
        meshes.push(CollisionMesh { mesh: res.mesh(&desc), text });
    }
    meshes
}
