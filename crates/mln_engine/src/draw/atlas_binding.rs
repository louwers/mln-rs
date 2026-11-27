use mln_gfx::gpu::ShaderKey;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_style::types::AlignmentType;

use crate::draw::context::Context;
use crate::draw::objects::{DrawObjects, Textures};
use crate::draw::store::SharedTexture;
use crate::draw::tile_render_data::TileAtlasTextures;
use crate::render::paint_parameters::PaintParameters;

/// Gives a draw the textures of its tile's glyph and icon atlases, which
/// change after the draw is made.
#[derive(Clone)]
pub struct AtlasBinding {
    /// The textures of the tile's glyph and icon atlases, if it has
    /// atlases.
    atlases: Option<(Option<SharedTexture>, Option<SharedTexture>)>,
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
    pub fn new(
        atlases: Option<TileAtlasTextures>,
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
            atlases: atlases.map(|atlases| (atlases.glyph().cloned(), atlases.icon().cloned())),
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
    /// shader samples them from. `bind` sets how they filter.
    #[must_use]
    pub fn textures(&self, shader: &ShaderKey) -> Textures {
        let mut textures = Textures::default();
        self.set_textures(self.samples_icon(shader), |slot, texture| textures[slot] = texture.cloned());
        textures
    }

    /// Gives the mesh the atlases' textures before it draws, and sets the
    /// samplers of the atlases, which the tile's draws share, to filter as
    /// the frame's view asks.
    pub(crate) fn bind(&self, mesh: &mut DrawObjects, parameters: &mut PaintParameters<'_>) {
        let state = parameters.view;
        let transformed = self.rotation_alignment == AlignmentType::Map || state.pitch() != 0.0;
        let linear_filter_for_icons = if self.is_text {
            parameters.camera_moving || transformed || !self.text_size_is_zoom_constant
        } else {
            self.sdf_icons || parameters.camera_moving || self.icon_scaled || transformed
        };
        if self.glyph_slot.is_some()
            && let Some((glyph, icon)) = &self.atlases
        {
            set_samplers(parameters.context, glyph.as_ref(), icon.as_ref(), linear_filter_for_icons);
        }
        let samples_icon = self.samples_icon(mesh.shader());
        self.set_textures(samples_icon, |slot, texture| mesh.set_texture(slot, texture));
    }

    /// Sets the atlases' textures, those of a shader that samples the icon
    /// texture or not, with `set(slot, texture)`.
    fn set_textures(&self, samples_icon: bool, mut set: impl FnMut(usize, Option<&SharedTexture>)) {
        let (Some(glyph_slot), Some((glyph, icon))) = (self.glyph_slot, &self.atlases) else {
            return;
        };
        let (glyph, icon) = (glyph.as_ref(), icon.as_ref());
        let glyph_slot = usize::from(glyph_slot);
        if let Some(icon_slot) = self.icon_slot.map(usize::from).filter(|_| samples_icon) {
            debug_assert_ne!(glyph_slot, icon_slot);
            set(glyph_slot, glyph);
            set(icon_slot, icon);
        } else {
            set(glyph_slot, if self.is_text { glyph } else { icon });
        }
    }

    /// Whether `shader` samples the icon texture.
    fn samples_icon(&self, shader: &ShaderKey) -> bool {
        let textures = shader.program().textures;
        self.icon_slot.is_some_and(|slot| textures.get(usize::from(slot)).is_some_and(|texture| !texture.is_unused()))
    }
}

/// Makes the glyph atlas's texture filter linearly, and the icon atlas's
/// linearly or not.
fn set_samplers(
    context: &mut Context,
    glyph: Option<&SharedTexture>,
    icon: Option<&SharedTexture>,
    linear_filter_for_icons: bool,
) {
    if let Some(mut glyph) = glyph.and_then(|glyph| context.texture_mut(glyph)) {
        glyph.set_sampler(SamplerState {
            filter: TextureFilterType::Linear,
            wrap_u: TextureWrapType::Clamp,
            wrap_v: TextureWrapType::Clamp,
            ..Default::default()
        });
    }
    if let Some(mut icon) = icon.and_then(|icon| context.texture_mut(icon)) {
        icon.set_sampler(SamplerState {
            filter: if linear_filter_for_icons { TextureFilterType::Linear } else { TextureFilterType::Nearest },
            wrap_u: TextureWrapType::Clamp,
            wrap_v: TextureWrapType::Clamp,
            ..Default::default()
        });
    }
}
