// The controls' overlay on OpenGL ES: egui's own painter (egui_glow), in the
// map's context, over the window's framebuffer. It leaves its GL state
// behind, which the backend's context forgets after each overlay.

use std::cell::RefCell;
use std::rc::Rc;

use mln_gl::surface_target::{Overlay, OverlayTarget};
use mln_util::logging::{Event, Log};

use crate::overlay::Paint;

/// egui's painter, whose GL objects go with it.
struct Painter(egui_glow::Painter);

impl Drop for Painter {
    fn drop(&mut self) {
        // The backend drops its overlay with the context current.
        self.0.destroy();
    }
}

/// The overlay that paints the controls on OpenGL ES: egui's painter, made
/// with the first frame.
pub(crate) fn gl_overlay(paint: Rc<RefCell<Paint>>) -> Overlay {
    let mut painter: Option<Painter> = None;
    let mut failed = false;
    Box::new(move |target: &OverlayTarget<'_>| {
        if painter.is_none() && !failed {
            // As egui-wgpu does by default, with dithering.
            match egui_glow::Painter::new(std::sync::Arc::clone(target.gl), "", None, true) {
                Ok(made) => painter = Some(Painter(made)),
                Err(e) => {
                    failed = true;
                    Log::error(Event::Render, format!("Can't draw the controls with OpenGL ES: {e}"));
                }
            }
        }
        let Some(Painter(painter)) = painter.as_mut() else {
            return;
        };
        let mut paint = paint.borrow_mut();
        let mut textures = std::mem::take(&mut paint.textures);
        for (id, deltas) in &textures.set {
            for delta in deltas {
                painter.set_texture(*id, delta);
            }
        }
        if !paint.primitives.is_empty() {
            let size = [target.size.width, target.size.height];
            painter.paint_primitives(size, paint.pixels_per_point, &paint.primitives);
        }
        for id in &textures.free {
            painter.free_texture(*id);
        }
        textures.clear();
    })
}
