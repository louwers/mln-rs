// The controls' paint, which an overlay of the window's backend draws over
// each frame of the map (`set_overlay`): egui's own renderer on WebGPU, its
// own painter on OpenGL ES (gl_overlay.rs), and on Metal and Vulkan egui's
// meshes drawn in the frame's command buffer (metal_overlay.rs,
// vulkan_overlay.rs).

use std::cell::RefCell;
use std::rc::Rc;

/// The controls' last frame, which the overlay paints over each frame of the
/// map until the next.
#[derive(Default)]
pub(crate) struct Paint {
    pub(crate) primitives: Vec<egui::ClippedPrimitive>,
    /// The textures to update before painting, and to free after.
    pub(crate) textures: egui::TexturesDelta,
    pub(crate) pixels_per_point: f32,
}

impl Paint {
    /// Has egui send every texture again, whole, with its next pass, for an
    /// overlay that has none (egui sends a texture once): the font atlas,
    /// and the images, which egui loads again.
    pub(crate) fn resend_textures(&mut self, ui: &egui::Context) {
        self.textures.clear();
        #[expect(
            clippy::redundant_closure_for_method_calls,
            reason = "`FontsView::image` takes one lifetime of the view, the closure every one"
        )]
        let font_image = ui.fonts(|fonts| fonts.image());
        ui.tex_manager().write().set(
            egui::TextureId::default(),
            egui::epaint::ImageDelta::full(font_image, egui::epaint::TextureAtlas::texture_options()),
        );
        ui.forget_all_images();
    }
}

impl Drop for Paint {
    fn drop(&mut self) {
        // The window closes with these unpainted, which is fine.
        self.textures.clear();
    }
}

/// The overlay that paints the controls on WebGPU: egui's renderer on the
/// map's device, made with the first frame.
pub(crate) fn webgpu_overlay(paint: Rc<RefCell<Paint>>) -> mln_webgpu::surface_target::Overlay {
    let mut renderer: Option<egui_wgpu::Renderer> = None;
    Box::new(move |target: &mln_webgpu::surface_target::OverlayTarget<'_>| {
        let renderer = renderer.get_or_insert_with(|| {
            egui_wgpu::Renderer::new(target.device, target.format, egui_wgpu::RendererOptions::default())
        });
        let mut paint = paint.borrow_mut();
        let mut textures = std::mem::take(&mut paint.textures);
        for (id, deltas) in &textures.set {
            for delta in deltas {
                renderer.update_texture(target.device, target.queue, *id, delta);
            }
        }
        if !paint.primitives.is_empty() {
            let screen = egui_wgpu::ScreenDescriptor {
                size_in_pixels: [target.size.width, target.size.height],
                pixels_per_point: paint.pixels_per_point,
            };
            let mut encoder =
                target.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("controls") });
            let mut commands =
                renderer.update_buffers(target.device, target.queue, &mut encoder, &paint.primitives, &screen);
            {
                let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("controls"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target.view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    })],
                    ..Default::default()
                });
                renderer.render(&mut pass.forget_lifetime(), &paint.primitives, &screen);
            }
            commands.push(encoder.finish());
            target.queue.submit(commands);
        }
        for id in &textures.free {
            renderer.free_texture(id);
        }
        textures.clear();
    })
}
