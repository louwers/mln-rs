//! The textures of a tile's glyph and icon atlases, which the draws of
//! symbols and patterns sample, filtered as each frame asks.

use mln_engine::draw::{LayerFrame, Mesh, Resources, Texture, Textures};
use mln_engine::layer::TileAtlasTextures;
use mln_gfx::gpu::ShaderKey;
use mln_gfx::shaders::program::Program;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_style::types::AlignmentType;

/// Gives a draw the textures of its tile's glyph and icon atlases, and
/// the states it samples them with, before each frame draws it.
#[derive(Clone)]
pub(crate) struct AtlasBinding {
    glyph: Option<Texture>,
    icon: Option<Texture>,
    icon_slot: Option<u8>,
    glyph_slot: Option<u8>,
    is_text: bool,
    sdf_icons: bool,
    rotation_alignment: AlignmentType,
    icon_scaled: bool,
    text_size_is_zoom_constant: bool,
}

impl AtlasBinding {
    /// The binding of a tile's glyph and icon atlas textures, `atlases`, to
    /// the slots of a program that samples them, for a draw of text
    /// (`is_text`) or icons laid out as the other arguments say.
    #[expect(clippy::too_many_arguments, reason = "each argument is a field of the binding")]
    #[must_use]
    pub(crate) fn new(
        atlases: &TileAtlasTextures,
        icon_slot: Option<usize>,
        glyph_slot: Option<usize>,
        is_text: bool,
        sdf_icons: bool,
        rotation_alignment: AlignmentType,
        icon_scaled: bool,
        text_size_is_zoom_constant: bool,
    ) -> Self {
        debug_assert_ne!(icon_slot, glyph_slot);
        let slot = |slot: Option<usize>| slot.and_then(|slot| u8::try_from(slot).ok());
        AtlasBinding {
            glyph: atlases.glyph().cloned(),
            icon: atlases.icon().cloned(),
            icon_slot: slot(icon_slot),
            glyph_slot: slot(glyph_slot),
            is_text,
            sdf_icons,
            rotation_alignment,
            icon_scaled,
            text_size_is_zoom_constant,
        }
    }

    /// The textures of a draw with `shader`: the atlases', in the slots the
    /// shader samples them from. [`bind`](Self::bind) sets how they filter.
    #[must_use]
    pub(crate) fn textures(&self, shader: &ShaderKey) -> Textures {
        let mut textures = Textures::default();
        self.set_textures(self.samples_icon(shader.program()), |slot, texture, _| textures[slot] = texture.cloned());
        textures
    }

    /// Gives `mesh` the atlases' textures, sampled as `frame` asks: the
    /// glyphs' linearly, and the icons' linearly where the icons scale,
    /// turn or move with the frame's camera.
    pub(crate) fn bind(&self, mesh: &Mesh, res: &mut Resources<'_>, frame: &LayerFrame<'_>) {
        let transformed = self.rotation_alignment == AlignmentType::Map || frame.view.pitch() != 0.0;
        let linear_filter_for_icons = if self.is_text {
            frame.camera_moving || transformed || !self.text_size_is_zoom_constant
        } else {
            self.sdf_icons || frame.camera_moving || self.icon_scaled || transformed
        };
        let sampler = |filter| SamplerState {
            filter,
            wrap_u: TextureWrapType::Clamp,
            wrap_v: TextureWrapType::Clamp,
            ..Default::default()
        };
        let glyph_sampler = sampler(TextureFilterType::Linear);
        let icon_sampler =
            sampler(if linear_filter_for_icons { TextureFilterType::Linear } else { TextureFilterType::Nearest });
        self.set_textures(self.samples_icon(mesh.program()), |slot, texture, is_glyph| {
            mesh.set_texture(res, slot, texture);
            mesh.set_sampler(res, slot, Some(if is_glyph { glyph_sampler } else { icon_sampler }));
        });
    }

    /// Sets the atlases' textures, those of a program that samples the icon
    /// texture or not, with `set(slot, texture, is_glyph)`.
    fn set_textures(&self, samples_icon: bool, mut set: impl FnMut(usize, Option<&Texture>, bool)) {
        let Some(glyph_slot) = self.glyph_slot else {
            return;
        };
        let (glyph, icon) = (self.glyph.as_ref(), self.icon.as_ref());
        let glyph_slot = usize::from(glyph_slot);
        if let Some(icon_slot) = self.icon_slot.map(usize::from).filter(|_| samples_icon) {
            debug_assert_ne!(glyph_slot, icon_slot);
            set(glyph_slot, glyph, true);
            set(icon_slot, icon, false);
        } else if self.is_text {
            set(glyph_slot, glyph, true);
        } else {
            set(glyph_slot, icon, false);
        }
    }

    /// Whether `program` samples the icon texture.
    fn samples_icon(&self, program: &Program) -> bool {
        let textures = program.textures;
        self.icon_slot.is_some_and(|slot| textures.get(usize::from(slot)).is_some_and(|texture| !texture.is_unused()))
    }
}
