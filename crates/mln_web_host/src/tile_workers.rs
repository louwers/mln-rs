//! The map side of tile workers in other Web Workers: `mln_renderer`'s
//! `WorkerInstances` over a `MessagePort` to each, whose other end a Web
//! Worker serves with [`tile_worker::serve`](crate::tile_worker::serve).
//!
//! A request goes as its bytes, transferred. A batch of replies comes back
//! as an array, each reply an array of `Uint8Array`s: its bytes, then its
//! payloads (pixels, vertices, indices), which the map takes as they are
//! (`mln_util::host_bytes`). A Web Worker whose instance panicked sends
//! `{fatal: message}` instead.

use std::rc::Rc;

use mln_renderer::tile::worker_codec::{Encoded, SerializingTransport, WorkerInstances};
use mln_util::host_bytes::HostBytes;
use mln_util::serialization::payload::Payload;
use wasm_bindgen::prelude::*;

/// A transport whose tile workers are in the Web Workers at the other end of
/// `ports` (at least one), for the renderer of this thread's current
/// `RunLoop`, which handles their replies (the transport's `ReplySink`).
/// `on_reply(instance)` is called as each reply arrives, and
/// `on_fatal(instance, message)` when an instance panicked.
pub fn transport(
    ports: Vec<web_sys::MessagePort>,
    on_reply: impl Fn(usize) + 'static,
    on_fatal: impl Fn(usize, JsValue) + 'static,
) -> Rc<SerializingTransport> {
    SerializingTransport::new(move |sink| {
        let reply = move |instance, reply| {
            on_reply(instance);
            sink.send(instance, reply);
        };
        Box::new(PortInstances::new(ports, reply, on_fatal))
    })
}

/// What a map side sends a port to disconnect it: the Web Worker drops the
/// port's tile workers.
const DISCONNECT: JsValue = JsValue::NULL;

/// Tile worker instances behind `MessagePort`s, instance `i` behind port `i`.
/// Dropping it disconnects them.
pub struct PortInstances {
    ports: Vec<web_sys::MessagePort>,
    _listeners: Vec<Closure<dyn FnMut(web_sys::MessageEvent)>>,
}

impl PortInstances {
    /// Instances behind `ports`, which must not be empty. Their replies go to
    /// `reply(instance, reply)`, and a panic of an instance to
    /// `fatal(instance, message)`, on this thread, as their messages arrive.
    pub fn new(
        ports: Vec<web_sys::MessagePort>,
        reply: impl Fn(usize, Encoded) + 'static,
        fatal: impl Fn(usize, JsValue) + 'static,
    ) -> PortInstances {
        let handlers = Rc::new((reply, fatal));
        let listeners = ports
            .iter()
            .enumerate()
            .map(|(instance, port)| {
                let handlers = handlers.clone();
                let listener = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |event: web_sys::MessageEvent| {
                    let data = event.data();
                    match data.dyn_into::<js_sys::Array>() {
                        Ok(batch) => {
                            for reply in batch.iter() {
                                (handlers.0)(instance, decode(&reply.unchecked_into()));
                            }
                        }
                        Err(data) => {
                            let message = js_sys::Reflect::get(&data, &JsValue::from_str("fatal")).unwrap_or(data);
                            (handlers.1)(instance, message);
                        }
                    }
                });
                port.set_onmessage(Some(listener.as_ref().unchecked_ref()));
                listener
            })
            .collect();
        PortInstances { ports, _listeners: listeners }
    }
}

/// A reply as a Web Worker sends it.
fn decode(reply: &js_sys::Array) -> Encoded {
    let mut parts = reply.iter().map(js_sys::Uint8Array::from);
    let bytes = parts.next().map(|bytes| bytes.to_vec()).unwrap_or_default();
    let payloads = parts.map(|part| Payload::Host(HostBytes::new(part))).collect();
    Encoded { bytes, payloads }
}

impl WorkerInstances for PortInstances {
    fn count(&self) -> usize {
        self.ports.len()
    }

    fn post(&self, instance: usize, request: Vec<u8>) {
        let bytes = js_sys::Uint8Array::from(&request[..]);
        let transfer = js_sys::Array::of1(&bytes.buffer());
        if let Err(error) = self.ports[instance].post_message_with_transferable(&bytes, &transfer) {
            web_sys::console::error_2(&JsValue::from_str("posting to a tile worker failed:"), &error);
        }
    }
}

impl Drop for PortInstances {
    fn drop(&mut self) {
        for port in &self.ports {
            port.set_onmessage(None);
            let _ = port.post_message(&DISCONNECT);
            port.close();
        }
    }
}
