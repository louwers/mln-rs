//! The projections that a host registers in `mln_runtime_options`: each is a
//! [`Crs`] of its runtime's registry, whose converter calls the host's
//! callbacks.

use core::ffi::c_void;
use std::sync::Arc;

use mln_style::crs::{Crs, CrsConverter, TileMatrixSet};

use crate::callback::Registration;
use crate::error::Error;
use crate::generated::types::{mln_lat_lng, mln_projected_point, mln_registered_projection, mln_tile_matrix_set};
use crate::{abi, buffer, handles, process_exit};

abi::sized_element!(mln_registered_projection);

// The converter hands the host a CRS's positions in place.
const _: () = assert!(
    size_of::<mln_projected_point>() == size_of::<[f64; 2]>()
        && align_of::<mln_projected_point>() == align_of::<[f64; 2]>()
        && core::mem::offset_of!(mln_projected_point, x) == 0
        && core::mem::offset_of!(mln_projected_point, y) == size_of::<f64>()
);

type ForwardCallback = unsafe extern "C" fn(*mut c_void, *const mln_lat_lng, usize, *mut mln_projected_point);
type InverseCallback = unsafe extern "C" fn(*mut c_void, *const mln_projected_point, usize, *mut mln_lat_lng);

/// The registrations of the host's converters, for a runtime that is being
/// created: the caller keeps their user data until
/// [`accept`](Self::accept).
#[derive(Default)]
pub(crate) struct Converters(Vec<Arc<Registration>>);

impl Converters {
    /// Hands the converters' user data to the library, which releases each
    /// once nothing converts in its projection any more.
    pub(crate) fn accept(&self) {
        for registration in &self.0 {
            registration.accept();
        }
    }
}

/// Checks a caller's `count` projections at `projections`, and makes a CRS of
/// each, in order, whose converter calls the projection's callbacks.
///
/// # Safety
///
/// As for [`abi::sized_elements`], and each projection's `name` is a valid
/// view.
pub(crate) unsafe fn copy(
    projections: *const mln_registered_projection,
    count: usize,
) -> Result<(Vec<Crs>, Converters), Error> {
    // SAFETY: the caller's contract.
    let projections =
        unsafe { abi::sized_elements(projections, count, "projection", "projections", "projection_count") }?;
    let (mut crss, mut converters) = (Vec::with_capacity(projections.len()), Converters::default());
    for (index, projection) in projections.iter().enumerate() {
        // SAFETY: the caller's contract.
        let name = unsafe { buffer::text(projection.name, &format!("the name of projection {index}")) }?;
        let converter = projection.converter;
        let (Some(forward), Some(inverse)) = (converter.forward, converter.inverse) else {
            return Err(Error::invalid_argument(format!(
                "projection {index} needs a converter with both forward and inverse"
            )));
        };
        let registration = Arc::new(Registration::new(
            "projection converter",
            true,
            converter.user_data,
            converter.release_user_data,
            handles::NULL,
        )?);
        let mln_tile_matrix_set { origin, extent_at_zoom0 } = projection.tile_matrix_set;
        crss.push(Crs {
            name: name.to_owned(),
            converter: Box::new(HostConverter { forward, inverse, registration: Arc::clone(&registration) }),
            tile_matrix_set: TileMatrixSet { origin: [origin.x, origin.y], extent_at_zoom0 },
        });
        converters.0.push(registration);
    }
    Ok((crss, converters))
}

/// A converter that calls the host's callbacks, with the host's user data,
/// which its registration releases once the converter is dropped. A position
/// that the host leaves unwritten has no answer, nor does any once the
/// process exits.
struct HostConverter {
    forward: ForwardCallback,
    inverse: InverseCallback,
    registration: Arc<Registration>,
}

impl CrsConverter for HostConverter {
    fn forward(&self, lng_lats: &[[f64; 2]], positions: &mut [[f64; 2]]) {
        positions.fill([f64::NAN; 2]);
        let count = lng_lats.len().min(positions.len());
        if count == 0 || process_exit::exiting() {
            return;
        }
        let coordinates: Vec<mln_lat_lng> =
            lng_lats[..count].iter().map(|&[longitude, latitude]| mln_lat_lng { latitude, longitude }).collect();
        // SAFETY: the host's callback with its user data, `count`
        // coordinates, and room for as many points, which `positions` lays
        // out as `mln_projected_point`s (asserted above).
        unsafe {
            (self.forward)(
                self.registration.user_data().get(),
                coordinates.as_ptr(),
                count,
                positions.as_mut_ptr().cast(),
            );
        }
    }

    fn inverse(&self, positions: &[[f64; 2]], lng_lats: &mut [[f64; 2]]) {
        lng_lats.fill([f64::NAN; 2]);
        let count = positions.len().min(lng_lats.len());
        if count == 0 || process_exit::exiting() {
            return;
        }
        let mut coordinates = vec![mln_lat_lng { latitude: f64::NAN, longitude: f64::NAN }; count];
        // SAFETY: the host's callback with its user data, `count` points,
        // which `positions` lays out as `mln_projected_point`s (asserted
        // above), and room for as many coordinates.
        unsafe {
            (self.inverse)(
                self.registration.user_data().get(),
                positions.as_ptr().cast(),
                count,
                coordinates.as_mut_ptr(),
            );
        }
        for (lng_lat, coordinate) in lng_lats.iter_mut().zip(&coordinates) {
            *lng_lat = [coordinate.longitude, coordinate.latitude];
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;
    use crate::diagnostic::tests::{diagnostic, message};
    use crate::generated::types::{
        MLN_STATUS_INVALID_ARGUMENT, MLN_STATUS_OK, mln_buffer_view, mln_diagnostic, mln_map, mln_map_projection,
        mln_projection_converter, mln_runtime, mln_runtime_options, mln_screen_point, mln_status,
    };
    use crate::map::mln_map_set_style_json;
    use crate::map::tests::{create_map, options, release_map, view};
    use crate::projection::{
        mln_map_projection_create, mln_map_projection_lat_lng_for_pixel, mln_map_projection_pixel_for_lat_lng,
        mln_map_projection_release,
    };
    use crate::runtime::mln_runtime_create;
    use crate::runtime::tests::{Waiter, release as release_runtime};

    const NO_DIAGNOSTIC: *mut mln_diagnostic = core::ptr::null_mut();

    /// What a test's converter saw: its calls, and its release, which sends
    /// on `released`. The library holds a reference of its own as the
    /// converter's user data.
    struct Probe {
        calls: AtomicUsize,
        released: mpsc::Sender<()>,
    }

    impl Probe {
        fn new() -> (Arc<Probe>, mpsc::Receiver<()>) {
            let (released, receiver) = mpsc::channel();
            (Arc::new(Probe { calls: AtomicUsize::new(0), released }), receiver)
        }
    }

    /// The probe of a converter's user data.
    ///
    /// # Safety
    ///
    /// `user_data` is a reference that [`projection`] leaked.
    unsafe fn probe<'a>(user_data: *mut c_void) -> &'a Probe {
        // SAFETY: the caller's contract.
        unsafe { &*user_data.cast::<Probe>() }
    }

    /// Writes each coordinate's longitude and latitude as its point, as the
    /// built-in `identity` does.
    unsafe extern "C" fn plane_forward(
        user_data: *mut c_void,
        coordinates: *const mln_lat_lng,
        count: usize,
        out_points: *mut mln_projected_point,
    ) {
        // SAFETY: the probe that `projection` leaked, and the library's
        // arrays of `count` items.
        unsafe {
            probe(user_data).calls.fetch_add(1, Ordering::SeqCst);
            let coordinates = core::slice::from_raw_parts(coordinates, count);
            let points = core::slice::from_raw_parts_mut(out_points, count);
            for (point, coordinate) in points.iter_mut().zip(coordinates) {
                *point = mln_projected_point { x: coordinate.longitude, y: coordinate.latitude };
            }
        }
    }

    /// The inverse of [`plane_forward`].
    unsafe extern "C" fn plane_inverse(
        user_data: *mut c_void,
        points: *const mln_projected_point,
        count: usize,
        out_coordinates: *mut mln_lat_lng,
    ) {
        // SAFETY: as for `plane_forward`.
        unsafe {
            probe(user_data).calls.fetch_add(1, Ordering::SeqCst);
            let points = core::slice::from_raw_parts(points, count);
            let coordinates = core::slice::from_raw_parts_mut(out_coordinates, count);
            for (coordinate, point) in coordinates.iter_mut().zip(points) {
                *coordinate = mln_lat_lng { latitude: point.y, longitude: point.x };
            }
        }
    }

    /// Answers only the first of its coordinates.
    unsafe extern "C" fn first_only(
        _: *mut c_void,
        coordinates: *const mln_lat_lng,
        count: usize,
        out_points: *mut mln_projected_point,
    ) {
        if count > 0 {
            // SAFETY: the library's arrays of `count` items.
            unsafe {
                let coordinate = *coordinates;
                *out_points = mln_projected_point { x: coordinate.longitude, y: coordinate.latitude };
            }
        }
    }

    unsafe extern "C" fn release(user_data: *mut c_void) {
        // SAFETY: the reference that `projection` leaked, which the library
        // releases once.
        let probe = unsafe { Arc::from_raw(user_data.cast::<Probe>()) };
        let released = probe.released.clone();
        // Before the test hears of it, which then holds the last reference.
        drop(probe);
        let _ = released.send(());
    }

    /// The identity's tile matrix set: tile 0/0/0 spans -90 to 90 on both
    /// axes.
    const PLANE: mln_tile_matrix_set =
        mln_tile_matrix_set { origin: mln_projected_point { x: -90.0, y: 90.0 }, extent_at_zoom0: 180.0 };

    /// A projection named `name` whose converter is [`plane_forward`] and
    /// [`plane_inverse`] and reports to `probe`, which it holds a reference
    /// to as its user data.
    fn projection(name: &'static str, probe: &Arc<Probe>) -> mln_registered_projection {
        mln_registered_projection {
            size: size_of::<mln_registered_projection>() as u32,
            name: view(name),
            tile_matrix_set: PLANE,
            converter: mln_projection_converter {
                forward: Some(plane_forward),
                inverse: Some(plane_inverse),
                user_data: Arc::into_raw(Arc::clone(probe)).cast_mut().cast(),
                release_user_data: Some(release),
            },
        }
    }

    /// Takes back the reference that a refused projection's user data
    /// holds, which the library left with the caller.
    fn reclaim(projection: &mln_registered_projection) {
        // SAFETY: the reference that `projection` leaked, which the library
        // didn't release.
        drop(unsafe { Arc::from_raw(projection.converter.user_data.cast::<Probe>()) });
    }

    /// A runtime of `projections`, or the status and message of the refusal.
    fn create(
        projections: *const mln_registered_projection,
        count: usize,
    ) -> Result<mln_runtime, (mln_status, String)> {
        // SAFETY: zeros are valid options, which the fields below complete.
        let mut options: mln_runtime_options = unsafe { core::mem::zeroed() };
        options.size = size_of::<mln_runtime_options>() as u32;
        options.projections = projections;
        options.projection_count = count;
        let mut runtime = 0;
        let mut out = diagnostic();
        // SAFETY: local options, output and diagnostic, and the caller's
        // projections.
        match unsafe { mln_runtime_create(&raw const options, &raw mut runtime, &raw mut *out) } {
            MLN_STATUS_OK => Ok(runtime),
            status => {
                assert_eq!(runtime, 0);
                Err((status, message(&out).to_owned()))
            }
        }
    }

    fn set_style(map: mln_map, json: &str) {
        let styled = Waiter::new();
        // SAFETY: local arguments.
        let status = unsafe { mln_map_set_style_json(map, view(json), &raw const styled.descriptor, NO_DIAGNOSTIC) };
        assert_eq!(status, MLN_STATUS_OK);
        assert_eq!(styled.status(), MLN_STATUS_OK);
    }

    fn map_projection(map: mln_map) -> mln_map_projection {
        let mut projection = handles::NULL;
        // SAFETY: a local output and a null diagnostic.
        let status = unsafe { mln_map_projection_create(map, core::ptr::null(), &raw mut projection, NO_DIAGNOSTIC) };
        assert_eq!(status, MLN_STATUS_OK);
        projection
    }

    fn pixel(projection: mln_map_projection, latitude: f64, longitude: f64) -> mln_screen_point {
        let mut point = mln_screen_point { x: f64::NAN, y: f64::NAN };
        let coordinate = mln_lat_lng { latitude, longitude };
        // SAFETY: a local output and a null diagnostic.
        let status =
            unsafe { mln_map_projection_pixel_for_lat_lng(projection, coordinate, &raw mut point, NO_DIAGNOSTIC) };
        assert_eq!(status, MLN_STATUS_OK);
        point
    }

    fn style(projection: &str) -> String {
        format!(
            r#"{{"version":8,"projection":{{"type":"{projection}"}},"center":[10,20],"zoom":2,"sources":{{}},"layers":[]}}"#
        )
    }

    // GL JS: a style draws in the projection that addProjection registered
    // under its `projection.type`.
    #[test]
    fn a_map_draws_in_a_projection_that_its_runtime_registers() {
        let (probe, released) = Probe::new();
        let registered = [projection("test:plane", &probe)];
        let runtime = create(registered.as_ptr(), 1).unwrap();
        let (plane, identity) = (create_map(runtime, &options(1.0)), create_map(runtime, &options(1.0)));
        set_style(plane, &style("test:plane"));
        set_style(identity, &style("identity"));
        let (plane_projection, identity_projection) = (map_projection(plane), map_projection(identity));

        // The host's plane converts as the built-in identity does, through
        // the host's callbacks.
        let calls = probe.calls.load(Ordering::SeqCst);
        let point = pixel(plane_projection, 30.0, 40.0);
        assert!(probe.calls.load(Ordering::SeqCst) > calls);
        let expected = pixel(identity_projection, 30.0, 40.0);
        assert!((point.x - expected.x).abs() < 1e-9 && (point.y - expected.y).abs() < 1e-9, "{point:?} {expected:?}");
        let mut back = mln_lat_lng { latitude: f64::NAN, longitude: f64::NAN };
        // SAFETY: a local output and a null diagnostic.
        let status =
            unsafe { mln_map_projection_lat_lng_for_pixel(plane_projection, point, &raw mut back, NO_DIAGNOSTIC) };
        assert_eq!(status, MLN_STATUS_OK);
        assert!((back.latitude - 30.0).abs() < 1e-9 && (back.longitude - 40.0).abs() < 1e-9, "{back:?}");

        // The converter's user data stays the library's until the runtime,
        // its maps and their projections are released, and then goes once.
        for projection in [plane_projection, identity_projection] {
            mln_map_projection_release(projection);
        }
        release_map(plane);
        release_map(identity);
        assert_eq!(released.try_recv(), Err(mpsc::TryRecvError::Empty));
        release_runtime(runtime);
        released.recv_timeout(Duration::from_secs(30)).expect("the converter's user data is released");
        assert_eq!(Arc::strong_count(&probe), 1);
    }

    /// An edit that makes creation refuse a pair of projections.
    type Refusal = fn(&mut [mln_registered_projection; 2]);

    // GL JS: addProjection refuses a name without a colon or one that is
    // taken, and a zoom 0 extent that isn't positive.
    #[test]
    fn creation_refuses_a_projection_and_leaves_every_user_data_with_the_caller() {
        let (probe, released) = Probe::new();
        let refusals: [(Refusal, &str); 7] = [
            (|p| p[1].name = view("plane"), "projection 1: the projection name \"plane\" needs a colon"),
            (|p| p[1].name = view("test:plane"), "projection 1: \"test:plane\" is already registered"),
            (|p| p[1].tile_matrix_set.origin.y = f64::NAN, "positive, finite extent"),
            (|p| p[1].tile_matrix_set.extent_at_zoom0 = 0.0, "positive, finite extent"),
            (|p| p[1].converter.inverse = None, "projection 1 needs a converter with both forward and inverse"),
            (|p| p[1].size -= 8, "projection 1 has an invalid size"),
            (|p| p[1].name = mln_buffer_view { data: core::ptr::null(), size: 1 }, "the name of projection 1 is null"),
        ];
        for (refuse, expected) in refusals {
            let mut projections = [projection("test:plane", &probe), projection("test:other", &probe)];
            refuse(&mut projections);
            let (status, message) = create(projections.as_ptr(), 2).unwrap_err();
            assert_eq!(status, MLN_STATUS_INVALID_ARGUMENT, "{message}");
            assert!(message.contains(expected), "{message}");
            projections.iter().for_each(reclaim);
        }
        let (status, message) = create(core::ptr::null(), 1).unwrap_err();
        assert_eq!(status, MLN_STATUS_INVALID_ARGUMENT);
        assert!(message.contains("projections must not be null"), "{message}");
        assert_eq!(released.try_recv(), Err(mpsc::TryRecvError::Empty));
        assert_eq!(Arc::strong_count(&probe), 1);
    }

    #[test]
    fn a_converter_answers_in_place_and_leaves_what_the_host_skips_without_an_answer() {
        let (probe, released) = Probe::new();
        let mut first = projection("test:first", &probe);
        first.converter.forward = Some(first_only);
        // SAFETY: a local projection, whose name is static.
        let (crss, converters) = unsafe { copy(&raw const first, 1) }.unwrap();
        let [crs] = &crss[..] else { panic!("one CRS") };
        assert_eq!(crs.tile_matrix_set, TileMatrixSet { origin: [-90.0, 90.0], extent_at_zoom0: 180.0 });

        let mut positions = [[0.0; 2]; 2];
        crs.converter.forward(&[[10.0, 20.0], [30.0, 40.0]], &mut positions);
        assert_eq!(positions[0], [10.0, 20.0]);
        assert!(positions[1].iter().all(|value| value.is_nan()));
        let mut lng_lats = [[0.0; 2]; 2];
        crs.converter.inverse(&[[10.0, 20.0], [-30.0, -40.0]], &mut lng_lats);
        assert_eq!(lng_lats, [[10.0, 20.0], [-30.0, -40.0]]);

        // Dropped before it was accepted, the converter leaves its user data
        // with the caller; accepted, it releases it once it drops.
        drop(crss);
        drop(converters);
        assert_eq!(released.try_recv(), Err(mpsc::TryRecvError::Empty));
        reclaim(&first);
        let second = projection("test:second", &probe);
        // SAFETY: as above.
        let (crss, converters) = unsafe { copy(&raw const second, 1) }.unwrap();
        converters.accept();
        drop(converters);
        assert_eq!(released.try_recv(), Err(mpsc::TryRecvError::Empty));
        drop(crss);
        assert_eq!(released.try_recv(), Ok(()));
        assert_eq!(Arc::strong_count(&probe), 1);
    }
}
