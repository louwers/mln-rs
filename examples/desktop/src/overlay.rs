// The controls' paint, which an overlay of the window's backend draws over
// each frame of the map (`set_overlay`): egui's own renderer on WebGPU, and
// on Metal egui's meshes drawn in the frame's command buffer
// (metal_overlay.rs).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

/// The controls' last frame, which the overlay paints over each frame of the
/// map until the next.
#[derive(Default)]
pub(crate) struct Paint {
    pub(crate) primitives: Vec<egui::ClippedPrimitive>,
    /// The textures to update before painting, and to free after.
    pub(crate) textures: egui::TexturesDelta,
    pub(crate) pixels_per_point: f32,
    /// Every texture egui has, as of its last update: egui sends a texture
    /// once, and a new overlay needs them all again (`resend_textures`).
    images: HashMap<egui::TextureId, (Arc<egui::ColorImage>, egui::TextureOptions)>,
}

impl Paint {
    /// Takes egui's texture updates, for the overlay to apply before it next
    /// paints.
    pub(crate) fn add_textures(&mut self, delta: egui::TexturesDelta) {
        for (id, updates) in &delta.set {
            for update in updates {
                let egui::ImageData::Color(image) = &update.image;
                match update.pos {
                    None => {
                        self.images.insert(*id, (image.clone(), update.options));
                    }
                    Some([x, y]) => {
                        if let Some((full, _)) = self.images.get_mut(id) {
                            let full = Arc::make_mut(full);
                            let width = full.size[0];
                            for (row, pixels) in image.pixels.chunks_exact(image.size[0].max(1)).enumerate() {
                                let start = (y + row) * width + x;
                                full.pixels[start..start + pixels.len()].copy_from_slice(pixels);
                            }
                        }
                    }
                }
            }
        }
        for id in &delta.free {
            self.images.remove(id);
        }
        self.textures.append(delta);
    }

    /// Sends every texture again, whole, for an overlay that has none.
    pub(crate) fn resend_textures(&mut self) {
        self.textures.clear();
        for (id, (image, options)) in &self.images {
            self.textures.push(*id, egui::epaint::ImageDelta::full(image.clone(), *options));
        }
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
pub(crate) fn webgpu_overlay(paint: Rc<RefCell<Paint>>) -> mln_webgpu::surface_backend::Overlay {
    let mut renderer: Option<egui_wgpu::Renderer> = None;
    Box::new(move |target: &mln_webgpu::surface_backend::OverlayTarget<'_>| {
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
