//! `mln_render_session_set_frame_pacing()`: its options and its session, and
//! on the platforms with a display to follow, the frames it demands. The
//! runtime's tests cover how a pacer demands frames, and `mln_runtime::frame_pacer`'s
//! each platform's source.

use core::ffi::c_void;

use super::tests::{attach_options, demand, offscreen_target, release_session, rendered_session, sized};
use super::*;
use crate::generated::types::{
    MLN_STATUS_INVALID_ARGUMENT, MLN_STATUS_INVALID_STATE, MLN_STATUS_UNSUPPORTED, mln_map, mln_runtime,
};
use crate::map::tests::{create_map, options, release_map};
use crate::render_target::mln_map_attach;
use crate::runtime::tests::{Waiter, create as create_runtime, release as release_runtime};

const NO_DIAGNOSTIC: *mut mln_diagnostic = core::ptr::null_mut();

fn set_pacing(session: mln_render_session, options: Option<&mln_frame_pacing_options>) -> mln_status {
    let options = options.map_or(core::ptr::null(), core::ptr::from_ref);
    // SAFETY: the caller's options, and a null diagnostic.
    unsafe { mln_render_session_set_frame_pacing(session, options, NO_DIAGNOSTIC) }
}

fn pacing(fields: u32, maximum_frames_per_second: u32, view: *mut c_void) -> mln_frame_pacing_options {
    mln_frame_pacing_options { fields, maximum_frames_per_second, view, ..sized() }
}

fn barrier(session: mln_render_session) {
    let done = Waiter::new();
    // SAFETY: a local completion.
    let status = unsafe { mln_render_session_barrier(session, &raw const done.descriptor, NO_DIAGNOSTIC) };
    assert_eq!((status, done.status()), (MLN_STATUS_OK, MLN_STATUS_OK));
}

/// The frame results that wait.
fn drain(session: mln_render_session) -> Vec<mln_render_frame_result> {
    let mut batch = handles::NULL;
    let mut view = sized::<mln_render_frame_batch_view>();
    // SAFETY: local outputs.
    let status =
        unsafe { mln_render_session_drain_frame_results(session, &raw mut batch, &raw mut view, NO_DIAGNOSTIC) };
    assert_eq!(status, MLN_STATUS_OK);
    if batch == handles::NULL {
        return Vec::new();
    }
    // SAFETY: the batch's results, live until its release.
    let results = unsafe { core::slice::from_raw_parts(view.results, view.result_count) }.to_vec();
    mln_render_frame_batch_release(batch);
    results
}

/// A session that rendered its map, with what the map had to draw answered,
/// and no frame result waiting.
fn settled_session() -> (mln_runtime, mln_map, mln_render_session) {
    let (runtime, map, session) = rendered_session();
    // The redraw that the new session's map asks for comes before the
    // barrier, so the demand after answers it.
    barrier(session);
    assert_eq!(demand(session, 0, 9), MLN_STATUS_OK);
    barrier(session);
    assert_eq!(drain(session).iter().map(|result| result.token).collect::<Vec<_>>(), [9]);
    (runtime, map, session)
}

fn end(runtime: mln_runtime, map: mln_map, session: mln_render_session) {
    release_session(session);
    release_map(map);
    release_runtime(runtime);
}

#[test]
fn options_check_their_fields_limit_and_view() {
    let (runtime, map, session) = settled_session();
    let invalid = [
        pacing(MLN_FRAME_PACING_OPTION_MAXIMUM_FRAMES_PER_SECOND, 0, core::ptr::null_mut()),
        pacing(1 << 7, 30, core::ptr::null_mut()),
        pacing(MLN_FRAME_PACING_OPTION_VIEW, 0, core::ptr::null_mut()),
        mln_frame_pacing_options { size: 4, ..pacing(0, 0, core::ptr::null_mut()) },
    ];
    for options in invalid {
        assert_eq!(set_pacing(session, Some(&options)), MLN_STATUS_INVALID_ARGUMENT);
    }
    // A limit that its field doesn't set is no limit.
    let unset = pacing(0, 0, core::ptr::null_mut());
    let expected = if cfg!(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "tvos",
        target_os = "android",
        target_env = "ohos"
    )) {
        MLN_STATUS_OK
    } else {
        MLN_STATUS_UNSUPPORTED
    };
    // Android's pacer needs a looper, which the tests' threads don't have.
    if cfg!(not(target_os = "android")) {
        assert_eq!(set_pacing(session, Some(&unset)), expected);
    }
    end(runtime, map, session);
}

/// Only macOS, iOS and tvOS take a view, and only on the main thread, which
/// the tests don't run on.
#[test]
fn a_view_needs_an_apple_platform_and_its_main_thread() {
    let (runtime, map, session) = settled_session();
    let mut view = 0_u8;
    let options = pacing(MLN_FRAME_PACING_OPTION_VIEW, 0, (&raw mut view).cast());
    let apple = cfg!(any(target_os = "macos", target_os = "ios", target_os = "tvos"));
    let expected = if apple { MLN_STATUS_INVALID_STATE } else { MLN_STATUS_UNSUPPORTED };
    assert_eq!(set_pacing(session, Some(&options)), expected);
    end(runtime, map, session);
}

unsafe extern "C" fn ignore_redraw(_user_data: *mut c_void) {}

#[test]
fn set_frame_pacing_checks_its_session() {
    let (runtime, map, session) = settled_session();
    // Null options stop pacing, which a session that isn't paced takes too.
    assert_eq!(set_pacing(session, None), MLN_STATUS_OK);
    assert_eq!(set_pacing(handles::NULL, None), MLN_STATUS_INVALID_ARGUMENT);
    end(runtime, map, session);
    assert_eq!(set_pacing(session, None), MLN_STATUS_INVALID_STATE);

    let runtime = create_runtime();
    let map = create_map(runtime, &options(1.0));
    let target = offscreen_target(16, 16);
    let mut options = attach_options();
    options.redraw_wake.callback = Some(ignore_redraw);
    let mut session = handles::NULL;
    let attached = Waiter::new();
    // SAFETY: local arguments.
    let status = unsafe {
        mln_map_attach(
            map,
            &raw const target,
            0,
            &raw const options,
            &raw mut session,
            &raw const attached.descriptor,
            NO_DIAGNOSTIC,
        )
    };
    assert_eq!((status, attached.status()), (MLN_STATUS_OK, MLN_STATUS_OK));
    // A session with a redraw wake takes no pacing, nor its removal.
    assert_eq!(set_pacing(session, None), MLN_STATUS_INVALID_STATE);
    end(runtime, map, session);
}

// MARK: - The display's frames

/// The platforms with a display to follow from any thread.
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos", target_env = "ohos"))]
mod display {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::generated::types::mln_completion_result;
    use crate::map::mln_map_set_style_json;
    use crate::map::tests::view;
    use crate::runtime::tests::channel_completion;

    const TIMEOUT: Duration = Duration::from_secs(30);

    unsafe extern "C" fn send_update_generation(user_data: *mut c_void, result: *const mln_completion_result) {
        // SAFETY: the sender that `channel_completion` boxed, and the library's
        // result.
        unsafe {
            let sender = &*user_data.cast::<std::sync::mpsc::Sender<(mln_status, u64)>>();
            let _ = sender.send(((*result).status, (*result).render_update_generation));
        }
    }

    /// Gives `map` a background of `color`, and returns the map update
    /// generation of the frames that show it.
    fn paint(map: mln_map, color: &str) -> u64 {
        let style = format!(
            r#"{{"version":8,"sources":{{}},"layers":[{{"id":"bg","type":"background","paint":{{"background-color":"{color}"}}}}]}}"#
        );
        let (completion, receiver) = channel_completion::<(mln_status, u64)>(send_update_generation);
        // SAFETY: local arguments, and a completion whose sender the library
        // releases after its call.
        let status = unsafe { mln_map_set_style_json(map, view(&style), &raw const completion, NO_DIAGNOSTIC) };
        assert_eq!(status, MLN_STATUS_OK);
        let (status, generation) = receiver.recv_timeout(TIMEOUT).unwrap();
        assert_eq!(status, MLN_STATUS_OK);
        generation
    }

    /// Waits for a frame that the pacing demanded and that shows the map update
    /// `generation`, skipping the frames before it.
    fn wait_for_paced_frame(session: mln_render_session, generation: u64) {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            for result in drain(session) {
                assert_eq!(result.token, 0, "only the pacing demands frames");
                if result.disposition == MLN_RENDER_RESULT_RENDERED && result.map_update_generation >= generation {
                    return;
                }
            }
            assert!(Instant::now() < deadline, "no paced frame showed update {generation}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// The display paces a session, and a call that fails leaves its pacing.
    #[test]
    fn the_display_paces_a_session() {
        for options in
            [None, Some(pacing(MLN_FRAME_PACING_OPTION_MAXIMUM_FRAMES_PER_SECOND, 30, core::ptr::null_mut()))]
        {
            let (runtime, map, session) = settled_session();
            let options = options.unwrap_or_else(|| pacing(0, 0, core::ptr::null_mut()));
            assert_eq!(set_pacing(session, Some(&options)), MLN_STATUS_OK);
            wait_for_paced_frame(session, paint(map, "red"));
            let invalid = pacing(MLN_FRAME_PACING_OPTION_MAXIMUM_FRAMES_PER_SECOND, 0, core::ptr::null_mut());
            assert_eq!(set_pacing(session, Some(&invalid)), MLN_STATUS_INVALID_ARGUMENT);
            wait_for_paced_frame(session, paint(map, "blue"));
            // Without pacing, nothing demands a frame.
            assert_eq!(set_pacing(session, None), MLN_STATUS_OK);
            barrier(session);
            drain(session);
            paint(map, "green");
            barrier(session);
            std::thread::sleep(Duration::from_millis(100));
            assert!(drain(session).is_empty());
            end(runtime, map, session);
        }
    }
}
