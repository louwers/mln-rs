//! The C API's pacers on the platform's `CADisplayLink`, which
//! `mln_frame_pacer` follows on macOS, iOS and tvOS.

#[cfg(all(test, any(target_os = "macos", target_os = "ios", target_os = "tvos")))]
mod tests {
    use crate::frame_pacer::mln_frame_pacer_create;
    use crate::frame_pacer::tests::expect_paced_frames;
    use crate::generated::types::{
        MLN_FRAME_PACER_OPTION_MAXIMUM_FRAMES_PER_SECOND, MLN_STATUS_OK, mln_frame_pacer, mln_frame_pacer_options,
    };
    use crate::render_session::tests::sized;

    fn create(options: Option<&mln_frame_pacer_options>) -> mln_frame_pacer {
        let options = options.map_or(core::ptr::null(), core::ptr::from_ref);
        let mut pacer = crate::handles::NULL;
        // SAFETY: the caller's options, a local out-handle and a null
        // diagnostic.
        let status = unsafe { mln_frame_pacer_create(options, &raw mut pacer, core::ptr::null_mut()) };
        assert_eq!(status, MLN_STATUS_OK);
        pacer
    }

    #[test]
    fn a_display_link_paces_a_session() {
        expect_paced_frames(create(None), false);
    }

    #[test]
    fn a_released_display_link_pacer_keeps_pacing_its_sessions() {
        expect_paced_frames(create(None), true);
    }

    /// The limit also sets the display link's preferred frame rate range.
    #[test]
    fn a_display_link_with_a_frame_rate_limit_paces_a_session() {
        let options = mln_frame_pacer_options {
            fields: MLN_FRAME_PACER_OPTION_MAXIMUM_FRAMES_PER_SECOND,
            maximum_frames_per_second: 30,
            ..sized()
        };
        expect_paced_frames(create(Some(&options)), false);
    }
}
