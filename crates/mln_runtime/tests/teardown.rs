// Dropping a runtime or a render session waits for the threads that it owns
// to end, and a drop on one of the library's threads doesn't wait there.

mod support;

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};

use httpmock::MockServer;
use mln_runtime::{AttachOptions, Map, RenderSession, RenderTarget, Runtime, RuntimeOptions};
use mln_util::size::Size;
use parking_lot::Mutex;
use support::{EMPTY_STYLE, TIMEOUT, eventually, map, wait};

/// The calling thread's name, and a flag that it sets once it has ended.
#[derive(Clone)]
struct Ended {
    thread: String,
    flag: Arc<AtomicBool>,
}

impl Ended {
    fn here() -> Ended {
        /// Sets its flag as the thread's locals are destroyed, which is
        /// after the thread's work and before a join of it returns.
        struct SetOnDrop(Ended);
        impl Drop for SetOnDrop {
            fn drop(&mut self) {
                self.0.flag.store(true, Ordering::SeqCst);
            }
        }
        thread_local! {
            static ENDED: RefCell<Option<SetOnDrop>> = const { RefCell::new(None) };
        }
        ENDED.with(|ended| {
            let mut ended = ended.borrow_mut();
            let thread = std::thread::current().name().unwrap_or("unnamed").to_owned();
            ended.get_or_insert_with(|| SetOnDrop(Ended { thread, flag: Arc::default() })).0.clone()
        })
    }

    fn has(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}

/// The runtime's thread.
fn runtime_thread(map: &Map) -> Ended {
    wait(map.with_core(|_| Ended::here()))
}

fn attach(map: &Map) -> RenderSession {
    let target = RenderTarget::Offscreen { size: Size::new(64, 64), device: None };
    let (session, attached) = map.attach(target, AttachOptions::default()).unwrap();
    attached.wait().unwrap();
    session
}

/// The session's thread, where it delivers the results of its work.
fn session_thread(session: &RenderSession) -> Ended {
    let (ended, here) = channel();
    session.send_then(RenderSession::barrier, move |_| ended.send(Ended::here()).unwrap()).unwrap();
    here.recv_timeout(TIMEOUT).unwrap()
}

#[test]
fn dropping_a_runtime_waits_for_its_threads() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method("GET").path("/style.json");
        then.status(200).body(EMPTY_STYLE);
    });
    let runtime = Runtime::new(&RuntimeOptions::default()).unwrap();
    // The header transform runs on the thread of the runtime's network file
    // source.
    let (network, network_thread) = channel();
    let network = Mutex::new(network);
    wait(runtime.set_http_header_transform(move |_, _| {
        let _ = network.lock().send(Ended::here());
        Vec::new()
    }));
    let map = map(&runtime);
    wait(map.set_style_url(format!("{}/style.json", server.base_url())));
    let threads = [runtime_thread(&map), network_thread.recv_timeout(TIMEOUT).unwrap()];
    drop(map);
    drop(runtime);
    for ended in threads {
        assert!(ended.has(), "the runtime's {} thread runs", ended.thread);
    }
}

#[test]
fn dropping_a_render_session_waits_for_its_thread() {
    let runtime = Runtime::new(&RuntimeOptions::default()).unwrap();
    let map = map(&runtime);
    let session = attach(&map);
    let ended = session_thread(&session);
    drop(session);
    assert!(ended.has(), "the session's thread runs");
}

#[test]
fn a_session_that_holds_the_last_map_has_its_thread_wait_for_the_runtime() {
    let runtime = Runtime::new(&RuntimeOptions::default()).unwrap();
    let map = map(&runtime);
    let session = attach(&map);
    let threads = [runtime_thread(&map), session_thread(&session)];
    drop(runtime);
    drop(map);
    // The session's thread lets go of the map, and of the runtime with it.
    drop(session);
    for ended in threads {
        assert!(ended.has(), "the {} thread runs", ended.thread);
    }
}

#[test]
fn a_runtime_dropped_on_its_own_thread_ends() {
    let runtime = Runtime::new(&RuntimeOptions::default()).unwrap();
    let map = map(&runtime);
    let ended = runtime_thread(&map);
    drop(runtime);
    // The runtime's thread waits until this thread has let go of the map,
    // then drops the map's last handle, and the runtime's with it.
    let (open, gate) = channel::<()>();
    let parked = map.with_core(move |_| gate.recv_timeout(TIMEOUT).unwrap()).unwrap();
    let last = map.clone();
    let dropped = map.with_core(move |_| drop(last)).unwrap();
    drop(map);
    open.send(()).unwrap();
    drop((parked, dropped));
    assert!(eventually(|| ended.has()), "the runtime's thread runs");
}

#[test]
fn a_session_dropped_in_its_own_callback_ends() {
    let runtime = Runtime::new(&RuntimeOptions::default()).unwrap();
    let map = map(&runtime);
    let session = attach(&map);
    let ended = session_thread(&session);
    // The session's thread waits in a callback until this thread has let go
    // of the session, then drops its last handle in the next one.
    let (open, gate) = channel::<()>();
    session.send_then(RenderSession::barrier, move |_| gate.recv_timeout(TIMEOUT).unwrap()).unwrap();
    let last = session.clone();
    session.send_then(RenderSession::barrier, move |_| drop(last)).unwrap();
    drop(session);
    open.send(()).unwrap();
    assert!(eventually(|| ended.has()), "the session's thread runs");
}

#[test]
fn dropping_a_released_runtime_doesnt_wait_for_its_thread() {
    /// Waits as it drops, on the runtime's thread, for this thread to go on
    /// past the drop of the runtime.
    struct WaitsForHost(Mutex<std::sync::mpsc::Receiver<()>>, Sender<bool>);
    impl Drop for WaitsForHost {
        fn drop(&mut self) {
            let _ = self.1.send(self.0.get_mut().recv_timeout(TIMEOUT).is_ok());
        }
    }
    let runtime = Runtime::new(&RuntimeOptions::default()).unwrap();
    let (go_on, gone_on) = channel();
    let (saw, saw_host) = channel();
    let blocker = WaitsForHost(Mutex::new(gone_on), saw);
    wait(runtime.set_event_handler(move |_| {
        let _ = &blocker;
    }));
    let released = runtime.release().unwrap();
    drop(runtime);
    go_on.send(()).unwrap();
    assert!(saw_host.recv_timeout(TIMEOUT).unwrap(), "the drop waited for the runtime's thread");
    released.wait().unwrap();
}
