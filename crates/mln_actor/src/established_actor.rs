// Ports include/mln/actor/established_actor.hpp and aspiring_actor.hpp.
//
// `AspiringActor<O>` is the mailbox that exists before the object (messages
// queue in it); `EstablishedActor<O>` constructs the object on the thread
// that will own it, opens the mailbox there, and on drop closes the mailbox
// before the object goes away.
//
// TODO(port): `Actor<O>` (actor.hpp) on a thread pool scheduler: here the
// object stays on the thread that established it (see mailbox.rs).

use std::any::Any;
use std::rc::Rc;
use std::sync::Arc;

use crate::actor_ref::ActorRef;
use crate::mailbox::Mailbox;
use crate::scheduler::SchedulerPtr;

/// `AspiringActor<O>`: the mailbox and the `ActorRef` handed out before the
/// object exists.
pub struct AspiringActor<O: 'static> {
    pub(crate) mailbox: Arc<Mailbox>,
    _object: std::marker::PhantomData<fn(&O)>,
}

impl<O: 'static> Default for AspiringActor<O> {
    fn default() -> Self {
        Self::new()
    }
}

impl<O: 'static> AspiringActor<O> {
    #[must_use]
    pub fn new() -> Self {
        Self::with_mailbox(Mailbox::new())
    }

    pub(crate) fn with_mailbox(mailbox: Arc<Mailbox>) -> Self {
        AspiringActor { mailbox, _object: std::marker::PhantomData }
    }

    /// `self()`.
    #[must_use]
    pub fn self_ref(&self) -> ActorRef<O> {
        ActorRef::new(&self.mailbox)
    }
}

/// `EstablishedActor<O>`: owns the object on its thread.
pub struct EstablishedActor<O: 'static> {
    mailbox: Arc<Mailbox>,
    object: Option<Rc<O>>,
}

impl<O: 'static> EstablishedActor<O> {
    /// `EstablishedActor(Scheduler&, AspiringActor<O>& parent, args...)`:
    /// `make` gets the actor's own reference, as an `O(ActorRef<O>, ...)`
    /// constructor does.
    pub fn new(scheduler: &SchedulerPtr, parent: &AspiringActor<O>, make: impl FnOnce(ActorRef<O>) -> O) -> Self {
        let object = Rc::new(make(parent.self_ref()));
        parent.mailbox.open(scheduler, object.clone() as Rc<dyn Any>);
        EstablishedActor { mailbox: parent.mailbox.clone(), object: Some(object) }
    }

    #[must_use]
    pub fn object(&self) -> &O {
        self.object.as_ref().expect("established actor object")
    }
}

impl<O: 'static> Drop for EstablishedActor<O> {
    fn drop(&mut self) {
        self.mailbox.close();
        drop(self.object.take());
    }
}
