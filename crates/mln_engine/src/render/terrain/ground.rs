//! The ground that the layers that stand on terrain stand on, and hide
//! behind, in a frame: the ground the frame's surface drew, which gives
//! their points their heights ([`TileGround`]), and the depth of that
//! ground, which the layers that hide behind it read.

use std::mem::size_of;
use std::sync::{Arc, Weak};

use mln_gfx::gpu::{ColorAttachment, DepthStencilAttachment, Gpu, LoadOp, TargetFormats, TextureFormat};
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_gfx::{AttributeDataType, Rgba, Size};

use super::{TerrainElevation, TileGround};
use crate::draw::context::Context;
use crate::draw::objects::Mesh;
use crate::draw::resources::GpuTexture;
use crate::draw::store::SharedTexture;
use crate::draw::vertex_attribute::SharedAttribute;
use crate::draw::vertices::{SharedVertices, TypedVertices};
use crate::render::paint_parameters::OpenPass;
use crate::tile::UnwrappedTileID;

/// The texel format of the ground's depth: an `f32`'s bits in RGBA8, which
/// every backend draws into and reads in a vertex stage.
const DEPTH_FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;

/// The format of the depth attachment the ground's pass tests, so that the
/// nearest ground stays in its texture whatever order the tiles draw in:
/// one every backend draws into, as the layer composite's.
const DEPTH_STENCIL: TextureFormat = TextureFormat::Depth24PlusStencil8;

/// The ground of a frame: the ground its surface drew, and, where a layer
/// hides behind it, the texture of its depth.
pub(crate) struct FrameGround {
    elevation: Arc<TerrainElevation>,
    depth: Option<SharedTexture>,
}

impl FrameGround {
    /// The ground `elevation`, whose depth draws into `depth` where a layer
    /// hides behind it.
    pub(crate) fn new(elevation: Arc<TerrainElevation>, depth: Option<SharedTexture>) -> FrameGround {
        FrameGround { elevation, depth }
    }

    /// The ground the frame drew, whose `Arc` stays the same while it does.
    pub(crate) fn elevation(&self) -> &Arc<TerrainElevation> {
        &self.elevation
    }

    /// The ground under the points of `tile`.
    pub(crate) fn tile(&self, tile: UnwrappedTileID) -> TileGround<'_> {
        TileGround::new(&self.elevation, tile)
    }

    /// The texture of the ground's depth, where a layer hides behind it:
    /// each pixel's eye distance, an `f32`'s bits, least significant byte
    /// first, 0 where no ground drew.
    pub(crate) fn depth(&self) -> Option<&SharedTexture> {
        self.depth.as_ref()
    }
}

/// The center of the point of a vertex of a point's quad at `a1`, the
/// point's position, doubled, plus the quad's corner (`add_point_quads`).
pub(crate) fn point_center(a1: [i16; 2]) -> [i16; 2] {
    a1.map(|value| value >> 1)
}

/// The heights of the ground under a draw's vertices, which an attribute or
/// storage buffer of its reads, and the ground they are of.
#[derive(Default)]
pub(crate) struct GroundHeights(Option<(Weak<TerrainElevation>, TypedVertices<f32>)>);

impl GroundHeights {
    /// Whether they are of `ground`, which stays the frame's ground while
    /// its `Arc` does.
    pub(crate) fn are_of(&self, ground: &FrameGround) -> bool {
        self.0.as_ref().is_some_and(|(of, _)| Weak::ptr_eq(of, &Arc::downgrade(ground.elevation())))
    }

    /// Makes them `heights`, of `ground`, through `context`: written over
    /// those they were where they fit, which their readers read still, else
    /// in new vertices, which it returns for the readers to read.
    pub(crate) fn write(
        &mut self,
        context: &mut Context,
        ground: &FrameGround,
        heights: &[f32],
    ) -> Option<SharedVertices> {
        let of = Arc::downgrade(ground.elevation());
        if let Some((_, vertices)) = &self.0
            && context.update_vertices(vertices, heights)
        {
            self.0 = Some((of, vertices.clone()));
            return None;
        }
        let vertices = context.add_dynamic_vertices(heights);
        let shared = vertices.shared().clone();
        self.0 = Some((of, vertices));
        Some(shared)
    }
}

/// The attribute of a vertex that reads its height from `heights`, an `f32`
/// a vertex.
pub(crate) fn height_attribute(heights: &SharedVertices) -> SharedAttribute {
    SharedAttribute::new(heights.clone(), AttributeDataType::Float, 0, size_of::<f32>() as u32)
}

/// A tile's draw whose points stand on the ground: its mesh, and the
/// heights of the ground under its vertices, which an attribute of theirs
/// reads.
pub(crate) struct StandingDraw {
    pub(crate) mesh: Mesh,
    heights: GroundHeights,
}

impl StandingDraw {
    /// The draw of `mesh`, which stands on no ground yet.
    pub(crate) fn new(mesh: Mesh) -> Self {
        StandingDraw { mesh, heights: GroundHeights::default() }
    }

    /// Over `ground`, has the attribute `name` of the draw's vertices read the
    /// heights of the ground under `centers`, the center of each vertex's
    /// point in `tile`: those of the ground they were of while it stays the
    /// frame's, else written again, through `context`.
    pub(crate) fn stand(
        &mut self,
        context: &mut Context,
        ground: Option<&FrameGround>,
        tile: UnwrappedTileID,
        name: &'static str,
        centers: &[[i16; 2]],
    ) {
        let Some(ground) = ground.filter(|ground| !self.heights.are_of(ground)) else {
            return;
        };
        let tile_ground = ground.tile(tile);
        let heights: Vec<f32> =
            centers.iter().map(|&[x, y]| tile_ground.height(glam::Vec2::new(f32::from(x), f32::from(y)))).collect();
        if let Some(vertices) = self.heights.write(context, ground, &heights)
            && let Some(attrs) = self.mesh.vertex_attributes_mut(context)
        {
            attrs.set_shared(name, height_attribute(&vertices));
        }
    }
}

/// The texture of the ground's depth, of the frame's size, the depth
/// attachment it draws with, and the pass that draws it. The pass draws
/// into neither of the frame's attachments, which a window's framebuffer
/// can't share with other textures.
#[derive(Default)]
pub(crate) struct GroundDepth {
    texture: Option<(SharedTexture, Size)>,
    /// The depth the pass tests, made with it, of the texture's size.
    depth: Option<GpuTexture>,
}

impl GroundDepth {
    /// The texture of a frame of `size` device pixels, made in `context`'s
    /// store when the frame's size changes.
    pub(crate) fn texture(&mut self, context: &mut Context, size: Size) -> SharedTexture {
        match &self.texture {
            Some((texture, made)) if *made == size => texture.clone(),
            _ => {
                let texture = context.add_texture_with(|texture| {
                    texture.set_size(size);
                    texture.set_format(DEPTH_FORMAT);
                    texture.set_render_target();
                    texture.set_sampler(SamplerState {
                        filter: TextureFilterType::Nearest,
                        wrap_u: TextureWrapType::Clamp,
                        wrap_v: TextureWrapType::Clamp,
                        ..SamplerState::default()
                    });
                });
                self.depth = None;
                self.texture.insert((texture, size)).0.clone()
            }
        }
    }

    /// The pass that draws the ground into the texture, cleared to no
    /// ground, testing a depth attachment of its own, cleared too. `None`
    /// without a texture, or where making it through `gpu` failed, which
    /// fails the frame.
    pub(crate) fn pass(&mut self, gpu: &mut dyn Gpu, context: &mut Context) -> Option<OpenPass> {
        let (texture, size) = self.texture.as_ref()?;
        let made = context.create_texture(gpu, texture).and_then(|color| {
            if self.depth.is_none() {
                self.depth = Some(context.device().attachment(gpu, "ground depth", *size, DEPTH_STENCIL)?);
            }
            Ok(color)
        });
        let color = match made {
            Ok(color) => color,
            Err(error) => {
                context.fail(format!("Making the ground's depth failed: {error}"));
                return None;
            }
        };
        Some(OpenPass {
            label: "ground depth",
            color: Some(ColorAttachment { texture: color, load: LoadOp::Clear(Rgba::new(0.0, 0.0, 0.0, 0.0)) }),
            depth_stencil: self.depth.as_ref().map(|depth| DepthStencilAttachment {
                texture: depth.id(),
                depth: LoadOp::Clear(1.0),
                stencil: LoadOp::Clear(0),
            }),
            size: *size,
            targets: TargetFormats { color: Some(DEPTH_FORMAT), depth_stencil: Some(DEPTH_STENCIL) },
        })
    }

    /// Drops the texture and its depth, in a frame that doesn't draw the
    /// ground's depth.
    pub(crate) fn clear(&mut self) {
        self.texture = None;
        self.depth = None;
    }
}
