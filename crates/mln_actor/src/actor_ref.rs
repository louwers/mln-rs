// C++ `ActorRef<O>` is `(O* object, std::weak_ptr<Mailbox>)`; `invoke(fn,
// args...)` pushes a message calling `object->*fn(args...)`. Here the target
// is found through the mailbox (see mailbox.rs) and a message is a closure
// taking `&O` (targets that mutate use interior mutability, since messages
// can run while other code on the same thread holds the target).

use std::any::Any;
use std::marker::PhantomData;
use std::sync::{Arc, Weak};

use crate::mailbox::Mailbox;

/// A non-owning, `Send` reference to an actor's object of type `O`.
/// Sending to a reference whose actor is gone does nothing.
pub struct ActorRef<O: 'static> {
    weak_mailbox: Weak<Mailbox>,
    _object: PhantomData<fn(&O)>,
}

impl<O: 'static> Clone for ActorRef<O> {
    fn clone(&self) -> Self {
        ActorRef { weak_mailbox: Weak::clone(&self.weak_mailbox), _object: PhantomData }
    }
}

impl<O: 'static> ActorRef<O> {
    /// A reference that sends to `mailbox`, whose target is the object.
    pub fn new(mailbox: &Arc<Mailbox>) -> Self {
        ActorRef { weak_mailbox: Arc::downgrade(mailbox), _object: PhantomData }
    }

    /// `invoke(fn, args...)`.
    pub fn invoke(&self, f: impl FnOnce(&O) + Send + 'static) {
        if let Some(mailbox) = self.weak_mailbox.upgrade() {
            mailbox.push(Box::new(move |object: &dyn Any| {
                if let Some(object) = object.downcast_ref::<O>() {
                    f(object);
                }
            }));
        }
    }
}
