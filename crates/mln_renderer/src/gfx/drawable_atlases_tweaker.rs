// This tweaker reads the renderer's `TileAtlasTextures`, so it lives in
// mln_renderer rather than mln_gfx.

use std::rc::Rc;

use mln_gfx::drawable::Drawable;
use mln_gfx::drawable_tweaker::DrawableTweaker;
use mln_gfx::paint_parameters::PaintParameters;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_style_types::AlignmentType;

use crate::tile_render_data::TileAtlasTextures;

/// Tweaker that applies the latest values from a retained
/// `TileAtlasTexturesPtr`.
pub(crate) struct DrawableAtlasesTweaker {
    atlases: Option<Rc<TileAtlasTextures>>,
    icon_texture_id: Option<usize>,
    glyph_texture_id: Option<usize>,
    is_text: bool,
    sdf_icons: bool,
    rotation_alignment: AlignmentType,
    icon_scaled: bool,
    text_size_is_zoom_constant: bool,
}

impl DrawableAtlasesTweaker {
    #[allow(clippy::too_many_arguments)]
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
        DrawableAtlasesTweaker {
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

    fn setup_textures(&self, drawable: &mut dyn Drawable, linear_filter_for_icons: bool) {
        let Some(shader) = drawable.get_shader().clone() else {
            return;
        };
        let (Some(glyph_texture_id), Some(atlases)) = (self.glyph_texture_id, &self.atlases) else {
            return;
        };
        let glyph = atlases.glyph.borrow().clone();
        let icon = atlases.icon.borrow().clone();
        if let Some(glyph) = &glyph {
            glyph.borrow_mut().set_sampler_configuration(SamplerState {
                filter: TextureFilterType::Linear,
                wrap_u: TextureWrapType::Clamp,
                wrap_v: TextureWrapType::Clamp,
                ..Default::default()
            });
        }
        if let Some(icon) = &icon {
            icon.borrow_mut().set_sampler_configuration(SamplerState {
                filter: if linear_filter_for_icons { TextureFilterType::Linear } else { TextureFilterType::Nearest },
                wrap_u: TextureWrapType::Clamp,
                wrap_v: TextureWrapType::Clamp,
                ..Default::default()
            });
        }
        if let Some(icon_texture_id) = self.icon_texture_id
            && shader.get_sampler_location(icon_texture_id).is_some()
        {
            debug_assert_ne!(glyph_texture_id, icon_texture_id);
            drawable.set_texture(glyph, glyph_texture_id);
            drawable.set_texture(icon, icon_texture_id);
        } else {
            drawable.set_texture(if self.is_text { glyph } else { icon }, glyph_texture_id);
        }
    }
}

impl DrawableTweaker for DrawableAtlasesTweaker {
    fn init(&self, drawable: &mut dyn Drawable) {
        self.setup_textures(drawable, true);
    }

    fn execute(&self, drawable: &mut dyn Drawable, parameters: &mut dyn PaintParameters) {
        let transformed = self.rotation_alignment == AlignmentType::Map || parameters.state_pitch() != 0.0;
        let linear_filter_for_icons = if self.is_text {
            parameters.state_is_changing() || transformed || !self.text_size_is_zoom_constant
        } else {
            self.sdf_icons || parameters.state_is_changing() || self.icon_scaled || transformed
        };
        self.setup_textures(drawable, linear_filter_for_icons);
    }
}
