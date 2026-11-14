//! A handle to a map that lives on its runtime's thread.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, Weak};

use mln_actor::RunLoopHandle;
use mln_map::{
    AnimationOptions, BoundOptions, Camera, CameraDelta, CameraOptions, CameraUpdate, FreeCameraOptions, LogicalExtent,
    MapDebugOptions, MapEvent, MapMode, MapSnapshot, ProjectionMode, StyleLoadError, TransitionEnd, TransitionOutcome,
};
use mln_renderer::UpdateReader;
use mln_style::layers::CustomLayerHost;
use mln_text::GlyphCache;
use mln_util::error::{Error, ErrorKind};
use mln_util::geo::LatLngBounds;
use mln_util::size::Size;
use parking_lot::Mutex;

use crate::camera::{self, CameraFitArea, CameraFitOptions, CameraFitUpdate, TileOptions, ViewportOptions};
use crate::completion::{Completion, Resolver};
use crate::event::{EventMask, EventSource, RuntimeEvent, RuntimeEventKind};
use crate::feature_state::{FeatureState, FeatureStateSelector};
use crate::render::{
    Abandoner, AttachOptions, RenderSession, RenderTarget, SessionId, SessionShared, Worker, worker_post,
};
use crate::runtime::{Outbox, Runtime, RuntimeCore, RuntimeShared, post};
use crate::style::{CallbackSources, GeoJsonUpdates};

/// The ID of a map, unique within its runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MapId(pub u64);

/// How to create a map.
#[derive(Clone, Debug)]
pub struct MapOptions {
    pub map: mln_map::MapOptions,
    /// The events that the map reports.
    pub event_mask: EventMask,
    /// JSON text of the font faces that the map falls back to for glyphs
    /// that the style's fonts lack, as `Style::set_fallback_font_faces`
    /// takes it. Without it, the map falls back to the system's fonts
    /// unless `system_fonts` is off.
    pub fallback_font_faces: Option<String>,
    /// Whether the map falls back to the system's fonts
    /// (`mln_system_fonts::fallback_font_faces`) for CJK and the scripts that
    /// need complex shaping, when `fallback_font_faces` is `None`. On by
    /// default. The first map that uses them waits for them to be found. In
    /// the browser, which has no font files to read, the browser draws CJK
    /// glyphs in its sans-serif font instead, where `fallback_font_faces`
    /// doesn't cover them, as MapLibre GL JS does.
    pub system_fonts: bool,
    /// Whether the map's render sessions keep tiles in memory after they
    /// leave the view, so they show again without loading. On by default.
    pub tile_cache_enabled: bool,
}

impl Default for MapOptions {
    fn default() -> Self {
        MapOptions {
            map: mln_map::MapOptions::default(),
            event_mask: EventMask::default(),
            fallback_font_faces: None,
            system_fonts: true,
            tile_cache_enabled: true,
        }
    }
}

/// What a map command left behind once it ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Applied {
    /// The command's place among the map's commands: the map's creation is
    /// 1, and each command takes the next number when it is sent.
    pub sequence: u64,
    /// The map's generation after the command: a snapshot with this
    /// generation or a later one includes the command.
    pub generation: u64,
    /// The map's update generation after the command: a frame that renders
    /// this update or a later one shows the command.
    pub render_update_generation: u64,
}

/// The camera, and the generation of the map that it belongs to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraQuery {
    pub generation: u64,
    pub camera: Camera,
}

/// What the runtime's thread publishes for a map's handles.
pub(crate) struct Published {
    snapshot: Mutex<Option<MapSnapshot>>,
    /// The map's updates for its renderer, once the map is built.
    updates: OnceLock<UpdateReader>,
    /// The sequence number of the last command that ran.
    applied: AtomicU64,
    event_mask: AtomicU64,
    /// The still image that waits for a frame, as the
    /// `still_image_request_id` of the updates that ask for it, or 0.
    still_image: AtomicU64,
}

impl Published {
    /// The sequence number of the last command that ran.
    pub(crate) fn applied(&self) -> u64 {
        self.applied.load(Ordering::SeqCst)
    }

    /// The still image that waits for a frame, as the
    /// `still_image_request_id` of the updates that ask for it, or 0 when
    /// none waits.
    pub(crate) fn pending_still_image(&self) -> u64 {
        self.still_image.load(Ordering::SeqCst)
    }

    pub(crate) fn new(event_mask: EventMask) -> Self {
        Published {
            snapshot: Mutex::new(None),
            updates: OnceLock::new(),
            applied: AtomicU64::new(0),
            event_mask: AtomicU64::new(event_mask.bits()),
            still_image: AtomicU64::new(0),
        }
    }
}

pub(crate) struct MapShared {
    id: MapId,
    runtime: Arc<RuntimeShared>,
    mode: MapMode,
    /// The map's scale factor, an `f64`'s bits: the options' pixel ratio,
    /// until a session's resize sends another.
    scale_factor: AtomicU64,
    tile_cache_enabled: bool,
    system_fonts: bool,
    /// The render session attached to the map, from its attach until its
    /// detach completes.
    session: Mutex<Option<SessionClaim>>,
    /// The sequence number of the last command sent. It stays locked while
    /// a command is numbered and posted, so the runtime's thread runs the
    /// map's commands in the order of their numbers.
    sent: Mutex<u64>,
    /// Whether the handles refuse commands.
    released: AtomicBool,
    /// Whether the map's removal was posted to the runtime's thread.
    removed: AtomicBool,
    published: Arc<Published>,
}

impl MapShared {
    pub(crate) fn new(id: MapId, runtime: Arc<RuntimeShared>, options: &MapOptions, published: Arc<Published>) -> Self {
        MapShared {
            id,
            runtime,
            mode: options.map.map_mode,
            scale_factor: AtomicU64::new(f64::from(options.map.pixel_ratio).to_bits()),
            tile_cache_enabled: options.tile_cache_enabled,
            system_fonts: options.system_fonts,
            session: Mutex::new(None),
            sent: Mutex::new(1),
            released: AtomicBool::new(false),
            removed: AtomicBool::new(false),
            published,
        }
    }

    /// Numbers a command and posts it to the runtime's thread.
    pub(crate) fn send<R: Send + 'static>(
        &self,
        command: impl FnOnce(&mut RuntimeCore, &mut Outbox, MapId, u64, Resolver<R>) + Send + 'static,
    ) -> Result<Completion<R>, Error> {
        let mut sent = self.sent.lock();
        if self.released.load(Ordering::SeqCst) {
            return Err(released());
        }
        let (completion, resolver) = self.runtime.completion();
        let id = self.id;
        *sent += 1;
        let sequence = *sent;
        self.runtime.post(move |core, outbox| command(core, outbox, id, sequence, resolver));
        Ok(completion)
    }

    /// Releases the map: no command is sent after its removal. Fails when
    /// the map was released already.
    fn release(&self, done: Option<Resolver<()>>) -> Result<(), Error> {
        self.close()?;
        self.remove(done);
        Ok(())
    }

    /// Refuses the commands sent from now on. Fails when the map was
    /// released already.
    fn close(&self) -> Result<(), Error> {
        let _sent = self.sent.lock();
        if self.released.swap(true, Ordering::SeqCst) {
            return Err(released());
        }
        Ok(())
    }

    /// Removes the closed map from its runtime's thread, after the commands
    /// sent before, once.
    fn remove(&self, done: Option<Resolver<()>>) {
        if self.removed.swap(true, Ordering::SeqCst) {
            return;
        }
        let id = self.id;
        self.runtime.post(move |core, outbox| remove(core, id, done, outbox));
        self.runtime.map_released();
    }
}

impl Drop for MapShared {
    fn drop(&mut self) {
        let _ = self.close();
        let deferred = self.session.get_mut().take().and_then(|claim| claim.removal);
        self.remove(deferred);
    }
}

/// A handle to a map, which lives on its runtime's thread.
///
/// Each command takes the next sequence number when it is sent and runs on
/// the runtime's thread after every command sent before it, to this map,
/// another map or the runtime. It returns a [`Completion`] of its result, or
/// an error when it can't be sent, and then never completes. A command sent
/// before the map is built waits for it. `Clone` is another handle to the
/// same map, and any thread may use one.
///
/// The map lives until [`release`](Self::release) or until its last handle
/// drops. A command on a released map fails with
/// [`ErrorKind::InvalidState`](mln_util::ErrorKind).
#[derive(Clone)]
pub struct Map {
    pub(crate) shared: Arc<MapShared>,
}

impl std::fmt::Debug for Map {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Map").field("id", &self.shared.id).finish()
    }
}

impl PartialEq for Map {
    fn eq(&self, other: &Map) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }
}

impl Eq for Map {}

fn released() -> Error {
    Error::invalid_state("the map was released")
}

pub(crate) fn style_not_loaded() -> Error {
    Error::invalid_state("style is not loaded")
}

/// The error of a command whose map failed to be built, or was released
/// while the command was on its way.
pub(crate) fn missing_map() -> Error {
    Error::invalid_state("the map failed to be built or was released")
}

impl Map {
    pub(crate) fn new(shared: MapShared) -> Self {
        Map { shared: Arc::new(shared) }
    }

    #[must_use]
    pub fn id(&self) -> MapId {
        self.shared.id
    }

    /// When the map renders, as its options set it.
    #[must_use]
    pub fn mode(&self) -> MapMode {
        self.shared.mode
    }

    /// The runtime that the map lives on.
    #[must_use]
    pub fn runtime(&self) -> Runtime {
        Runtime { shared: Arc::clone(&self.shared.runtime) }
    }

    /// The sequence number of the last command sent to the map.
    #[must_use]
    pub fn sent_sequence(&self) -> u64 {
        *self.shared.sent.lock()
    }

    /// The sequence number of the last command that ran.
    #[must_use]
    pub fn applied_sequence(&self) -> u64 {
        self.shared.published.applied.load(Ordering::SeqCst)
    }

    /// The map's state after the last command that ran, without waiting for
    /// the commands still on their way. To read the state after them, send a
    /// command and read the snapshot once it completes, or use a query such
    /// as [`camera`](Self::camera). Its `update_generation` counts the
    /// renderer's updates as they are when it is read, which can be later
    /// than the rest of the state.
    ///
    /// Fails with [`ErrorKind::InvalidState`](mln_util::ErrorKind) until the
    /// map is built, and after it is released.
    pub fn snapshot(&self) -> Result<MapSnapshot, Error> {
        if self.shared.released.load(Ordering::SeqCst) {
            return Err(released());
        }
        let mut snapshot =
            self.shared.published.snapshot.lock().ok_or_else(|| Error::invalid_state("the map isn't built"))?;
        // The map publishes an update, which the renderer may draw at once,
        // before it copies the snapshot: count the updates as they are now, so
        // a frame never shows a later update than the snapshot.
        if let Some(updates) = self.shared.published.updates.get() {
            snapshot.update_generation = updates.generation();
        }
        Ok(snapshot)
    }

    /// The events that the map reports, as of the last command that ran.
    #[must_use]
    pub fn event_mask(&self) -> EventMask {
        EventMask::from_bits(self.shared.published.event_mask.load(Ordering::SeqCst)).unwrap_or_default()
    }

    /// Sends a command that runs `apply` on the map's entry, and completes
    /// with what `finish` makes of its result and the map after it.
    pub(crate) fn send<T: 'static, R: Send + 'static>(
        &self,
        apply: impl FnOnce(&mut MapEntry, &mut Outbox) -> Result<T, Error> + Send + 'static,
        finish: impl FnOnce(T, &MapEntry) -> R + Send + 'static,
    ) -> Result<Completion<R>, Error> {
        self.shared.send(move |core, outbox, id, sequence, resolver| {
            run_command(core, id, sequence, apply, finish, resolver, outbox);
        })
    }

    /// A command that changes the map.
    pub(crate) fn command(
        &self,
        apply: impl FnOnce(&mut mln_map::Map) -> Result<(), Error> + Send + 'static,
    ) -> Result<Completion<Applied>, Error> {
        self.send(move |entry, _| apply(&mut entry.core), |(), entry| entry.applied())
    }

    /// A command that reads the map.
    pub(crate) fn query<T: Send + 'static>(
        &self,
        read: impl FnOnce(&mut mln_map::Map) -> Result<T, Error> + Send + 'static,
    ) -> Result<Completion<T>, Error> {
        self.send(move |entry, _| read(&mut entry.core), |value, _| value)
    }

    /// Runs `f` on the map itself, on the runtime's thread, in its place
    /// among the map's commands. It sees the effects of the commands sent
    /// before it. The map handles what `f` changed, and reports its events,
    /// before the completion.
    pub fn with_core<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut mln_map::Map) -> R + Send + 'static,
    ) -> Result<Completion<R>, Error> {
        self.query(move |map| Ok(f(map)))
    }

    /// Releases the map. The completion reports when the runtime has let go
    /// of it. A render session that was released, but hasn't let go of the
    /// map yet, is abandoned, as
    /// [`RenderSession::abandon`](crate::RenderSession::abandon) does: a
    /// caller-driven one that the host stopped servicing would otherwise
    /// hold the map for good.
    ///
    /// Fails with [`ErrorKind::InvalidState`](mln_util::ErrorKind) when the
    /// map was released already, or while a render session that wasn't
    /// released is attached to it.
    pub fn release(&self) -> Result<Completion<()>, Error> {
        self.release_with(false)
    }

    /// Releases the map whatever its state, as dropping its last handle
    /// does, and lets the runtime finish in the background: the completion
    /// reports when the runtime has let go of it, and may be dropped. Every
    /// command fails with [`ErrorKind::InvalidState`](mln_util::ErrorKind)
    /// from now on. A render session that is attached and wasn't released
    /// keeps drawing the map's last update, and the runtime lets go of the
    /// map once the session detaches or is abandoned. A released one is
    /// abandoned, as [`release`](Self::release) does.
    ///
    /// Fails with [`ErrorKind::InvalidState`](mln_util::ErrorKind) when the
    /// map was released already.
    pub fn dispose(&self) -> Result<Completion<()>, Error> {
        self.release_with(true)
    }

    fn release_with(&self, in_any_state: bool) -> Result<Completion<()>, Error> {
        let (completion, resolver) = self.shared.runtime.completion();
        let claimed = {
            let mut claim = self.shared.session.lock();
            match claim.as_mut() {
                None => None,
                Some(held) => match held.session.upgrade() {
                    // Abandoned now, or ended already and about to let go.
                    Some(session) if session.is_released() => Some((held.id, Some(session))),
                    // The session lets go of the map when it ends, and the
                    // map is removed then.
                    Some(_) if in_any_state => {
                        self.shared.close()?;
                        held.removal = Some(resolver);
                        return Ok(completion);
                    }
                    Some(_) => return Err(Error::invalid_state("map still has an attached render session")),
                    // The session is gone without letting go of the map.
                    None => Some((held.id, None)),
                },
            }
        };
        if let Some((id, session)) = claimed {
            if let Some(session) = session {
                let _ = session.abandon(Abandoner::Library);
            }
            self.release_session_claim(id);
        }
        self.shared.release(Some(resolver))?;
        Ok(completion)
    }

    // MARK: - Map

    /// Sets the map's size and scale factor. The map keeps its tiles, and a
    /// resize to the extent the map has changes nothing, so it renders
    /// nothing. Fails with [`ErrorKind::InvalidState`](mln_util::ErrorKind)
    /// while a render session is attached, since the session sizes the map
    /// to its target: resize the session instead. Fails with
    /// [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for an empty size,
    /// or a scale factor that isn't finite and positive.
    pub fn resize(&self, extent: LogicalExtent) -> Result<Completion<Applied>, Error> {
        let LogicalExtent { size, scale_factor } = extent;
        if self.shared.session.lock().is_some() {
            return Err(Error::invalid_state("the map has a render session: resize the session instead"));
        }
        if size.is_empty() {
            return Err(Error::invalid_argument("logical size dimensions must be positive"));
        }
        if !scale_factor.is_finite() || scale_factor <= 0.0 {
            return Err(Error::invalid_argument(format!("scale factor {scale_factor} isn't finite and positive")));
        }
        // The scale factor that a session attached later renders at.
        self.shared.scale_factor.store(scale_factor.to_bits(), Ordering::SeqCst);
        self.command(move |map| {
            if map.get_map_options().size != size {
                map.set_size(size)?;
            }
            map.set_pixel_ratio(scale_factor as f32)
        })
    }

    /// Renders a new frame of a continuous map, even when nothing changed.
    /// Fails with [`ErrorKind::InvalidState`](mln_util::ErrorKind) for a
    /// static or tile map, which renders only still images.
    pub fn request_repaint(&self) -> Result<Completion<Applied>, Error> {
        if self.shared.mode != MapMode::Continuous {
            return Err(Error::invalid_state("map is not in continuous mode"));
        }
        self.command(|map| {
            map.trigger_repaint();
            Ok(())
        })
    }

    /// Holds the map's changes back from rendering until the matching
    /// [`end_command_group`](Self::end_command_group), so that they show in
    /// the same frame. Groups nest, and the changes show when the outermost
    /// group ends.
    pub fn begin_command_group(&self) -> Result<Completion<Applied>, Error> {
        self.command(|map| {
            map.begin_command_group();
            Ok(())
        })
    }

    /// Ends the group that [`begin_command_group`](Self::begin_command_group)
    /// began. Completes with [`ErrorKind::InvalidState`](mln_util::ErrorKind)
    /// when no group is open.
    pub fn end_command_group(&self) -> Result<Completion<Applied>, Error> {
        self.command(mln_map::Map::end_command_group)
    }

    /// Begins a command group that ends when the guard drops, or with
    /// [`CommandGroup::end`].
    pub fn command_group(&self) -> Result<CommandGroup, Error> {
        drop(self.begin_command_group()?);
        Ok(CommandGroup { map: Some(self.clone()) })
    }

    /// Selects the events that the map reports.
    pub fn set_event_mask(&self, mask: EventMask) -> Result<Completion<Applied>, Error> {
        self.send(
            move |entry, _| {
                if entry.mask != mask {
                    entry.mask = mask;
                    entry.published.event_mask.store(mask.bits(), Ordering::SeqCst);
                    entry.core.record_change();
                }
                Ok(())
            },
            |(), entry| entry.applied(),
        )
    }

    // MARK: - Rendering

    /// Attaches a render session, which renders the map into `target`, and
    /// sizes the map to the target. A map has one session at a time. The
    /// session takes frame demands and other work at once, and runs them
    /// once the completion reports it ready.
    ///
    /// The session renders on a thread of its own, or, with
    /// [`Driver::CallerThread`](crate::Driver::CallerThread), on the thread
    /// that takes its [`DriverWork`](crate::DriverWork): the attach completes
    /// in that thread's first service.
    ///
    /// Fails with [`ErrorKind::InvalidState`](mln_util::ErrorKind) when the
    /// map already has a session or was released, or a window is minimized
    /// or has no client area, and with
    /// [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for an empty
    /// target, a [`Surface::wgpu`](crate::Surface::wgpu) without the
    /// host's device, or a Win32 window with a pixel format of its own that
    /// OpenGL would present to. The completion fails as
    /// [`Device::new`](crate::Device::new) does when the
    /// session has to create its device and can't, with
    /// [`ErrorKind::InvalidState`](mln_util::ErrorKind) when the window or
    /// surface had no area to present to by then, with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) when no GPU can present
    /// to a window, and with [`ErrorKind::Native`](mln_util::ErrorKind) when
    /// the window has no surface to give.
    pub fn attach(
        &self,
        target: RenderTarget,
        options: AttachOptions,
    ) -> Result<(RenderSession, Completion<()>), Error> {
        crate::render::attach(self, target, options, |session, attached| (session.clone(), attached))
    }

    /// Attaches a render session as [`attach`](Self::attach) does, and calls
    /// `then` with it and the attach's completion before the session's
    /// thread may deliver the attach's result. A callback that `then`
    /// attaches with [`Completion::on_complete`] runs where the session
    /// delivers it: on the session's thread, inside its driver call, even
    /// when the session attaches before `then` runs. One attached after
    /// `attach` returned would run at once on this thread instead.
    ///
    /// `then` keeps the session with a clone of it: the session is released
    /// once its last handle drops. Fails as `attach` does, before `then`
    /// runs.
    pub fn attach_then<R>(
        &self,
        target: RenderTarget,
        options: AttachOptions,
        then: impl FnOnce(&RenderSession, Completion<()>) -> R,
    ) -> Result<R, Error> {
        crate::render::attach(self, target, options, then)
    }

    /// Renders a still image of a static or tile map, once every resource
    /// that it needs has loaded. The map's render session draws it, and
    /// [`RenderSession::read_texture`] reads its pixels once the completion
    /// reports success.
    ///
    /// While the image waits, the session has a demand of its own for it,
    /// whose [`FrameResult`](crate::FrameResult) has no token. The demand
    /// waits for the map to load what the image needs, and parks while a
    /// borrowed ring has no texture to render into. It finishes with the
    /// image's frame, which completes the image once its result is queued:
    /// a frame of the host's demands completes none. The host's demands
    /// neither replace it nor are replaced by it. When the session ends its
    /// demands, by a barrier, a target's release or a detach, the demand
    /// ends too, and the image waits for the next one: once the barrier ran,
    /// the target came back, or the next session attached.
    ///
    /// Fails with [`ErrorKind::InvalidState`](mln_util::ErrorKind) for a
    /// continuous map. Completes with
    /// [`ErrorKind::InvalidState`](mln_util::ErrorKind) while another image
    /// is on its way or a command group is open,
    /// [`ErrorKind::Native`](mln_util::ErrorKind) when the
    /// style or a resource that the image needs fails to load, and
    /// [`ErrorKind::Cancelled`](mln_util::ErrorKind) when the map is released
    /// first.
    pub fn request_still_image(&self) -> Result<Completion<()>, Error> {
        if self.shared.mode == MapMode::Continuous {
            return Err(Error::invalid_state("the map is not in static or tile mode"));
        }
        self.shared.send(move |core, outbox, id, sequence, resolver| {
            if !core.maps.contains_key(&id) {
                return outbox.resolve(resolver, Err(missing_map()));
            }
            apply_command(
                core,
                id,
                sequence,
                outbox,
                move |entry, outbox| {
                    if entry.pending_still.is_some() {
                        let error = Error::invalid_state("the map is rendering a still image already");
                        return outbox.resolve(resolver, Err(error));
                    }
                    // The group holds back the update that the image needs.
                    if entry.core.command_group_depth() > 0 {
                        let error = Error::invalid_state("a still image can't render while a command group is open");
                        return outbox.resolve(resolver, Err(error));
                    }
                    let outcome = Rc::new(RefCell::new(None));
                    let done = Rc::clone(&outcome);
                    entry.core.render_still(Box::new(move |result| {
                        *done.borrow_mut() = Some(result.map_err(still_image_error));
                    }));
                    entry.pending_still = Some(PendingStill { resolver, outcome });
                },
                |_, (), _, _| {},
            );
        })
    }

    /// Whether the map was released or disposed.
    pub(crate) fn is_released(&self) -> bool {
        self.shared.released.load(Ordering::SeqCst)
    }

    pub(crate) fn runtime_thread(&self) -> std::thread::ThreadId {
        self.shared.runtime.thread()
    }

    /// The map's scale factor: device pixels per logical pixel. It is the
    /// pixel ratio of the map's options until a render session's
    /// [`resize`](crate::RenderSession::resize) sets another, and the map's
    /// next session renders at it.
    #[must_use]
    pub fn scale_factor(&self) -> f64 {
        f64::from_bits(self.shared.scale_factor.load(Ordering::SeqCst))
    }

    pub(crate) fn tile_cache_enabled(&self) -> bool {
        self.shared.tile_cache_enabled
    }

    /// The font family that the map's renderers draw CJK glyphs in rather
    /// than downloading them: in the browser with system fonts, its
    /// sans-serif font, as MapLibre GL JS's `localIdeographFontFamily`
    /// defaults to.
    pub(crate) fn local_font_family(&self) -> Option<String> {
        (cfg!(target_family = "wasm") && self.shared.system_fonts).then(|| "sans-serif".to_owned())
    }

    pub(crate) fn glyph_cache(&self) -> Arc<GlyphCache> {
        Arc::clone(&self.shared.runtime.glyph_cache)
    }

    /// Reserves the map for the session `id`.
    pub(crate) fn claim_session(&self, id: SessionId, session: Weak<SessionShared>) -> Result<(), Error> {
        let mut claim = self.shared.session.lock();
        if self.shared.released.load(Ordering::SeqCst) {
            return Err(released());
        }
        if claim.is_some() {
            return Err(Error::invalid_state("the map already has an attached render session"));
        }
        *claim = Some(SessionClaim { id, session, removal: None });
        Ok(())
    }

    /// Lets another session attach once the session `id` is gone, and
    /// removes the map when a dispose waited for the session.
    pub(crate) fn release_session_claim(&self, id: SessionId) {
        let claim = {
            let mut session = self.shared.session.lock();
            if session.as_ref().is_some_and(|claim| claim.id == id) { session.take() } else { None }
        };
        if let Some(removal) = claim.and_then(|claim| claim.removal) {
            self.shared.remove(Some(removal));
        }
    }

    /// Sends the command that binds the session `id`, whose thread runs
    /// `run_loop`, to the map: it sizes the map to `size`, and hands the
    /// session the map's updates. The map tells the session of the commands
    /// it runs from `awaited` on.
    pub(crate) fn bind_session(
        &self,
        id: SessionId,
        run_loop: RunLoopHandle,
        awaited: Arc<AtomicU64>,
        size: Size,
    ) -> Result<(), Error> {
        let sent = self.shared.send(move |core, outbox, map_id, sequence, resolver: Resolver<()>| {
            drop(resolver);
            if !core.maps.contains_key(&map_id) {
                return worker_post(&run_loop, |worker, outbox| worker.bind_failed(missing_map(), outbox));
            }
            let link = SessionLink { id, run_loop: run_loop.clone(), awaited };
            apply_command(
                core,
                map_id,
                sequence,
                outbox,
                move |entry, _| {
                    entry.session = Some(link);
                    let resized = entry.core.set_size(size);
                    (resized, entry.core.update_reader(), entry.core.renderer_events(), Arc::clone(&entry.published))
                },
                move |_, (resized, reader, events, published), _, _| match resized {
                    Ok(()) => {
                        let bound = move |worker: &mut Worker, outbox: &mut Outbox| {
                            worker.bound(sequence, reader, events, published, outbox);
                        };
                        worker_post(&run_loop, bound);
                    }
                    Err(error) => worker_post(&run_loop, move |worker, outbox| worker.bind_failed(error, outbox)),
                },
            );
        });
        sent.map(drop)
    }

    /// Unbinds the session `id` from the map on the runtime's thread, after
    /// the commands sent before.
    pub(crate) fn unbind_session(&self, id: SessionId) {
        let map_id = self.shared.id;
        // A runtime that shut down has no map to unbind from.
        self.shared.runtime.post_or((), move |core, _, ()| unbind(core, map_id, id), |()| {});
    }

    /// Unbinds the session `id` as [`unbind_session`](Self::unbind_session)
    /// does, then runs `then` on the session's thread.
    pub(crate) fn unbind_session_then(
        &self,
        id: SessionId,
        run_loop: RunLoopHandle,
        then: impl FnOnce(&mut Worker, &mut Outbox) + Send + 'static,
    ) {
        let map_id = self.shared.id;
        let session_loop = run_loop.clone();
        // A runtime that shut down has no map to unbind from.
        self.shared.runtime.post_or(
            then,
            move |core, _, then| {
                unbind(core, map_id, id);
                worker_post(&run_loop, then);
            },
            |then| worker_post(&session_loop, then),
        );
    }

    /// Sends a command that resizes the map to `size` at `scale_factor`,
    /// then runs `resize` with its outcome on the thread of the session that
    /// `run_loop` runs, before the session learns of any later command.
    pub(crate) fn resize_for_session(
        &self,
        size: Size,
        scale_factor: f64,
        run_loop: RunLoopHandle,
        resize: impl FnOnce(&mut Worker, Result<Applied, Error>, &mut Outbox) + Send + 'static,
    ) -> Result<(), Error> {
        // The scale factor that a session attached later renders at.
        self.shared.scale_factor.store(scale_factor.to_bits(), Ordering::SeqCst);
        let sent = self.shared.send(move |core, outbox, id, sequence, resolver: Resolver<()>| {
            drop(resolver);
            if !core.maps.contains_key(&id) {
                return worker_post(&run_loop, move |worker, outbox| resize(worker, Err(missing_map()), outbox));
            }
            apply_command(
                core,
                id,
                sequence,
                outbox,
                move |entry, _| {
                    // A host may hand the session the target it has, such as
                    // on every frame. That changes nothing, so it publishes
                    // no update, which a demand sent before would draw.
                    let pixel_ratio = scale_factor as f32;
                    if entry.core.get_map_options().size == size && entry.core.get_pixel_ratio() == pixel_ratio {
                        return Ok(());
                    }
                    entry.core.set_size(size)?;
                    entry.core.set_pixel_ratio(pixel_ratio)
                },
                move |_, resized, applied, _| {
                    let result = resized.map(|()| applied);
                    worker_post(&run_loop, move |worker, outbox| resize(worker, result, outbox));
                },
            );
        });
        sent.map(drop)
    }

    // MARK: - Feature state

    /// Merges `state` into the state of the feature that `selector` names:
    /// its keys replace the same keys of the feature's state, and the other
    /// keys stay.
    ///
    /// The map keeps the state until it is removed, its source leaves the
    /// style, or a style loads: [`remove_style_source`](Self::remove_style_source)
    /// removes the state of the source's features, and a style that loads
    /// removes the state of every source. To set state for a new style's
    /// sources, wait until [`set_style_url`](Self::set_style_url) or
    /// [`set_style_json`](Self::set_style_json) completes.
    ///
    /// The map's next update for its render session carries the state, so a
    /// frame shows it, and a rendered query reads it, once the frame renders
    /// that update. A command that changes no state publishes no update: an
    /// empty `state` merges nothing.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a
    /// selector without a feature.
    pub fn set_feature_state(
        &self,
        selector: &FeatureStateSelector,
        state: FeatureState,
    ) -> Result<Completion<Applied>, Error> {
        let key = selector.key(true)?;
        self.command(move |map| {
            let feature_id = key.feature_id.as_deref().unwrap_or_default();
            map.set_feature_state(&key.source_id, Some(&key.source_layer_id), feature_id, state);
            Ok(())
        })
    }

    /// The state of the feature that `selector` names, empty when it has
    /// none. Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind)
    /// for a selector without a feature.
    pub fn feature_state(&self, selector: &FeatureStateSelector) -> Result<Completion<FeatureState>, Error> {
        let key = selector.key(true)?;
        self.query(move |map| {
            let feature_id = key.feature_id.as_deref().unwrap_or_default();
            Ok(map.get_feature_state(&key.source_id, Some(&key.source_layer_id), feature_id))
        })
    }

    /// Removes the state that `selector` selects: one key of a feature, all
    /// of a feature's state, or that of every feature in a source layer.
    /// The removal reaches the render session as
    /// [`set_feature_state`](Self::set_feature_state) describes, and a
    /// removal of no state publishes no update. Fails with
    /// [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a state key
    /// without a feature.
    pub fn remove_feature_state(&self, selector: &FeatureStateSelector) -> Result<Completion<Applied>, Error> {
        let key = selector.key(false)?;
        self.command(move |map| {
            map.remove_feature_state(
                &key.source_id,
                Some(&key.source_layer_id),
                key.feature_id.as_deref(),
                key.state_key.as_deref(),
            );
            Ok(())
        })
    }

    // MARK: - Style

    /// Loads the map's style from `url`.
    ///
    /// Completes once the style document has loaded and parsed, with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) when it fails to, and with
    /// [`ErrorKind::Cancelled`](mln_util::ErrorKind) when another style load
    /// starts or the map is released first. The style's sources, sprites and
    /// glyphs then load in the background. Commands that change the style
    /// fail with [`ErrorKind::InvalidState`](mln_util::ErrorKind) until the
    /// document has parsed.
    pub fn set_style_url(&self, url: impl Into<String>) -> Result<Completion<Applied>, Error> {
        let url = url.into();
        if url.is_empty() {
            return Err(Error::invalid_argument("style URL must not be empty"));
        }
        self.load_style(StyleLoad::Url(url))
    }

    /// Loads the map's style from JSON text. Completes as
    /// [`set_style_url`](Self::set_style_url) does, once the style has
    /// parsed, but with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind)
    /// for JSON that isn't a valid style. A style that fails to parse leaves
    /// the map's style as it was.
    pub fn set_style_json(&self, json: impl Into<String>) -> Result<Completion<Applied>, Error> {
        let json = json.into();
        if json.is_empty() {
            return Err(Error::invalid_argument("style JSON must not be empty"));
        }
        self.load_style(StyleLoad::Json(json))
    }

    fn load_style(&self, load: StyleLoad) -> Result<Completion<Applied>, Error> {
        self.shared.send(move |core, outbox, id, sequence, resolver| {
            load_style(core, id, sequence, load, resolver, outbox);
        })
    }

    /// The JSON of the style that the map loaded, without the changes made
    /// since.
    pub fn loaded_style_json(&self) -> Result<Completion<String>, Error> {
        self.query(|map| Ok(map.style().get_json()))
    }

    /// The URL of the map's style, or `None` for a style loaded from JSON.
    pub fn style_url(&self) -> Result<Completion<Option<String>>, Error> {
        self.query(|map| Ok(Some(map.style().get_url()).filter(|url| !url.is_empty())))
    }

    /// Adds a source to the style from its JSON. See
    /// `Style::add_source_json` for the errors.
    pub fn add_style_source_json(
        &self,
        source_id: impl Into<String>,
        json: impl Into<String>,
    ) -> Result<Completion<Applied>, Error> {
        let (source_id, json) = (source_id.into(), json.into());
        self.command(move |map| map.style_mut().add_source_json(&source_id, &json))
    }

    /// Removes a source from the style. Completes with
    /// [`ErrorKind::NotFound`](mln_util::ErrorKind) when the style has no
    /// such source, and [`ErrorKind::InvalidState`](mln_util::ErrorKind)
    /// when a layer uses it or the style isn't loaded. The source's features
    /// lose their state, and a custom source's callbacks drop before the
    /// completion.
    pub fn remove_style_source(&self, source_id: impl Into<String>) -> Result<Completion<Applied>, Error> {
        let source_id = source_id.into();
        self.send(
            move |entry, _| {
                let style = entry.core.style_mut();
                if !style.is_loaded() {
                    return Err(style_not_loaded());
                }
                if style.get_source(&source_id).is_none() {
                    return Err(Error::not_found(format!("source \"{source_id}\" does not exist")));
                }
                if let Some(layer) = style.get_layers().into_iter().find(|layer| layer.get_source_id() == source_id) {
                    let message = format!("source \"{source_id}\" is used by layer \"{}\"", layer.get_id());
                    return Err(Error::invalid_state(message));
                }
                style.remove_source(&source_id);
                entry.core.remove_source_feature_state(&source_id);
                Ok(())
            },
            |(), entry| entry.applied(),
        )
    }

    /// Adds a layer to the style from its JSON, below `before_layer_id` or
    /// above every layer. See `Style::add_layer_json` for the errors.
    pub fn add_style_layer_json(
        &self,
        json: impl Into<String>,
        before_layer_id: Option<String>,
    ) -> Result<Completion<Applied>, Error> {
        let json = json.into();
        self.command(move |map| map.style_mut().add_layer_json(&json, before_layer_id.as_deref()))
    }

    /// Adds a custom layer named `layer_id` that `host` draws, below
    /// `before_layer_id` or above every layer. See `Style::add_custom_layer`
    /// for the errors. The map lets go of `host` once the style no longer has
    /// the layer, and no renderer draws it.
    pub fn add_custom_layer(
        &self,
        layer_id: impl Into<String>,
        host: Arc<dyn CustomLayerHost>,
        before_layer_id: Option<String>,
    ) -> Result<Completion<Applied>, Error> {
        let layer_id = layer_id.into();
        self.command(move |map| map.style_mut().add_custom_layer(&layer_id, host, before_layer_id.as_deref()))
    }

    /// Removes a layer from the style. Completes with
    /// [`ErrorKind::NotFound`](mln_util::ErrorKind) when the style has no
    /// such layer, and [`ErrorKind::InvalidState`](mln_util::ErrorKind)
    /// while the style isn't loaded.
    pub fn remove_style_layer(&self, layer_id: impl Into<String>) -> Result<Completion<Applied>, Error> {
        let layer_id = layer_id.into();
        self.command(move |map| {
            let style = map.style_mut();
            if !style.is_loaded() {
                return Err(style_not_loaded());
            }
            style
                .remove_layer(&layer_id)
                .map(drop)
                .ok_or_else(|| Error::not_found(format!("layer \"{layer_id}\" does not exist")))
        })
    }

    /// A layer of the style as JSON, or `None` when the style has no such
    /// layer.
    pub fn style_layer_json(&self, layer_id: impl Into<String>) -> Result<Completion<Option<String>>, Error> {
        let layer_id = layer_id.into();
        self.query(move |map| Ok(map.style().layer_json(&layer_id)))
    }

    /// Sets a property of a layer to a JSON value. See
    /// `Style::set_layer_property_json` for the errors.
    pub fn set_style_layer_property(
        &self,
        layer_id: impl Into<String>,
        name: impl Into<String>,
        json: impl Into<String>,
    ) -> Result<Completion<Applied>, Error> {
        let (layer_id, name, json) = (layer_id.into(), name.into(), json.into());
        self.command(move |map| map.style_mut().set_layer_property_json(&layer_id, &name, &json))
    }

    /// A property of a layer as JSON, or `None` when the layer doesn't set
    /// it. See `Style::layer_property_json` for the errors.
    pub fn style_layer_property(
        &self,
        layer_id: impl Into<String>,
        name: impl Into<String>,
    ) -> Result<Completion<Option<String>>, Error> {
        let (layer_id, name) = (layer_id.into(), name.into());
        self.query(move |map| map.style().layer_property_json(&layer_id, &name))
    }

    /// Sets a root property of the style to a JSON value, without
    /// reloading the style. See `Style::set_root_property_json` for the
    /// properties that change and the errors.
    pub fn set_style_root_property(
        &self,
        name: impl Into<String>,
        json: impl Into<String>,
    ) -> Result<Completion<Applied>, Error> {
        let (name, json) = (name.into(), json.into());
        self.command(move |map| map.style_mut().set_root_property_json(&name, &json))
    }

    /// A root property of the style as JSON, or `None` when neither the
    /// style nor the map sets it. See `Style::root_property_json`.
    pub fn style_root_property(&self, name: impl Into<String>) -> Result<Completion<Option<String>>, Error> {
        let name = name.into();
        self.query(move |map| map.style().root_property_json(&name))
    }

    /// Sets a property of the style's light to a JSON value.
    pub fn set_style_light_property(
        &self,
        name: impl Into<String>,
        json: impl Into<String>,
    ) -> Result<Completion<Applied>, Error> {
        let (name, json) = (name.into(), json.into());
        self.command(move |map| map.style_mut().set_light_property_json(&name, &json))
    }

    /// A property of the style's light as JSON, or `None` when the light
    /// doesn't set it.
    pub fn style_light_property(&self, name: impl Into<String>) -> Result<Completion<Option<String>>, Error> {
        let name = name.into();
        self.query(move |map| Ok(map.style().light_property_json(&name)))
    }

    // MARK: - Camera

    /// A command that moves the camera: its animation's end function runs on
    /// the runtime's thread once the map has published the snapshot that
    /// shows where the change left the camera, whose generation the end
    /// reports, and before the command's completion.
    fn camera_command(
        &self,
        apply: impl FnOnce(&mut mln_map::Map, &Ends) -> Result<(), Error> + Send + 'static,
    ) -> Result<Completion<Applied>, Error> {
        self.send(move |entry, _| apply(&mut entry.core, &entry.ends), |(), entry| entry.applied())
    }

    /// Moves the camera to absolute values, by a jump, an ease or a flight.
    pub fn update_camera(&self, mut update: CameraUpdate) -> Result<Completion<Applied>, Error> {
        self.camera_command(move |map, ends| {
            ends.defer(&mut update.animation);
            map.update_camera(&update);
            Ok(())
        })
    }

    /// Moves the camera relative to where it is, as a gesture does.
    /// Completes with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for
    /// a delta that the camera can't apply.
    pub fn apply_camera_delta(&self, mut delta: CameraDelta) -> Result<Completion<Applied>, Error> {
        self.camera_command(move |map, ends| {
            ends.defer(&mut delta.animation);
            map.apply_camera_delta(&delta).map_err(|error| Error::invalid_argument(error.to_string()))
        })
    }

    /// The camera that fits `area` in the map's view, after the commands
    /// sent before. It keeps the map's bearing and pitch unless `options`
    /// sets them, and its zoom stays within the map's zoom limits.
    ///
    /// Completes with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for
    /// an empty area, and for padding that leaves no room for it.
    pub fn camera_for(
        &self,
        area: CameraFitArea,
        options: CameraFitOptions,
    ) -> Result<Completion<CameraOptions>, Error> {
        self.query(move |map| camera::camera_for(map, &area, &options))
    }

    /// Moves the camera to the one that [`camera_for`](Self::camera_for)
    /// finds once the commands sent before have run, as
    /// [`update_camera`](Self::update_camera) moves it.
    ///
    /// Completes as `camera_for` fails, and then leaves the camera where it
    /// is and ends the animation as cancelled.
    pub fn fit_camera(&self, area: CameraFitArea, update: CameraFitUpdate) -> Result<Completion<Applied>, Error> {
        self.camera_command(move |map, ends| {
            let CameraFitUpdate { mode, fit, mut animation } = update;
            ends.defer(&mut animation);
            match camera::camera_for(map, &area, &fit) {
                Ok(camera) => {
                    map.update_camera(&CameraUpdate { mode, camera, animation, ..CameraUpdate::default() });
                    Ok(())
                }
                Err(error) => {
                    if let Some(end_fn) = &animation.transition_end_fn {
                        end_fn(&TransitionEnd { outcome: TransitionOutcome::Cancelled, ..TransitionEnd::default() });
                    }
                    Err(error)
                }
            }
        })
    }

    /// Stops every camera animation.
    pub fn cancel_camera_transitions(&self) -> Result<Completion<Applied>, Error> {
        self.command(|map| {
            map.cancel_transitions();
            Ok(())
        })
    }

    /// Stops the camera animations whose transition ID is `transition_id`.
    pub fn cancel_camera_transitions_with_id(&self, transition_id: u64) -> Result<Completion<Applied>, Error> {
        self.command(move |map| {
            map.cancel_transitions_with_id(transition_id);
            Ok(())
        })
    }

    /// The camera after the commands sent before.
    pub fn camera(&self) -> Result<Completion<CameraQuery>, Error> {
        self.send(
            |_, _| Ok(()),
            |(), entry| CameraQuery { generation: entry.core.generation(), camera: entry.core.snapshot().view.camera },
        )
    }

    /// The area that `camera` would show, with the fields that `camera`
    /// leaves out from the map's camera. It holds every corner of a rotated
    /// view, and its longitudes can pass ±180, so a view across the
    /// antimeridian gives one continuous range.
    pub fn lat_lng_bounds_for_camera(&self, camera: CameraOptions) -> Result<Completion<LatLngBounds>, Error> {
        self.query(move |map| map.lat_lng_bounds_for_camera_unwrapped(&camera))
    }

    /// Sets the limits on the camera.
    pub fn set_bounds(&self, bounds: BoundOptions) -> Result<Completion<Applied>, Error> {
        self.command(move |map| map.set_bounds(&bounds))
    }

    /// Sets the camera's position and orientation in three dimensions.
    pub fn set_free_camera_options(&self, camera: FreeCameraOptions) -> Result<Completion<Applied>, Error> {
        self.command(move |map| {
            map.set_free_camera_options(&camera);
            Ok(())
        })
    }

    /// Sets how the map orients and constrains its view.
    pub fn set_viewport_options(&self, options: ViewportOptions) -> Result<Completion<Applied>, Error> {
        self.command(move |map| {
            camera::set_viewport(map, &options);
            Ok(())
        })
    }

    /// Sets how the map picks the tiles to load.
    pub fn set_tile_options(&self, options: TileOptions) -> Result<Completion<Applied>, Error> {
        self.command(move |map| {
            camera::set_tile_options(map, &options);
            Ok(())
        })
    }

    pub fn set_projection_mode(&self, mode: ProjectionMode) -> Result<Completion<Applied>, Error> {
        self.command(move |map| {
            map.set_projection_mode(&mode);
            Ok(())
        })
    }

    /// Selects the debug overlays that the map draws.
    pub fn set_debug_flags(&self, flags: MapDebugOptions) -> Result<Completion<Applied>, Error> {
        self.command(move |map| {
            map.set_debug(flags);
            Ok(())
        })
    }

    /// Shows or hides the rendering statistics over the map.
    pub fn set_rendering_stats_view_enabled(&self, enabled: bool) -> Result<Completion<Applied>, Error> {
        self.command(move |map| {
            map.enable_rendering_stats_view(enabled);
            Ok(())
        })
    }

    /// Sets whether drags and zooms on a globe turn the bearing, as
    /// `mln_map::Map::set_globe_free_bearing` describes.
    pub fn set_globe_free_bearing_enabled(&self, enabled: bool) -> Result<Completion<Applied>, Error> {
        self.command(move |map| {
            map.set_globe_free_bearing(enabled);
            Ok(())
        })
    }

    /// Logs the style's sources and their state.
    pub fn dump_debug_logs(&self) -> Result<Completion<Applied>, Error> {
        self.command(|map| {
            map.dump_debug_logs();
            Ok(())
        })
    }
}

/// A command group of a map that ends when it drops.
#[must_use = "the group ends when it drops"]
pub struct CommandGroup {
    map: Option<Map>,
}

impl CommandGroup {
    /// Ends the group, as [`Map::end_command_group`] does.
    pub fn end(mut self) -> Result<Completion<Applied>, Error> {
        match self.map.take() {
            Some(map) => map.end_command_group(),
            None => Err(Error::invalid_state("the group ended already")),
        }
    }
}

impl Drop for CommandGroup {
    fn drop(&mut self) {
        if let Some(map) = self.map.take() {
            let _ = map.end_command_group();
        }
    }
}

enum StyleLoad {
    Url(String),
    Json(String),
}

/// A style load that hasn't completed.
struct PendingStyle {
    resolver: Resolver<Applied>,
    /// Whether the caller passed the style document, which then is the
    /// argument that fails to parse.
    from_json: bool,
}

/// The render session that a map is reserved for.
struct SessionClaim {
    id: SessionId,
    session: Weak<SessionShared>,
    /// The completion of a dispose that waits for the session to end.
    removal: Option<Resolver<()>>,
}

/// Forgets the session `id` on the runtime's thread, unless the map has
/// bound another since.
fn unbind(core: &mut RuntimeCore, map_id: MapId, id: SessionId) {
    if let Some(entry) = core.maps.get_mut(&map_id)
        && entry.session.as_ref().is_some_and(|link| link.id == id)
    {
        entry.session = None;
    }
}

/// The render session bound to a map, as the runtime's thread knows it.
pub(crate) struct SessionLink {
    id: SessionId,
    run_loop: RunLoopHandle,
    /// The session's `awaited_command`.
    awaited: Arc<AtomicU64>,
}

/// A still image on its way: the map's callback leaves its outcome for the
/// next flush.
struct PendingStill {
    resolver: Resolver<()>,
    outcome: Rc<RefCell<Option<Result<(), Error>>>>,
}

/// A still image's error as the handle reports it: a load failure is the
/// library's, whatever the core called it.
fn still_image_error(error: Error) -> Error {
    match error.kind {
        ErrorKind::InvalidState => error,
        _ => Error::native(error.message),
    }
}

/// A map on the runtime's thread.
pub(crate) struct MapEntry {
    pub(crate) core: mln_map::Map,
    published: Arc<Published>,
    mask: EventMask,
    /// The sequence number of the last command that ran.
    applied: u64,
    /// The style load in progress.
    pending_style: Option<PendingStyle>,
    pending_still: Option<PendingStill>,
    /// The render session that draws the map.
    session: Option<SessionLink>,
    /// Whether a flush is due: set when the map's waker runs, and while a
    /// command runs, whose own flush follows; a flush clears it before it
    /// takes the map's events.
    flush_scheduled: Arc<AtomicBool>,
    ends: Ends,
    /// The callbacks of the custom sources that commands added.
    pub(crate) callback_sources: CallbackSources,
    /// The GeoJSON data that commands set and that is still being built.
    pub(crate) geojson_updates: GeoJsonUpdates,
}

impl MapEntry {
    pub(crate) fn applied(&self) -> Applied {
        Applied {
            sequence: self.applied,
            generation: self.core.generation(),
            render_update_generation: self.core.pending_update_generation(),
        }
    }
}

/// The camera changes of a map's commands that ended, which wait to report
/// their end until the map publishes the snapshot that shows it.
#[derive(Clone, Default)]
pub(crate) struct Ends(Arc<Mutex<Vec<(EndFn, TransitionEnd)>>>);

type EndFn = Arc<dyn Fn(&TransitionEnd) + Send + Sync>;

impl Ends {
    /// Makes `animation`'s end function wait for the snapshot.
    fn defer(&self, animation: &mut AnimationOptions) {
        if let Some(end_fn) = animation.transition_end_fn.take() {
            let ended = Arc::clone(&self.0);
            animation.transition_end_fn = Some(Arc::new(move |end: &TransitionEnd| {
                ended.lock().push((Arc::clone(&end_fn), *end));
            }));
        }
    }

    /// Reports the changes that ended to their commands, after what `outbox`
    /// holds, with the generation of the snapshot that shows them.
    fn deliver(&self, generation: u64, outbox: &mut Outbox) {
        for (end_fn, end) in std::mem::take(&mut *self.0.lock()) {
            outbox.push(move || end_fn(&TransitionEnd { generation, ..end }));
        }
    }
}

/// Builds the map `id` on the runtime's thread.
pub(crate) fn create(
    core: &mut RuntimeCore,
    id: MapId,
    options: &MapOptions,
    published: Arc<Published>,
    resolver: Resolver<Applied>,
    outbox: &mut Outbox,
) {
    let (file_sources, resource_options) = core.file_sources();
    let mut map = match mln_map::Map::with_resource_options(options.map, file_sources, resource_options) {
        Ok(map) => map,
        Err(error) => return outbox.resolve(resolver, Err(error)),
    };
    if let Some(json) = &options.fallback_font_faces {
        if let Err(error) = map.style_mut().set_fallback_font_faces(json) {
            return outbox.resolve(resolver, Err(error));
        }
    } else if options.system_fonts {
        map.style_mut().set_fallback_font_face_list(mln_system_fonts::fallback_font_face_list());
    }
    let _ = published.updates.set(map.update_reader());
    let flush_scheduled = Arc::new(AtomicBool::new(false));
    let scheduled = Arc::clone(&flush_scheduled);
    let run_loop = core.run_loop.clone();
    map.set_waker(Some(Arc::new(move || {
        if !scheduled.swap(true, Ordering::SeqCst) {
            post(&run_loop, move |core, outbox| flush_if_due(core, id, outbox));
        }
    })));
    published.applied.store(1, Ordering::SeqCst);
    core.maps.insert(
        id,
        MapEntry {
            core: map,
            published,
            mask: options.event_mask,
            applied: 1,
            pending_style: None,
            pending_still: None,
            session: None,
            flush_scheduled,
            ends: Ends::default(),
            callback_sources: CallbackSources::default(),
            geojson_updates: GeoJsonUpdates::default(),
        },
    );
    flush(core, id, outbox);
    if let Some(entry) = core.maps.get(&id) {
        outbox.resolve(resolver, Ok(entry.applied()));
    }
}

/// Runs a command on the map `id`, which must be there: handles what came
/// before it, applies it, publishes the map's state, sends its events, and
/// runs `after` with what `apply` returned. Then it tells the map's render
/// session that the command ran, so `after` can send the session what the
/// session must see first.
pub(crate) fn apply_command<T>(
    core: &mut RuntimeCore,
    id: MapId,
    sequence: u64,
    outbox: &mut Outbox,
    apply: impl FnOnce(&mut MapEntry, &mut Outbox) -> T,
    after: impl FnOnce(&MapEntry, T, Applied, &mut Outbox),
) {
    flush_if_due(core, id, outbox);
    let Some(entry) = core.maps.get_mut(&id) else { return };
    // The flush after the command takes what the command reports, and what
    // comes meanwhile, so the waker posts no flush of its own until then.
    entry.flush_scheduled.store(true, Ordering::SeqCst);
    let value = apply(entry, outbox);
    entry.applied = sequence;
    flush(core, id, outbox);
    let Some(entry) = core.maps.get(&id) else { return };
    after(entry, value, entry.applied(), outbox);
    entry.published.applied.store(sequence, Ordering::SeqCst);
    if let Some(link) = &entry.session
        && sequence >= link.awaited.load(Ordering::SeqCst)
    {
        worker_post(&link.run_loop, move |worker, outbox| worker.map_applied(sequence, outbox));
    }
}

/// Runs a command on the map `id` and completes with what `finish` makes of
/// its result and the map after it.
pub(crate) fn run_command<T, R: 'static>(
    core: &mut RuntimeCore,
    id: MapId,
    sequence: u64,
    apply: impl FnOnce(&mut MapEntry, &mut Outbox) -> Result<T, Error>,
    finish: impl FnOnce(T, &MapEntry) -> R,
    resolver: Resolver<R>,
    outbox: &mut Outbox,
) {
    if !core.maps.contains_key(&id) {
        return outbox.resolve(resolver, Err(missing_map()));
    }
    apply_command(core, id, sequence, outbox, apply, move |entry, result, _, outbox| {
        outbox.resolve(resolver, result.map(|value| finish(value, entry)));
    });
}

/// Starts a style load on the map `id`. Its completion waits for the load's
/// outcome, which a flush finds among the map's events.
fn load_style(
    core: &mut RuntimeCore,
    id: MapId,
    sequence: u64,
    load: StyleLoad,
    resolver: Resolver<Applied>,
    outbox: &mut Outbox,
) {
    if !core.maps.contains_key(&id) {
        return outbox.resolve(resolver, Err(missing_map()));
    }
    // The flush before the load sends the outcomes of earlier loads to their
    // own completions.
    apply_command(
        core,
        id,
        sequence,
        outbox,
        move |entry, outbox| {
            let from_json = matches!(load, StyleLoad::Json(_));
            if let Some(previous) = entry.pending_style.replace(PendingStyle { resolver, from_json }) {
                outbox.resolve(previous.resolver, Err(Error::cancelled("another style load started")));
            }
            let style = entry.core.style_mut();
            match load {
                StyleLoad::Url(url) => style.load_url(&url),
                StyleLoad::Json(json) => style.load_json(&json),
            }
        },
        |_, (), _, _| {},
    );
}

/// Flushes the map `id` unless a flush ran since its waker last did, so
/// that there is nothing to flush.
fn flush_if_due(core: &mut RuntimeCore, id: MapId, outbox: &mut Outbox) {
    if core.maps.get(&id).is_some_and(|entry| entry.flush_scheduled.load(Ordering::SeqCst)) {
        flush(core, id, outbox);
    }
}

/// Handles what the map's style and renderer reported, publishes its state,
/// and sends its events.
pub(crate) fn flush(core: &mut RuntimeCore, id: MapId, outbox: &mut Outbox) {
    let Some(entry) = core.maps.get_mut(&id) else { return };
    entry.flush_scheduled.store(false, Ordering::SeqCst);
    let events = entry.core.drain_events();
    // The sources that left the style, by a command or a style load, let go
    // of what they held before anything reports the change.
    crate::style::reconcile(entry, outbox);
    let generation = entry.core.generation();
    *entry.published.snapshot.lock() = Some(entry.core.snapshot());

    let mut style_outcome = None;
    let from_json = entry.pending_style.as_ref().is_some_and(|pending| pending.from_json);
    for event in &events {
        match event {
            MapEvent::StyleLoaded => style_outcome = Some(Ok(())),
            MapEvent::LoadingFailed(StyleLoadError::Parse(message)) if from_json => {
                style_outcome = Some(Err(Error::invalid_argument(message)));
            }
            MapEvent::LoadingFailed(error) => style_outcome = Some(Err(Error::native(error.message()))),
            _ => {}
        }
    }
    let style_outcome = style_outcome.map(|outcome| (outcome, entry.pending_style.take(), entry.applied()));
    let still_outcome = entry.pending_still.as_ref().and_then(|pending| pending.outcome.borrow_mut().take());
    let still = still_outcome.and_then(|outcome| Some((outcome, entry.pending_still.take()?.resolver)));
    // The session demands a frame for the image while it waits, and lets
    // the demand go once it doesn't.
    let still_image = entry.pending_still.as_ref().and(entry.core.pending_still_image()).unwrap_or(0);
    if entry.published.still_image.swap(still_image, Ordering::SeqCst) != still_image
        && let Some(link) = &entry.session
    {
        worker_post(&link.run_loop, Worker::still_image_changed);
    }

    let mask = entry.mask;
    let events: Vec<RuntimeEvent> = events
        .into_iter()
        .map(RuntimeEventKind::Map)
        .filter(|kind| mask.contains(kind.event_type()))
        .map(|kind| RuntimeEvent { source: EventSource::Map { id, generation }, kind })
        .collect();
    let ends = entry.ends.clone();
    core.send_events(events, outbox);
    ends.deliver(generation, outbox);

    if let Some((outcome, Some(pending), applied)) = style_outcome {
        outbox.resolve(pending.resolver, outcome.map(|()| applied));
    }
    if let Some((outcome, resolver)) = still {
        outbox.resolve(resolver, outcome);
    }
}

/// Lets go of the map `id`, after sending the events it still had.
fn remove(core: &mut RuntimeCore, id: MapId, done: Option<Resolver<()>>, outbox: &mut Outbox) {
    flush(core, id, outbox);
    if let Some(mut entry) = core.maps.remove(&id) {
        entry.core.set_waker(None);
        if let Some(pending) = entry.pending_style.take() {
            outbox.resolve(pending.resolver, Err(Error::cancelled("the map was released")));
        }
        if let Some(pending) = entry.pending_still.take() {
            outbox.resolve(pending.resolver, Err(Error::cancelled("the map was released")));
        }
        crate::style::release(&mut entry, outbox);
        // The map's camera changes end as closed as it drops, with no
        // snapshot to show them.
        let ends = entry.ends.clone();
        drop(entry);
        ends.deliver(0, outbox);
    }
    if let Some(done) = done {
        outbox.resolve(done, Ok(()));
    }
}
