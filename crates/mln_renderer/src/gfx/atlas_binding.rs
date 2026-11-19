use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_style_types::AlignmentType;

use crate::draw::{DrawObjects, Textures};
use crate::gfx::context::Context;
use crate::gfx::shader_program::ShaderProgram;
use crate::gfx::store::SharedTexture;
use crate::paint_parameters::PaintParameters;
use crate::tile_render_data::TileAtlasTextures;

/// Gives a draw the textures of its tile's glyph and icon atlases, which
/// change after the draw is made.
#[derive(Clone)]
pub(crate) struct AtlasBinding {
    /// The textures of the tile's glyph and icon atlases, if it has
    /// atlases.
    atlases: Option<(Option<SharedTexture>, Option<SharedTexture>)>,
    icon_texture_id: Option<u8>,
    glyph_texture_id: Option<u8>,
    is_text: bool,
    sdf_icons: bool,
    rotation_alignment: AlignmentType,
    icon_scaled: bool,
    text_size_is_zoom_constant: bool,
}

impl AtlasBinding {
    #[expect(clippy::too_many_arguments, reason = "each argument is a field of the binding")]
    #[must_use]
    pub(crate) fn new(
        atlases: Option<TileAtlasTextures>,
        icon_texture_id: Option<usize>,
        glyph_texture_id: Option<usize>,
        is_text: bool,
        sdf_icons: bool,
        rotation_alignment: AlignmentType,
        icon_scaled: bool,
        text_size_is_zoom_constant: bool,
    ) -> Self {
        debug_assert_ne!(icon_texture_id, glyph_texture_id);
        let texture_id = |id: Option<usize>| id.and_then(|id| u8::try_from(id).ok());
        AtlasBinding {
            atlases: atlases.map(|atlases| (atlases.glyph().cloned(), atlases.icon().cloned())),
            icon_texture_id: texture_id(icon_texture_id),
            glyph_texture_id: texture_id(glyph_texture_id),
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
    pub(crate) fn textures(&self, shader: &ShaderProgram) -> Textures {
        let mut textures = Textures::default();
        self.set_textures(self.samples_icon(shader), |id, texture| textures[id] = texture.cloned());
        textures
    }

    /// Gives the draw the atlases' textures before it draws, and sets the
    /// samplers of the atlases, which the tile's draws share, to filter as
    /// the frame's view asks.
    pub(crate) fn bind(&self, objects: &mut DrawObjects, parameters: &mut PaintParameters<'_>) {
        let state = parameters.state;
        let transformed = self.rotation_alignment == AlignmentType::Map || state.get_pitch() != 0.0;
        let linear_filter_for_icons = if self.is_text {
            parameters.camera_moving || transformed || !self.text_size_is_zoom_constant
        } else {
            self.sdf_icons || parameters.camera_moving || self.icon_scaled || transformed
        };
        if self.glyph_texture_id.is_some()
            && let Some((glyph, icon)) = &self.atlases
        {
            set_samplers(parameters.context, glyph.as_ref(), icon.as_ref(), linear_filter_for_icons);
        }
        let samples_icon = self.samples_icon(objects.shader());
        self.set_textures(samples_icon, |id, texture| objects.set_texture(id, texture));
    }

    /// Sets the atlases' textures, those of a shader that samples the icon
    /// texture or not, with `set(id, texture)`.
    fn set_textures(&self, samples_icon: bool, mut set: impl FnMut(usize, Option<&SharedTexture>)) {
        let (Some(glyph_texture_id), Some((glyph, icon))) = (self.glyph_texture_id, &self.atlases) else {
            return;
        };
        let (glyph, icon) = (glyph.as_ref(), icon.as_ref());
        let glyph_texture_id = usize::from(glyph_texture_id);
        if let Some(icon_texture_id) = self.icon_texture_id.map(usize::from).filter(|_| samples_icon) {
            debug_assert_ne!(glyph_texture_id, icon_texture_id);
            set(glyph_texture_id, glyph);
            set(icon_texture_id, icon);
        } else {
            set(glyph_texture_id, if self.is_text { glyph } else { icon });
        }
    }

    /// Whether `shader` samples the icon texture.
    fn samples_icon(&self, shader: &ShaderProgram) -> bool {
        self.icon_texture_id.is_some_and(|id| shader.samples(usize::from(id)))
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
