// The renderer frontend, after platform/glfw/glfw_renderer_frontend.cpp.
//
// Like HeadlessFrontend, the frontend is a handle (`Clone` shares the state)
// because the Map keeps one (C++: a reference). It holds the view's shared
// state to mark it dirty when the map has a new frame to draw.

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use mln_gfx::{BackendScope, RendererBackend, ScopeType};
use mln_renderer::{Renderer, RendererFrontend, RendererObserver, UpdateParameters};

use crate::view::ViewShared;

struct Shared {
    view: Rc<ViewShared>,
    backend: Rc<dyn RendererBackend>,
    renderer: RefCell<Option<Renderer>>,
    update_parameters: RefCell<Option<Arc<UpdateParameters>>>,
}

/// Draws the map's latest update parameters on the view's backend.
#[derive(Clone)]
pub struct Frontend {
    shared: Rc<Shared>,
}

impl Frontend {
    pub fn new(renderer: Renderer, backend: Rc<dyn RendererBackend>, view: Rc<ViewShared>) -> Self {
        Frontend {
            shared: Rc::new(Shared {
                view,
                backend,
                renderer: RefCell::new(Some(renderer)),
                update_parameters: RefCell::new(None),
            }),
        }
    }

    /// `render()`
    pub fn render(&self) {
        let shared = &self.shared;
        // onStyleImageMissing might be called during a render. The user
        // implemented method could trigger a call to update which overwrites
        // `updateParameters`. Copy the shared pointer here so that the
        // parameters aren't destroyed while `render(...)` is still using them.
        let Some(update_parameters) = shared.update_parameters.borrow().clone() else {
            return;
        };

        let _guard = BackendScope::new(&*shared.backend, ScopeType::Implicit);
        if let Some(renderer) = shared.renderer.borrow_mut().as_mut() {
            renderer.render(&update_parameters);
        }
    }

    /// `getRenderer()`: runs `f` with the renderer, if it still exists.
    pub fn with_renderer<R>(&self, f: impl FnOnce(&mut Renderer) -> R) -> Option<R> {
        self.shared.renderer.borrow_mut().as_mut().map(f)
    }
}

impl RendererFrontend for Frontend {
    fn reset(&self) {
        let renderer = self.shared.renderer.borrow_mut().take();
        drop(renderer);
    }

    fn set_observer(&self, observer: Weak<dyn RendererObserver>) {
        if let Some(renderer) = self.shared.renderer.borrow_mut().as_mut() {
            renderer.set_observer(Some(observer));
        }
    }

    fn update(&self, parameters: Arc<UpdateParameters>) {
        *self.shared.update_parameters.borrow_mut() = Some(parameters);
        self.shared.view.invalidate();
    }

    // TODO(port): getThreadPool() (RendererFrontend has no thread pool yet).
}
