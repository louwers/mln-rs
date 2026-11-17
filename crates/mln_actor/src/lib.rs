//! Values that live on the thread that made them, behind `Send` handles.

mod local_object;

pub use local_object::LocalObject;
