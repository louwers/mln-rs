//! The side of a Web Worker that runs tile workers: its own wasm instance,
//! with its own memory, and a `TileWorkerHost` for each map side that
//! connected a port to it ([`tile_workers`](crate::tile_workers) describes
//! the messages). The hosts' workers (vector and GeoJSON tiles' layouts,
//! raster tiles' images and raster-dem tiles' DEMs) run on the instance's
//! cooperative scheduler, in the [`pump`](crate::pump)'s slices of 10 ms, so
//! that requests (a tile's removal, newer data) get in between them.
//!
//! The JavaScript that runs the Web Worker sends `{fatal: message}` over
//! each port instead when the instance panics, which it sees as an `error`
//! event: nothing may call into the instance after that.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use mln_actor::TaggedScheduler;
use mln_renderer::tile::worker_codec::{Encoded, TileWorkerHost};
use mln_util::identity::SimpleIdentity;
use mln_util::serialization::payload::Payload;
use parking_lot::Mutex;
use wasm_bindgen::prelude::*;

use crate::pump;

/// The tile workers of one port.
struct Connection {
    port: web_sys::MessagePort,
    /// Dropped before the replies it sends to.
    host: TileWorkerHost,
    replies: Arc<Mutex<Vec<Encoded>>>,
    _listener: Closure<dyn FnMut(web_sys::MessageEvent)>,
    _step: pump::Step,
}

thread_local! {
    static CONNECTIONS: RefCell<Vec<Rc<Connection>>> = const { RefCell::new(Vec::new()) };
}

/// Runs tile workers for the map side at the other end of `port`, until it
/// disconnects. Call it on the instance of a Web Worker, once for each port
/// that a map side sends it: in the Web Worker's script, after the module's
/// `init`.
#[wasm_bindgen(js_name = mlnWebServeTileWorker)]
pub fn serve(port: &web_sys::MessagePort) {
    // The Web Worker's script reports a panic's message from `mlnPanic`.
    crate::set_panic_hook();
    let scheduler = crate::install_cooperative_scheduler();
    scheduler.set_waker(Some(pump::waker()));
    pump::set_slice(Duration::from_millis(10));
    let replies = Arc::new(Mutex::new(Vec::new()));
    let workers = TaggedScheduler::new(scheduler, SimpleIdentity::new());
    let sink = Arc::clone(&replies);
    // A raster tile's reply comes once the browser decoded its image,
    // outside a slice.
    let host = TileWorkerHost::new(workers, move |reply| {
        sink.lock().push(reply);
        pump::wake();
    });
    let connection = Rc::new_cyclic(|weak: &std::rc::Weak<Connection>| {
        let connection = std::rc::Weak::clone(weak);
        let listener = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |event: web_sys::MessageEvent| {
            let Some(connection) = connection.upgrade() else { return };
            let data = event.data();
            if data.is_null() {
                connection.disconnect();
            } else {
                connection.host.handle(&js_sys::Uint8Array::new(&data).to_vec());
                pump::wake();
            }
        });
        port.set_onmessage(Some(listener.as_ref().unchecked_ref()));
        let connection = std::rc::Weak::clone(weak);
        let step = pump::add(move || {
            if let Some(connection) = connection.upgrade() {
                connection.send_replies();
            }
            true
        });
        Connection { port: port.clone(), host, replies, _listener: listener, _step: step }
    });
    CONNECTIONS.with(|connections| connections.borrow_mut().push(connection));
}

impl Connection {
    /// Sends the replies since the last batch, their payloads transferred.
    fn send_replies(&self) {
        let replies = std::mem::take(&mut *self.replies.lock());
        if replies.is_empty() {
            return;
        }
        let transfer = js_sys::Array::new();
        let batch: js_sys::Array = replies
            .into_iter()
            .map(|reply| {
                std::iter::once(reply.bytes)
                    .chain(reply.payloads.into_iter().map(Payload::into_vec))
                    .map(|part| {
                        let part = js_sys::Uint8Array::from(&part[..]);
                        transfer.push(&part.buffer());
                        part
                    })
                    .collect::<js_sys::Array>()
            })
            .collect();
        if let Err(error) = self.port.post_message_with_transferable(&batch, &transfer) {
            web_sys::console::error_2(&JsValue::from_str("posting tile worker replies failed:"), &error);
        }
    }

    fn disconnect(self: Rc<Self>) {
        self.port.set_onmessage(None);
        self.port.close();
        CONNECTIONS.with(|connections| connections.borrow_mut().retain(|other| !Rc::ptr_eq(other, &self)));
    }
}
