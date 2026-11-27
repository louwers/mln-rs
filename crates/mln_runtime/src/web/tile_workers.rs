//! The map side of tile workers in other Web Workers: `mln_engine`'s
//! `WorkerInstances` over a `MessagePort` to each, whose other end a Web
//! Worker serves with [`tile_worker::serve`](super::tile_worker::serve).
//!
//! A request goes as its bytes, transferred. A batch of replies comes back
//! as an array, each reply an array of `Uint8Array`s: its bytes, then its
//! payloads (pixels, vertices, indices), which the map takes as they are
//! (`mln_wire::host_bytes`). A Web Worker whose instance panicked sends
//! `{fatal: message}` instead: its jobs are lost, and a new Web Worker takes
//! its place if the map side can start one.

use std::rc::Rc;
use std::sync::Arc;

use mln_engine::Registry;
use mln_engine::worker::worker_codec::{Encoded, RemoteWorkers, ReplySink, WorkerInstances};
use mln_wire::host_bytes::HostBytes;
use mln_wire::payload::Payload;
use wasm_bindgen::prelude::*;

/// Tile workers in the Web Workers at the other end of `ports` (at least
/// one), which listen to their ports once a renderer takes the workers.
/// Their jobs name `registry`'s registrations, so each Web Worker must serve
/// with a registry filled by the same calls. `on_fatal(instance, message)`
/// is called when an instance panicked. `respawn(instance)` gives a port to
/// a new Web Worker in place of a lost one, if it can.
pub fn workers(
    ports: Vec<web_sys::MessagePort>,
    registry: Arc<Registry>,
    on_fatal: impl Fn(usize, JsValue) + 'static,
    respawn: impl Fn(usize) -> Option<web_sys::MessagePort> + 'static,
) -> RemoteWorkers {
    RemoteWorkers::new(registry, move |sink| {
        let handlers = Rc::new(Handlers { sink, on_fatal: Box::new(on_fatal) });
        Box::new(PortInstances::new(ports, handlers, Box::new(respawn)))
    })
}

/// What a map side sends a port to disconnect it: the Web Worker drops the
/// port's tile workers.
const DISCONNECT: JsValue = JsValue::NULL;

/// What the ports' messages go to.
struct Handlers {
    sink: ReplySink,
    on_fatal: Box<dyn Fn(usize, JsValue)>,
}

/// Tile worker instances behind `MessagePort`s, instance `i` behind port `i`.
/// Dropping it disconnects them.
struct PortInstances {
    ports: Vec<Port>,
    handlers: Rc<Handlers>,
    respawn: Box<dyn Fn(usize) -> Option<web_sys::MessagePort>>,
}

/// A port, and the listener that takes its messages.
struct Port {
    port: web_sys::MessagePort,
    _listener: Closure<dyn FnMut(web_sys::MessageEvent)>,
}

impl Port {
    /// Listens to `port`, as instance `instance`.
    fn new(instance: usize, port: web_sys::MessagePort, handlers: &Rc<Handlers>) -> Port {
        let handlers = Rc::clone(handlers);
        let listener = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |event: web_sys::MessageEvent| {
            let data = event.data();
            match data.dyn_into::<js_sys::Array>() {
                Ok(batch) => {
                    for reply in batch.iter() {
                        handlers.sink.send(instance, decode(&reply.unchecked_into()));
                    }
                }
                Err(data) => {
                    let message = js_sys::Reflect::get(&data, &JsValue::from_str("fatal")).unwrap_or(data);
                    (handlers.on_fatal)(instance, message);
                    handlers.sink.lost(instance);
                }
            }
        });
        port.set_onmessage(Some(listener.as_ref().unchecked_ref()));
        Port { port, _listener: listener }
    }

    fn disconnect(&self) {
        self.port.set_onmessage(None);
        let _ = self.port.post_message(&DISCONNECT);
        self.port.close();
    }
}

impl PortInstances {
    fn new(
        ports: Vec<web_sys::MessagePort>,
        handlers: Rc<Handlers>,
        respawn: Box<dyn Fn(usize) -> Option<web_sys::MessagePort>>,
    ) -> PortInstances {
        let ports =
            ports.into_iter().enumerate().map(|(instance, port)| Port::new(instance, port, &handlers)).collect();
        PortInstances { ports, handlers, respawn }
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

    fn post(&mut self, instance: usize, request: Vec<u8>) {
        let bytes = js_sys::Uint8Array::from(&request[..]);
        let transfer = js_sys::Array::of1(&bytes.buffer());
        if let Err(error) = self.ports[instance].port.post_message_with_transferable(&bytes, &transfer) {
            web_sys::console::error_2(&JsValue::from_str("posting to a tile worker failed:"), &error);
        }
    }

    fn replace(&mut self, instance: usize) -> bool {
        let Some(port) = (self.respawn)(instance) else {
            return false;
        };
        self.ports[instance].disconnect();
        self.ports[instance] = Port::new(instance, port, &self.handlers);
        true
    }
}

impl Drop for PortInstances {
    fn drop(&mut self) {
        for port in &self.ports {
            port.disconnect();
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
/// and `port` to serve with the module's `mlnWebServeTileWorker` export,
/// which calls [`tile_worker::serve`](super::tile_worker::serve).
#[wasm_bindgen::prelude::wasm_bindgen(js_name = mlnWebConfigureTileWorkers)]
pub fn configure(count: Option<u32>, spawn: Option<js_sys::Function>) {
    with_pool(|pool| {
        pool.count = count.map(|count| count as usize);
        pool.spawn = spawn;
    });
}

/// Sets the script of the Web Workers that the instance starts when
/// [`configure`] gave no `spawn`: a module script, at `url`, that serves
/// each `{module, port}` it gets as `configure` describes. For the package
/// that ships it beside the module's bindings.
pub fn set_default_script(url: String) {
    with_pool(|pool| pool.script = Some(url));
}

fn with_pool<R>(f: impl FnOnce(&mut Pool) -> R) -> R {
    POOL.with(|pool| f(&mut pool.borrow_mut()))
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
    (0..count).map(|_| start_one(pool)).collect()
}

fn start_one(pool: &Pool) -> Result<JsValue, String> {
    match (&pool.spawn, &pool.script) {
        (Some(spawn), _) => spawn.call0(&JsValue::NULL).map_err(|error| format!("spawn() threw {error:?}")),
        (None, Some(script)) => {
            let options = web_sys::WorkerOptions::new();
            options.set_type(web_sys::WorkerType::Module);
            web_sys::Worker::new_with_options(script, &options)
                .map(JsValue::from)
                .map_err(|error| format!("the tile worker script {script} didn't start: {error:?}"))
        }
        (None, None) => Err("no tile worker script: configure a `spawn`".to_owned()),
    }
}

/// A port to `worker` for a new map side.
fn port_to(worker: &JsValue) -> Result<web_sys::MessagePort, String> {
    let channel = web_sys::MessageChannel::new().map_err(|error| format!("{error:?}"))?;
    let message = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&message, &JsValue::from_str("module"), &wasm_bindgen::module());
    let _ = js_sys::Reflect::set(&message, &JsValue::from_str("port"), &channel.port2());
    let post: js_sys::Function = js_sys::Reflect::get(worker, &JsValue::from_str("postMessage"))
        .map_err(|error| format!("{error:?}"))?
        .unchecked_into();
    post.call2(worker, &message, &js_sys::Array::of1(&channel.port2()))
        .map_err(|error| format!("posting to a tile worker failed: {error:?}"))?;
    Ok(channel.port1())
}

/// Connects a new map side to each of this instance's tile workers' Web
/// Workers, which start with the first: one port to each, for
/// [`workers`].
pub fn connect() -> Result<Vec<web_sys::MessagePort>, String> {
    with_pool(|pool| {
        if pool.workers.is_none() {
            pool.workers = Some(start(pool)?);
        }
        pool.workers.iter().flatten().map(port_to).collect()
    })
}

/// Starts a Web Worker in place of the tile worker `instance`, which was
/// lost, and returns a port to it for a map side; the map sides that connect
/// later get it too.
pub fn respawn(instance: usize) -> Result<web_sys::MessagePort, String> {
    with_pool(|pool| {
        let worker = start_one(pool)?;
        let port = port_to(&worker)?;
        if let Some(slot) = pool.workers.as_mut().and_then(|workers| workers.get_mut(instance)) {
            *slot = worker;
        }
        Ok(port)
    })
}
