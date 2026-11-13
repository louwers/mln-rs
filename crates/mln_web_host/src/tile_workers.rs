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
pub(crate) struct PortInstances {
    ports: Vec<web_sys::MessagePort>,
    _listeners: Vec<Closure<dyn FnMut(web_sys::MessageEvent)>>,
}

impl PortInstances {
    /// Instances behind `ports`, which must not be empty. Their replies go to
    /// `reply(instance, reply)`, and a panic of an instance to
    /// `fatal(instance, message)`, on this thread, as their messages arrive.
    pub(crate) fn new(
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

/// How this instance starts its tile workers' Web Workers.
#[derive(Default)]
struct Pool {
    /// How many to start; by default one fewer than the browser's logical
    /// processors, from one to four.
    count: Option<usize>,
    /// `spawn()` returns a new Web Worker (or what posts to one).
    spawn: Option<js_sys::Function>,
    /// The script of the Web Workers that `start` starts without `spawn`.
    script: Option<String>,
    /// The Web Workers, once the first map side connected.
    workers: Option<Vec<JsValue>>,
}

thread_local! {
    static POOL: std::cell::RefCell<Pool> = std::cell::RefCell::new(Pool::default());
}

/// Sets how this instance starts its tile workers' Web Workers, before the
/// first map side [`connect`]s: `count` of them (by default one fewer than
/// the browser's logical processors, from one to four), each from
/// `spawn()`, which returns a Web Worker, or else from the default script
/// ([`set_default_script`]). Each Web Worker gets a message
/// `{module, port}` for each map side that connects: `module` the
/// instance's `WebAssembly.Module`, which it instantiates with the first,
/// and `port` to serve with `mlnWebServeTileWorker`.
#[wasm_bindgen::prelude::wasm_bindgen(js_name = mlnWebConfigureTileWorkers)]
pub fn configure(count: Option<u32>, spawn: Option<js_sys::Function>) {
    POOL.with(|pool| {
        let mut pool = pool.borrow_mut();
        pool.count = count.map(|count| count as usize);
        pool.spawn = spawn;
    });
}

/// Sets the script of the Web Workers that the instance starts when
/// [`configure`] gave no `spawn`: a module script, at `url`, that serves
/// each `{module, port}` it gets as `configure` describes. For the package
/// that ships it beside the module's bindings.
pub fn set_default_script(url: String) {
    POOL.with(|pool| pool.borrow_mut().script = Some(url));
}

fn default_count() -> usize {
    let navigator = js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("navigator")).ok();
    let processors = navigator
        .and_then(|navigator| js_sys::Reflect::get(&navigator, &JsValue::from_str("hardwareConcurrency")).ok())
        .and_then(|count| count.as_f64())
        .unwrap_or(2.0) as usize;
    processors.saturating_sub(1).clamp(1, 4)
}

fn start(pool: &Pool) -> Result<Vec<JsValue>, String> {
    let count = pool.count.unwrap_or_else(default_count);
    if count == 0 {
        return Err("the instance was configured with no tile workers".to_owned());
    }
    (0..count)
        .map(|_| match (&pool.spawn, &pool.script) {
            (Some(spawn), _) => spawn.call0(&JsValue::NULL).map_err(|error| format!("spawn() threw {error:?}")),
            (None, Some(script)) => {
                let options = web_sys::WorkerOptions::new();
                options.set_type(web_sys::WorkerType::Module);
                web_sys::Worker::new_with_options(script, &options)
                    .map(JsValue::from)
                    .map_err(|error| format!("the tile worker script {script} didn't start: {error:?}"))
            }
            (None, None) => Err("no tile worker script: configure a `spawn`".to_owned()),
        })
        .collect()
}

/// Connects a new map side to each of this instance's tile workers' Web
/// Workers, which start with the first: one port to each, for
/// [`transport`].
pub fn connect() -> Result<Vec<web_sys::MessagePort>, String> {
    POOL.with(|pool| {
        let mut pool = pool.borrow_mut();
        if pool.workers.is_none() {
            pool.workers = Some(start(&pool)?);
        }
        let module = wasm_bindgen::module();
        let post = JsValue::from_str("postMessage");
        pool.workers
            .iter()
            .flatten()
            .map(|worker| {
                let channel = web_sys::MessageChannel::new().map_err(|error| format!("{error:?}"))?;
                let message = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&message, &JsValue::from_str("module"), &module);
                let _ = js_sys::Reflect::set(&message, &JsValue::from_str("port"), &channel.port2());
                let post: js_sys::Function =
                    js_sys::Reflect::get(worker, &post).map_err(|error| format!("{error:?}"))?.unchecked_into();
                post.call2(worker, &message, &js_sys::Array::of1(&channel.port2()))
                    .map_err(|error| format!("posting to a tile worker failed: {error:?}"))?;
                Ok(channel.port1())
            })
            .collect()
    })
}
