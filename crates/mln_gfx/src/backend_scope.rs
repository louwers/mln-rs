// Ports include/mln/gfx/backend_scope.hpp and src/mln/renderer/backend_scope.cpp
//
// The C++ scopes form a thread-local doubly linked list and re-activate the
// prior scope's backend when a nested scope for a *different* backend ends.
// TODO(port): that cross-backend hand-over (deactivate prior / re-activate
// on drop) needs a reference to the prior backend; the port tracks only the
// backend ids, so nesting different backends does not toggle the outer one.
// Nesting the same backend (the common case) behaves as in C++: activated
// once, deactivated once.

use std::cell::RefCell;

use mln_util::identity::SimpleIdentity;

use crate::renderer_backend::RendererBackend;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeType {
    /// The windowing system already activated the backend.
    Implicit,
    /// The scope activates the backend, and deactivates it when dropped.
    Explicit,
}

thread_local! {
    static CURRENT_SCOPES: RefCell<Vec<SimpleIdentity>> = const { RefCell::new(Vec::new()) };
}

pub struct BackendScope<'a> {
    backend: &'a dyn RendererBackend,
    activated: bool,
}

impl<'a> BackendScope<'a> {
    pub fn new(backend: &'a dyn RendererBackend, scope_type: ScopeType) -> Self {
        let id = backend.unique_id();
        let prior_same = CURRENT_SCOPES.with(|s| s.borrow().last() == Some(&id));
        let mut scope = BackendScope { backend, activated: false };
        // Only activate when set to Explicit and only once per RendererBackend
        if scope_type == ScopeType::Explicit && !prior_same {
            scope.backend.activate();
            scope.activated = true;
        }
        CURRENT_SCOPES.with(|s| s.borrow_mut().push(id));
        scope
    }

    /// Returns true when there is currently a `BackendScope` active in this thread.
    #[must_use]
    pub fn exists() -> bool {
        CURRENT_SCOPES.with(|s| !s.borrow().is_empty())
    }
}

impl Drop for BackendScope<'_> {
    fn drop(&mut self) {
        CURRENT_SCOPES.with(|s| s.borrow_mut().pop());
        if self.activated {
            self.backend.deactivate();
            self.activated = false;
        }
    }
}
