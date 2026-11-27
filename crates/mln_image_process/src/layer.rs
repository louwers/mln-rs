//! The render half: the passes of an image-process layer's frame.

use mln_engine::draw::{
    Depth, Draw, DrawGeometry, DrawIndexes, Draws, Mesh, RenderStaticData, SegmentBase, SharedTexture, Target,
    Textures, Uniforms, raw_vertices,
};
use mln_engine::layer::{FrameTile, Layer, LayerValues, NoTiles, PrepareCx};
use mln_engine::shaders::programs::POS;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::types::TextureChannelDataType;
use mln_gfx::{ColorMode, CullFaceMode, DrawMode, Rgba, Size};

use crate::ImageProcess;
use crate::members::{OPERATIONS, SOURCE, TargetName};
use crate::plan::plan;
use crate::program::IMAGE_PROCESS;

/// How a pass draws: over what it draws into, without a depth test.
const DRAW: Draw = Draw { depth: Depth::Off, ..Draw::DEFAULT };

/// An `org.maplibre.image-process` layer: the scratch targets of its passes
/// but the last, which draws in the layer's place, and the meshes of the
/// passes.
pub(crate) struct ImageProcessLayer {
    scratch: Vec<Target>,
    passes: Vec<PassMesh>,
}

/// A pass's quad over what it draws into, with the textures it samples.
struct PassMesh {
    textures: [SharedTexture; 2],
    /// Whether it blends over what it draws into, as the last pass does.
    blends: bool,
    mesh: Mesh,
}

impl Layer for ImageProcessLayer {
    type Style = ImageProcess;
    type Input = NoTiles;
    type Tile = ();

    fn new(_values: &LayerValues<ImageProcess>) -> Self {
        ImageProcessLayer { scratch: Vec::new(), passes: Vec::new() }
    }

    /// Draws each pass but the last into a scratch target, from the source
    /// target or the pass before, and the last in the layer's place. A layer
    /// without a source target, or that reads a target the frame doesn't
    /// have, draws nothing.
    fn prepare(&mut self, cx: &mut PrepareCx<'_, '_, Self>, _tiles: &[FrameTile<'_, Self>], out: &mut Draws) {
        let values = cx.values;
        let members = values.style.members();
        let Some(TargetName(source)) = members.get(&SOURCE) else { return };
        let operations = members.get(&OPERATIONS).map_or(&[][..], |operations| &operations.0[..]);
        let frame = cx.frame.size();
        let ratio = cx.frame.pixel_ratio();
        let logical = |device: u32| ((device as f32 / ratio).ceil() as u32).max(1);
        let logical = Size::new(logical(frame.width), logical(frame.height));
        let passes = plan(operations, cx.frame.view().zoom() as f32, frame, logical);
        let Some(mut input) = cx.read_target(source) else { return };
        let others: Option<Vec<_>> = passes
            .iter()
            .map(|pass| match pass.other.as_deref() {
                Some(name) => cx.read_target(name).map(Some),
                None => Some(None),
            })
            .collect();
        let Some(others) = others else { return };

        let scratch = passes.len() - 1;
        self.scratch.truncate(scratch);
        self.passes.truncate(passes.len());
        for (index, (pass, other)) in passes.iter().zip(others).enumerate() {
            let other = other.unwrap_or_else(|| input.clone());
            self.mesh(cx, index, [input.clone(), other], index == scratch);
            if index < scratch {
                input = self.scratch(cx, index, pass.size).texture().clone();
            }
        }
        for (index, pass) in passes.iter().enumerate() {
            let group = if index < scratch { out.target(&self.scratch[index]) } else { out.main() };
            group.push(&self.passes[index].mesh, DRAW, ()).block(&pass.block);
        }
    }
}

impl ImageProcessLayer {
    /// The scratch target of the pass at `index`, of `size`.
    fn scratch(&mut self, cx: &mut PrepareCx<'_, '_, Self>, index: usize, size: Size) -> &Target {
        if index == self.scratch.len() {
            self.scratch.push(cx.res.target(size, TextureChannelDataType::HalfFloat, Rgba::TRANSPARENT));
        }
        let target = &self.scratch[index];
        target.set_size(&mut cx.res, size);
        target
    }

    /// Makes the mesh of the pass at `index` sample `textures`, blending
    /// where it `blends`: the one it has, for the same.
    fn mesh(&mut self, cx: &mut PrepareCx<'_, '_, Self>, index: usize, textures: [SharedTexture; 2], blends: bool) {
        if self.passes.get(index).is_some_and(|pass| pass.textures == textures && pass.blends == blends) {
            return;
        }
        let shader = cx.res.context.shader_key(&IMAGE_PROCESS, Uniforms::default(), ProjectionVariant::Mercator);
        let corners: Vec<u8> = [0i16, 0, 1, 0, 0, 1, 1, 1].iter().flat_map(|c| c.to_le_bytes()).collect();
        let geometry = DrawGeometry {
            vertices: raw_vertices(POS.name, corners, 4, POS.format),
            vertex_count: 4,
            mode: DrawMode::Triangles(),
            indexes: DrawIndexes::new(RenderStaticData::quad_triangle_indices()),
            segments: vec![SegmentBase::with_lengths(0, 0, 4, 6)],
            instances: None,
        };
        let mut bound = Textures::default();
        bound[IMAGE_PROCESS.texture_slot("image")] = Some(textures[0].clone());
        bound[IMAGE_PROCESS.texture_slot("other")] = Some(textures[1].clone());
        let color_mode = if blends { ColorMode::alpha_blended() } else { ColorMode::unblended() };
        let mesh = cx.res.mesh(shader, color_mode, CullFaceMode::disabled(), geometry, bound);
        let pass = PassMesh { textures, blends, mesh };
        if index == self.passes.len() {
            self.passes.push(pass);
        } else {
            self.passes[index] = pass;
        }
    }
}
