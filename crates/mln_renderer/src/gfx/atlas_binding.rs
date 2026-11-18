use std::rc::Rc;

use mln_gfx::shaders::shader_program::ShaderProgram;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_style_types::AlignmentType;

use crate::draw::{DrawObjects, Textures};
use crate::paint_parameters::PaintParameters;
use crate::tile_render_data::TileAtlasTextures;

/// Gives a draw the textures of its tile's glyph and icon atlases, which
/// change after the draw is made.
pub(crate) struct AtlasBinding {
    atlases: Option<Rc<TileAtlasTextures>>,
    icon_texture_id: Option<usize>,
    glyph_texture_id: Option<usize>,
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
        atlases: Option<Rc<TileAtlasTextures>>,
        icon_texture_id: Option<usize>,
        glyph_texture_id: Option<usize>,
        is_text: bool,
        sdf_icons: bool,
        rotation_alignment: AlignmentType,
        icon_scaled: bool,
        text_size_is_zoom_constant: bool,
    ) -> Self {
        debug_assert_ne!(icon_texture_id, glyph_texture_id);
        AtlasBinding {
            atlases,
            icon_texture_id,
            glyph_texture_id,
            is_text,
            sdf_icons,
            rotation_alignment,
            icon_scaled,
            text_size_is_zoom_constant,
        }
    }

    /// Sets the atlases' textures in `textures`, those of a shader that
    /// samples the icon texture or not, with icons filtered linearly or not.
    fn setup_textures(&self, textures: &mut Textures, samples_icon: bool, linear_filter_for_icons: bool) {
        let (Some(glyph_texture_id), Some(atlases)) = (self.glyph_texture_id, &self.atlases) else {
            return;
        };
        let glyph = atlases.glyph();
        let icon = atlases.icon();
        if let Some(glyph) = &glyph {
            glyph.borrow_mut().set_sampler(SamplerState {
                filter: TextureFilterType::Linear,
                wrap_u: TextureWrapType::Clamp,
                wrap_v: TextureWrapType::Clamp,
                ..Default::default()
            });
        }
        if let Some(icon) = &icon {
            icon.borrow_mut().set_sampler(SamplerState {
                filter: if linear_filter_for_icons { TextureFilterType::Linear } else { TextureFilterType::Nearest },
                wrap_u: TextureWrapType::Clamp,
                wrap_v: TextureWrapType::Clamp,
                ..Default::default()
            });
        }
        if let Some(icon_texture_id) = self.icon_texture_id.filter(|_| samples_icon) {
            debug_assert_ne!(glyph_texture_id, icon_texture_id);
            textures[glyph_texture_id] = glyph;
            textures[icon_texture_id] = icon;
        } else {
            textures[glyph_texture_id] = if self.is_text { glyph } else { icon };
        }
    }

    /// Whether `shader` samples the icon texture.
    fn samples_icon(&self, shader: &ShaderProgram) -> bool {
        self.icon_texture_id.is_some_and(|id| shader.samples(id))
    }
}

impl AtlasBinding {
    /// The textures of a draw with `shader` when it is made: the atlases'.
    #[must_use]
    pub(crate) fn textures(&self, shader: &ShaderProgram) -> Textures {
        let mut textures = Textures::default();
        self.setup_textures(&mut textures, self.samples_icon(shader), true);
        textures
    }

    /// Gives the draw the atlases' textures before it draws, filtered as the
    /// frame's view asks.
    pub(crate) fn bind(&self, objects: &mut DrawObjects, parameters: &PaintParameters<'_>) {
        let state = parameters.state;
        let transformed = self.rotation_alignment == AlignmentType::Map || state.get_pitch() != 0.0;
        let linear_filter_for_icons = if self.is_text {
            state.is_changing() || transformed || !self.text_size_is_zoom_constant
        } else {
            self.sdf_icons || state.is_changing() || self.icon_scaled || transformed
        };
        let samples_icon = self.samples_icon(objects.shader());
        self.setup_textures(&mut objects.textures, samples_icon, linear_filter_for_icons);
    }
}
