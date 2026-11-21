//! Ring targets: a [`BorrowedTarget`] draws frames into a ring of the host's
//! textures and gives each back, and an [`ExportedTarget`] draws into a ring
//! of images of its own, which it exports, through a borrowed target over
//! each generation of them.
//!
//! A backend implements [`RingTextures`] for the host's textures,
//! [`RingBackend`], which imports them into its `Gpu` and makes the sync with
//! which each side waits for the other, and [`Exporter`], which allocates an
//! exported ring's images. The targets here do the rest: which slot a frame
//! draws into, which of the host's reads it waits for, what a texture that
//! the ring has again keeps across a replacement, and the older generations
//! that a resize leaves to the frames the host holds.

use mln_util::error::Error;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::export::{ExportKind, ExportedFrame, ExportedRing, Generations};
use crate::frame::{ConsumerSync, FrameImage, FrameTarget, GpuSync, Handoff, SyncKind};
use crate::gpu::{Gpu, GpuError, ResourceId, TargetFormats, TextureDesc, TextureId, TextureUsage, destroy_unused};
use crate::renderer_backend::RendererBackend;

/// The host's textures of a ring: one color texture for each slot, and
/// either one depth and stencil texture for each slot or none, in which
/// case frames draw into one of the target's own.
pub trait RingTextures {
    /// One of the host's color textures, as the backend's API names it.
    type Image;

    /// The color textures, one for each slot.
    fn images(&self) -> &[Self::Image];

    /// Whether `a` and `b` are the same texture of the host's.
    fn same_image(a: &Self::Image, b: &Self::Image) -> bool;

    /// The size of the textures, in physical pixels.
    fn size(&self) -> Size;

    /// Whether the host gave depth and stencil textures.
    fn has_depth(&self) -> bool;
}

/// A backend's part of a [`BorrowedTarget`]: the objects that its frames
/// signal, and how the host's textures and syncs become textures and waits
/// of its `Gpu`.
pub trait RingBackend: Sized {
    /// The backend's `Gpu`, which the host's textures are imported into.
    type Gpu: Gpu;
    /// The backend that the target is built on.
    type Renderer: RendererBackend<Gpu = Self::Gpu>;
    /// The host's textures.
    type Textures: RingTextures;
    /// What the next frame drawn into a slot waits for: the host's reads of
    /// the slot's texture.
    type Wait;
    /// What a slot's last frame handed the host and the target keeps until
    /// the host gives the slot back, such as a sync file.
    type Handed;

    /// The ring's objects for frames drawn into `textures` on `backend` and
    /// handed to the host with sync of the kind `producer`, once both are
    /// checked: `textures` against the backend's device, and `producer`
    /// against the kinds with which the backend hands frames over.
    fn new(backend: &mut Self::Renderer, textures: &Self::Textures, producer: SyncKind) -> Result<Self, Error>;

    /// Checks that `next`, as many textures as `current`, may replace them:
    /// textures of their formats, with depth textures where they have them,
    /// and as [`new`](Self::new) checks them.
    fn check_replacement(&self, current: &Self::Textures, next: &Self::Textures) -> Result<(), Error>;

    /// The formats that frames draw into `textures` in. Where the host gave
    /// no depth textures, the depth and stencil format is that of the
    /// target's own, which it names.
    fn targets(gpu: &Self::Gpu, textures: &Self::Textures) -> Result<TargetFormats, Error>;

    /// Imports the host's color texture at `slot` into `gpu`, or with
    /// `depth`, its depth and stencil texture.
    fn import(gpu: &mut Self::Gpu, textures: &Self::Textures, slot: usize, depth: bool) -> Result<TextureId, GpuError>;

    /// What the next frame drawn into a slot waits for, once the host gave
    /// the slot back with `consumer`, or `None` when it waits for nothing.
    fn wait_for(&mut self, gpu: &mut Self::Gpu, consumer: ConsumerSync) -> Result<Option<Self::Wait>, Error>;

    /// Has the next submit of `gpu` that draws into `texture` wait for
    /// `wait`.
    fn wait_before_using(gpu: &mut Self::Gpu, texture: TextureId, wait: &Self::Wait) -> Result<(), GpuError>;

    /// Has the next submit of `gpu` wait for what `texture` waits for,
    /// though nothing drew into it.
    fn take_waits_into_next_submit(gpu: &mut Self::Gpu, texture: TextureId);

    /// Drops what `texture` waits for, which no submit of `gpu` takes now.
    /// Returns whether it waited for something, which no pass took: false
    /// where a pass that drew into it took its waits into a submit.
    fn forget_waits(gpu: &mut Self::Gpu, texture: TextureId) -> bool;

    /// A submit of `gpu` took `wait`, or may have, which the ring lets go of
    /// once the work submitted so far and the next submit's is done.
    fn waited(gpu: &mut Self::Gpu, wait: Self::Wait) {
        let _ = (gpu, wait);
    }

    /// Has the next submit of `gpu` signal the object that the host waits
    /// on for a frame, for a producer kind that names one.
    fn signal_after_next_submit(&mut self, gpu: &mut Self::Gpu);

    /// The sync that the host waits for, once the frame was submitted, for
    /// a producer kind that names an object, and what the slot keeps of it
    /// until the host gives it back. `None` when there is none, which drops
    /// the frame. A frame that hands nothing to keep leaves what the slot
    /// kept of an earlier frame until the host gives the slot back.
    fn producer_sync(&mut self, gpu: &mut Self::Gpu) -> Option<(GpuSync, Option<Self::Handed>)>;

    /// Lets go of the objects that the frames signal once the work
    /// submitted so far is done, as the target goes.
    fn tear_down(&mut self, gpu: &mut Self::Gpu) {
        let _ = gpu;
    }

    /// Blocks until the work submitted so far is done, where the backend
    /// must before a target dropped without its tear-down, and so without
    /// the `Gpu` to defer to, lets go of what the frames used. Logs `what`
    /// was going on when the wait fails.
    fn wait_for_frames(&self, what: &str) {
        let _ = what;
    }

    /// Lets go of `wait`, which no submit takes.
    fn forget(wait: Self::Wait) {
        let _ = wait;
    }
}

/// A ring's textures as textures of the backend's `Gpu`, with their depth
/// and stencil textures: the host's, or one of the target's own.
pub struct Imports {
    size: Size,
    targets: TargetFormats,
    color: Vec<TextureId>,
    /// The host's depth and stencil textures, one for each slot.
    depth: Vec<TextureId>,
    /// The target's own depth and stencil, when the host gave none.
    own_depth: Option<TextureId>,
}

impl Imports {
    /// Imports `textures` into `gpu`, and makes a depth and stencil texture
    /// of the target's own where the host gave none.
    ///
    /// Fails as [`RingBackend::targets`] does, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) when `gpu` can't import a
    /// texture or make the depth and stencil texture. What it imported is
    /// then destroyed.
    pub fn new<B: RingBackend>(gpu: &mut B::Gpu, textures: &B::Textures) -> Result<Imports, Error> {
        let mut imports = Imports {
            size: textures.size(),
            targets: B::targets(gpu, textures)?,
            color: Vec::new(),
            depth: Vec::new(),
            own_depth: None,
        };
        match imports.import::<B>(gpu, textures) {
            Ok(()) => Ok(imports),
            Err(error) => {
                imports.tear_down(gpu);
                Err(error)
            }
        }
    }

    fn import<B: RingBackend>(&mut self, gpu: &mut B::Gpu, textures: &B::Textures) -> Result<(), Error> {
        let import = |gpu: &mut B::Gpu, slot, depth| {
            B::import(gpu, textures, slot, depth)
                .map_err(|error| Error::native(format!("Importing a borrowed texture failed: {error}")))
        };
        let slots = textures.images().len();
        for slot in 0..slots {
            self.color.push(import(gpu, slot, false)?);
        }
        if textures.has_depth() {
            for slot in 0..slots {
                self.depth.push(import(gpu, slot, true)?);
            }
        } else {
            let format = self
                .targets
                .depth_stencil
                .ok_or_else(|| Error::unsupported("the ring names no format of its depth and stencil"))?;
            let desc = TextureDesc {
                label: "BorrowedDepthStencil",
                size: self.size,
                format,
                usage: TextureUsage { sampled: false, render_target: true, copy_src: false },
            };
            let texture = gpu
                .create_texture(&desc)
                .map_err(|error| Error::native(format!("Making the depth and stencil texture failed: {error}")))?;
            self.own_depth = Some(texture);
        }
        Ok(())
    }

    /// The textures that a frame drawn into `slot` draws into.
    fn frame_image(&self, slot: usize) -> Option<FrameImage> {
        Some(FrameImage {
            size: self.size,
            color: *self.color.get(slot)?,
            depth_stencil: self.own_depth.or_else(|| self.depth.get(slot).copied()),
            targets: self.targets,
        })
    }

    /// Destroys the imports in `gpu`, which keeps what the work submitted
    /// so far uses until it finishes.
    fn tear_down(&mut self, gpu: &mut (impl Gpu + ?Sized)) {
        let textures = self.color.drain(..).chain(self.depth.drain(..)).chain(self.own_depth.take());
        destroy_unused(gpu, textures.map(ResourceId::Texture));
    }
}

/// What the next frame drawn into a slot waits for, and what the slot's
/// last frame handed the host.
struct Slot<B: RingBackend> {
    wait: Option<B::Wait>,
    /// The texture that has the next submit that draws into it wait for
    /// `wait`, from the frame begun on the slot until it ends.
    attached: Option<TextureId>,
    handed: Option<B::Handed>,
}

impl<B: RingBackend> Slot<B> {
    fn new(wait: Option<B::Wait>) -> Self {
        Slot { wait, attached: None, handed: None }
    }

    /// Lets go of what the slot waits for, and of what the slot handed the
    /// host: at once, unless a pass of a frame begun on the slot took the
    /// wait into a submit, which the wait then outlives.
    fn forget(&mut self, gpu: &mut B::Gpu) {
        let taken = self.detach(gpu);
        if let Some(wait) = self.wait.take() {
            if taken { B::waited(gpu, wait) } else { B::forget(wait) }
        }
        self.handed = None;
    }

    /// Ends the frame begun on the slot, if any, without a submit of its
    /// own. Returns whether a pass that drew took the slot's wait into a
    /// submit; otherwise the wait is the next frame's still.
    fn detach(&mut self, gpu: &mut B::Gpu) -> bool {
        self.attached.take().is_some_and(|texture| !B::forget_waits(gpu, texture))
    }
}

/// Frames drawn into the host's textures, each given back to the host.
pub struct BorrowedTarget<B: RingBackend> {
    /// `None` once the target let go of them.
    textures: Option<B::Textures>,
    imports: Imports,
    /// The slot the next frame draws into.
    selected: usize,
    slots: Vec<Slot<B>>,
    producer: SyncKind,
    /// What the sync of the last release or replacement of the textures
    /// handed the host, until the next one or the target goes.
    handed: Option<B::Handed>,
    /// Whether a tear-down deferred the release of what the frames use,
    /// after which dropping the target needs no wait.
    torn_down: bool,
    ring: B,
}

impl<B: RingBackend> BorrowedTarget<B> {
    /// A target on `backend` that draws into `textures` and hands each frame
    /// to the host with sync of the kind `producer`.
    ///
    /// Fails as [`RingBackend::new`] and [`Imports::new`] do.
    pub fn new(backend: &mut B::Renderer, textures: B::Textures, producer: SyncKind) -> Result<Self, Error> {
        let ring = B::new(backend, &textures, producer)?;
        let imports = Imports::new::<B>(&mut backend.gpu(), &textures)?;
        let slots = textures.images().iter().map(|_| Slot::new(None)).collect();
        Ok(BorrowedTarget {
            textures: Some(textures),
            imports,
            selected: 0,
            slots,
            producer,
            handed: None,
            torn_down: false,
            ring,
        })
    }

    /// Has the next frame draw into the texture at `slot`.
    pub fn select(&mut self, slot: usize) {
        self.selected = slot;
    }

    /// The size of the textures that frames draw into, or drew into last.
    #[must_use]
    pub fn size(&self) -> Size {
        self.imports.size
    }

    /// The host gave the texture at `slot` back: the next frame drawn into
    /// it waits for `consumer`, and the target lets go of what the slot's
    /// last frame handed over.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a
    /// slot the ring doesn't have, and as [`RingBackend::wait_for`] does,
    /// after which the next frame drawn into the slot waits for nothing.
    pub fn release(&mut self, gpu: &mut B::Gpu, slot: usize, consumer: ConsumerSync) -> Result<(), Error> {
        let Some(state) = self.slots.get_mut(slot) else {
            return Err(Error::invalid_argument("the ring has no such slot"));
        };
        state.forget(gpu);
        state.wait = self.ring.wait_for(gpu, consumer)?;
        Ok(())
    }

    /// The host gave up the texture at `slot` without saying when its reads
    /// end: the target lets go of what the slot's last frame handed over,
    /// and never draws into the texture again.
    pub fn discard(&mut self, gpu: &mut B::Gpu, slot: usize) {
        if let Some(state) = self.slots.get_mut(slot) {
            state.forget(gpu);
        }
    }

    /// Draws into `textures` from now on, which must be as many, and which
    /// the backend checks as [`RingBackend::check_replacement`] does. A
    /// texture that the new ring has again keeps what its next frame waits
    /// for. Returns the sync after which the host may use the textures that
    /// the ring let go of, as [`release_textures`](Self::release_textures)
    /// does.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for
    /// another number of textures, with
    /// [`ErrorKind::InvalidState`](mln_util::ErrorKind) once the target let
    /// go of its textures, as the check and [`Imports::new`] do, and as
    /// [`release_textures`](Self::release_textures) does. The target then
    /// keeps its textures.
    pub fn replace(&mut self, gpu: &mut B::Gpu, textures: B::Textures) -> Result<GpuSync, Error> {
        let Some(current) = &self.textures else {
            return Err(Error::invalid_state("the ring let go of its textures"));
        };
        if textures.images().len() != current.images().len() {
            return Err(Error::invalid_argument("a ring keeps its depth"));
        }
        self.ring.check_replacement(current, &textures)?;
        let mut imports = Imports::new::<B>(gpu, &textures)?;
        let kept: Vec<Option<usize>> = current
            .images()
            .iter()
            .map(|old| textures.images().iter().position(|image| B::Textures::same_image(old, image)))
            .collect();
        // A texture that the new ring has again keeps its wait for its
        // next frame, unless a pass of the frame begun on it took it.
        for (slot, _) in self.slots.iter_mut().zip(&kept).filter(|(_, kept)| kept.is_some()) {
            if slot.detach(gpu)
                && let Some(wait) = slot.wait.take()
            {
                B::waited(gpu, wait);
            }
        }
        let sync = match self.end_work(gpu) {
            Ok(sync) => sync,
            Err(error) => {
                imports.tear_down(gpu);
                return Err(error);
            }
        };
        std::mem::replace(&mut self.imports, imports).tear_down(gpu);
        let mut slots: Vec<Slot<B>> = textures.images().iter().map(|_| Slot::new(None)).collect();
        for (mut slot, kept) in std::mem::take(&mut self.slots).into_iter().zip(kept) {
            match kept.and_then(|index| slots.get_mut(index)) {
                Some(next) => next.wait = slot.wait.take(),
                None => slot.forget(gpu),
            }
        }
        self.slots = slots;
        self.textures = Some(textures);
        Ok(sync)
    }

    /// Lets go of the textures, and returns the sync after which the host
    /// may use them: the target's GPU work on them, and its waits for the
    /// host, end there. The target keeps the objects that the sync names
    /// until its next release or replacement of textures, or until it goes,
    /// and draws no frame until then.
    ///
    /// Fails with [`ErrorKind::InvalidState`](mln_util::ErrorKind) once the
    /// target let go of its textures, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) when it can't submit the
    /// work or make the sync. It keeps its textures then.
    pub fn release_textures(&mut self, gpu: &mut B::Gpu) -> Result<GpuSync, Error> {
        if self.textures.is_none() {
            return Err(Error::invalid_state("the ring let go of its textures"));
        }
        let sync = self.end_work(gpu)?;
        for mut slot in std::mem::take(&mut self.slots) {
            slot.forget(gpu);
        }
        self.imports.tear_down(gpu);
        self.textures = None;
        Ok(sync)
    }

    /// Submits the work recorded so far, with a signal of the producer's
    /// sync after it, and returns that sync, which ends the work on the
    /// textures. A frame begun and not ended lets go of what its slot waits
    /// for: a pass that drew took the wait into the submit, which the wait
    /// outlives, and a frame that drew nothing waits for no host.
    fn end_work(&mut self, gpu: &mut B::Gpu) -> Result<GpuSync, Error> {
        for slot in self.slots.iter_mut().filter(|slot| slot.attached.is_some()) {
            slot.forget(gpu);
        }
        self.ring.signal_after_next_submit(gpu);
        gpu.flush(false).map_err(|error| Error::native(format!("Submitting a ring's frames failed: {error}")))?;
        let (sync, handed) =
            self.producer_sync(gpu).ok_or_else(|| Error::native("Making the sync of a ring's frames failed"))?;
        self.handed = handed;
        Ok(sync)
    }

    /// The sync that the host waits for, once the frame was submitted, and
    /// what the slot keeps of it.
    fn producer_sync(&mut self, gpu: &mut B::Gpu) -> Option<(GpuSync, Option<B::Handed>)> {
        match self.producer {
            SyncKind::QueueOrder => Some((GpuSync::of(SyncKind::QueueOrder), None)),
            SyncKind::CpuComplete => match gpu.flush(true) {
                Ok(()) => Some((GpuSync::of(SyncKind::CpuComplete), None)),
                Err(error) => {
                    Log::error(Event::Render, format!("Waiting for a borrowed texture's frame failed: {error}"));
                    None
                }
            },
            _ => self.ring.producer_sync(gpu),
        }
    }
}

impl<B: RingBackend> FrameTarget for BorrowedTarget<B> {
    type Gpu = B::Gpu;

    /// The selected slot's texture, which the frame's first submit that
    /// draws into it waits for the host's reads before.
    fn begin_frame(&mut self, gpu: &mut B::Gpu) -> Option<FrameImage> {
        let image = self.imports.frame_image(self.selected)?;
        // A frame begun again keeps the wait it attached.
        if let Some(state) = self.slots.get_mut(self.selected)
            && state.attached != Some(image.color)
            && let Some(wait) = &state.wait
        {
            if let Err(error) = B::wait_before_using(gpu, image.color, wait) {
                Log::error(Event::Render, format!("Waiting for the host's reads failed: {error}"));
            }
            state.attached = Some(image.color);
        }
        Some(image)
    }

    /// Submits what is left of the frame, signaling the producer's sync
    /// after it.
    fn end_frame(&mut self, gpu: &mut B::Gpu) -> Handoff {
        let slot = self.selected;
        let Some(color) = self.imports.color.get(slot).copied() else {
            return Handoff::Dropped;
        };
        // What no pass took yet, such as where the frame drew nothing.
        B::take_waits_into_next_submit(gpu, color);
        self.ring.signal_after_next_submit(gpu);
        let submitted = gpu.flush(false);
        // The frame's submits took what the slot waited for.
        if let Some(state) = self.slots.get_mut(slot)
            && state.attached.take() == Some(color)
            && let Some(wait) = state.wait.take()
        {
            B::waited(gpu, wait);
        }
        if let Err(error) = submitted {
            Log::error(Event::Render, format!("Submitting a borrowed texture's frame failed: {error}"));
            return Handoff::Dropped;
        }
        let Some((sync, handed)) = self.producer_sync(gpu) else {
            return Handoff::Dropped;
        };
        if let Some(handed) = handed
            && let Some(state) = self.slots.get_mut(slot)
        {
            state.handed = Some(handed);
        }
        Handoff::GivenBack { slot, sync }
    }

    /// Lets go of what the slots wait for, submits the work recorded so
    /// far, and destroys the imports and the ring's objects once the frames
    /// drawn so far are done, without blocking.
    fn tear_down(&mut self, gpu: &mut B::Gpu) {
        for mut slot in std::mem::take(&mut self.slots) {
            slot.forget(gpu);
        }
        if let Err(error) = gpu.flush(false) {
            Log::error(
                Event::Render,
                format!("Submitting the frames before a borrowed target's deletion failed: {error}"),
            );
        }
        self.imports.tear_down(gpu);
        self.ring.tear_down(gpu);
        self.torn_down = true;
    }
}

impl<B: RingBackend> Drop for BorrowedTarget<B> {
    /// Lets go of what the slots wait for, after the frames drawn so far
    /// where the backend must wait and no tear-down deferred it. The wait of a
    /// frame begun and not ended drops without [`RingBackend::forget`],
    /// which leaks what the backend made for it: a submit to come may have
    /// taken it, and without the `Gpu` nothing defers its release past that
    /// submit.
    fn drop(&mut self) {
        if !self.torn_down {
            self.ring.wait_for_frames("borrowed target deletion");
        }
        for slot in self.slots.drain(..) {
            if slot.attached.is_none()
                && let Some(wait) = slot.wait
            {
                B::forget(wait);
            }
        }
    }
}

/// A backend's part of an [`ExportedTarget`]: the images that it allocates
/// and exports, and the host's textures over them, which a
/// [`BorrowedTarget`] draws into.
pub trait Exporter: Sized {
    /// The backend's part of the borrowed target over a generation's images.
    type Ring: RingBackend;
    /// One generation's images, which go after the target that draws into
    /// them.
    type Images;

    /// What allocates the images of `ring`, which the backend shares in
    /// `kind` and hands over with sync of the kind `producer`.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) when the
    /// backend can't export `kind` or hand frames over with `producer`.
    fn new(
        backend: &mut <Self::Ring as RingBackend>::Renderer,
        ring: &ExportedRing,
        kind: ExportKind,
        producer: SyncKind,
    ) -> Result<Self, Error>;

    /// A generation of images of `size` pixels, and the textures over them
    /// that frames draw into.
    fn allocate(
        &self,
        backend: &mut <Self::Ring as RingBackend>::Renderer,
        size: Size,
    ) -> Result<(Self::Images, <Self::Ring as RingBackend>::Textures), Error>;

    /// Lets go of `images`, which `gpu`'s frames drew into, once the work
    /// submitted so far is done.
    fn release_images(gpu: &mut <Self::Ring as RingBackend>::Gpu, images: Self::Images) {
        let _ = gpu;
        drop(images);
    }

    /// The frame at `slot` of a generation of `images`, whose textures are
    /// `textures`.
    fn frame(
        &self,
        images: &Self::Images,
        textures: &<Self::Ring as RingBackend>::Textures,
        slot: usize,
    ) -> Option<ExportedFrame>;
}

/// One generation of an exported ring's images, of one size, and the
/// borrowed target that draws into them.
struct Generation<E: Exporter> {
    // Before the images: dropping it without a tear-down waits, where the
    // backend must, for the frames that drew into them.
    target: BorrowedTarget<E::Ring>,
    /// The images, until a tear-down hands them to the exporter.
    images: Option<E::Images>,
}

impl<E: Exporter> Generation<E> {
    fn new(
        exporter: &E,
        backend: &mut <E::Ring as RingBackend>::Renderer,
        size: Size,
        producer: SyncKind,
    ) -> Result<Self, Error> {
        let (images, textures) = exporter.allocate(backend, size)?;
        let target = BorrowedTarget::new(backend, textures, producer)?;
        Ok(Generation { target, images: Some(images) })
    }

    /// Tears the target down, and lets the exporter release the images
    /// once the frames that drew into them are done.
    fn tear_down(&mut self, gpu: &mut <E::Ring as RingBackend>::Gpu) {
        self.target.tear_down(gpu);
        if let Some(images) = self.images.take() {
            E::release_images(gpu, images);
        }
    }
}

/// Frames drawn into images of the target's own, which it exports, and
/// queued for the host to acquire.
pub struct ExportedTarget<E: Exporter> {
    exporter: E,
    producer: SyncKind,
    generations: Generations<Generation<E>>,
}

impl<E: Exporter> ExportedTarget<E> {
    /// A target on `backend` that draws into `ring.depth` images of `size`
    /// pixels, exported as `kind`, and hands each frame over with sync of
    /// the kind `producer`.
    ///
    /// Fails as [`Exporter::new`], [`Exporter::allocate`] and
    /// [`BorrowedTarget::new`] do.
    pub fn new(
        backend: &mut <E::Ring as RingBackend>::Renderer,
        ring: &ExportedRing,
        kind: ExportKind,
        size: Size,
        producer: SyncKind,
    ) -> Result<Self, Error> {
        let exporter = E::new(backend, ring, kind, producer)?;
        let current = Generation::new(&exporter, backend, size, producer)?;
        Ok(ExportedTarget { exporter, producer, generations: Generations::new(current) })
    }

    /// Has the next frame draw into the image at `slot`.
    pub fn select(&mut self, slot: usize) {
        self.generations.current_mut().target.select(slot);
    }

    /// The host gave the frame at `slot` back, once its reads reach
    /// `consumer`. A frame of an older generation lets go of its image,
    /// which no frame draws into again.
    ///
    /// Fails as [`BorrowedTarget::release`] does.
    pub fn release(
        &mut self,
        gpu: &mut <E::Ring as RingBackend>::Gpu,
        slot: usize,
        consumer: ConsumerSync,
    ) -> Result<(), Error> {
        if self.generations.give_back(slot, |mut generation| generation.tear_down(gpu)) {
            return Ok(());
        }
        self.generations.current_mut().target.release(gpu, slot, consumer)
    }

    /// The host gave the frame at `slot` up without saying when its reads
    /// end.
    pub fn discard(&mut self, gpu: &mut <E::Ring as RingBackend>::Gpu, slot: usize) {
        if !self.generations.give_back(slot, |mut generation| generation.tear_down(gpu)) {
            self.generations.current_mut().target.discard(gpu, slot);
        }
    }

    /// Draws into images of `size` pixels from now on. The frames that the
    /// host holds, at `held` slots, keep their images. Returns whether the
    /// ring has new images, which it doesn't when they have `size` already.
    ///
    /// Fails as [`new`](Self::new) does, and the target then keeps its own
    /// images.
    pub fn resize(
        &mut self,
        backend: &mut <E::Ring as RingBackend>::Renderer,
        size: Size,
        held: &[usize],
    ) -> Result<bool, Error> {
        if self.size() == size {
            return Ok(false);
        }
        let next = Generation::new(&self.exporter, backend, size, self.producer)?;
        let mut gpu = backend.gpu();
        self.generations.replace(next, held, |mut generation| generation.tear_down(&mut gpu));
        Ok(true)
    }

    /// The size of the images that frames draw into.
    #[must_use]
    pub fn size(&self) -> Size {
        self.generations.current().target.size()
    }

    /// The images of the frame at `slot`: of the generation that the host
    /// holds it in.
    #[must_use]
    pub fn image(&self, slot: usize) -> Option<ExportedFrame> {
        let generations = &self.generations;
        let generation = generations.retired(slot).unwrap_or(generations.current());
        self.exporter.frame(generation.images.as_ref()?, generation.target.textures.as_ref()?, slot)
    }
}

impl<E: Exporter> FrameTarget for ExportedTarget<E> {
    type Gpu = <E::Ring as RingBackend>::Gpu;

    fn begin_frame(&mut self, gpu: &mut Self::Gpu) -> Option<FrameImage> {
        self.generations.current_mut().target.begin_frame(gpu)
    }

    fn end_frame(&mut self, gpu: &mut Self::Gpu) -> Handoff {
        match self.generations.current_mut().target.end_frame(gpu) {
            Handoff::GivenBack { slot, sync } => Handoff::Queued { slot, sync },
            other => other,
        }
    }

    fn tear_down(&mut self, gpu: &mut Self::Gpu) {
        self.generations.clear(|generation| generation.tear_down(gpu));
    }
}

#[cfg(test)]
mod tests {
    use std::any::Any;
    use std::cell::RefCell;

    use mln_util::hash::HashMap;

    use super::*;
    use crate::export::{ExportedImage, NativeTexture};
    use crate::gpu::{
        BindingsDesc, BindingsId, BufferDesc, BufferId, Caps, ClipDepthRange, Draw, GeometryDesc, GeometryId, PassDesc,
        PipelineDesc, PipelineId, ReadbackId, SamplerId, ShaderId, ShaderKey, SlotMap, TextureFormat, TextureRegion,
    };
    use crate::texture2d::SamplerState;

    /// A `Gpu` of textures alone, which records what its submits wait for.
    #[derive(Default)]
    struct FakeGpu {
        /// Each texture, and the host's image it imports, if any.
        textures: SlotMap<TextureId, Option<u32>>,
        texture_waits: HashMap<TextureId, Vec<u32>>,
        next_waits: Vec<u32>,
        /// What each flush waited for.
        submits: Vec<Vec<u32>>,
        signals: u64,
        /// The waits that the ring let go of after a submit.
        waited: Vec<u32>,
        /// Whether flushes fail, as on a lost device.
        lost: bool,
    }

    impl FakeGpu {
        fn imported(&self) -> Vec<u32> {
            let mut images: Vec<u32> = self.textures.values().filter_map(|image| *image).collect();
            images.sort_unstable();
            images
        }
    }

    fn unsupported<T>() -> Result<T, GpuError> {
        Err(GpuError::Unsupported("a fake".to_owned()))
    }

    impl Gpu for FakeGpu {
        fn caps(&self) -> &Caps {
            const CAPS: Caps = Caps {
                buffer_offset_alignment: 4,
                max_texture_size: 4096,
                half_float_render_targets: false,
                clip_depth: ClipDepthRange::ZeroToOne,
                render_target_depth: None,
                clear_stencil_with_pass: false,
                cpu_rasterizer: false,
                max_line_width: 1.0,
                binds_vertex_buffers: false,
            };
            &CAPS
        }
        fn create_buffer(&mut self, _: &BufferDesc<'_>) -> Result<BufferId, GpuError> {
            unsupported()
        }
        fn write_buffer(&mut self, _: BufferId, _: u64, _: &[u8]) -> Result<(), GpuError> {
            unsupported()
        }
        fn create_texture(&mut self, desc: &TextureDesc<'_>) -> Result<TextureId, GpuError> {
            if desc.size.width == 0 {
                return Err(GpuError::Invalid("an empty texture".to_owned()));
            }
            Ok(self.textures.insert(None))
        }
        fn write_texture(&mut self, _: TextureId, _: TextureRegion, _: &[u8]) -> Result<(), GpuError> {
            unsupported()
        }
        fn create_sampler(&mut self, _: &SamplerState) -> Result<SamplerId, GpuError> {
            unsupported()
        }
        fn create_shader(&mut self, _: &ShaderKey) -> Result<ShaderId, GpuError> {
            unsupported()
        }
        fn create_pipeline(&mut self, _: &PipelineDesc<'_>) -> Result<PipelineId, GpuError> {
            unsupported()
        }
        fn create_bindings(&mut self, _: &BindingsDesc<'_>) -> Result<BindingsId, GpuError> {
            unsupported()
        }
        fn create_geometry(&mut self, _: &GeometryDesc<'_>) -> Result<GeometryId, GpuError> {
            unsupported()
        }
        fn destroy(&mut self, resource: ResourceId) -> Result<(), GpuError> {
            if let ResourceId::Texture(texture) = resource {
                self.textures.remove(texture);
                self.texture_waits.remove(&texture);
            }
            Ok(())
        }
        fn begin_pass(&mut self, _: &PassDesc<'_>) -> Result<(), GpuError> {
            unsupported()
        }
        fn draw(&mut self, _: &Draw) -> Result<(), GpuError> {
            unsupported()
        }
        fn lend(&mut self, _: &mut dyn FnMut(&mut dyn Any)) -> Result<(), GpuError> {
            unsupported()
        }
        fn end_pass(&mut self) -> Result<(), GpuError> {
            unsupported()
        }
        fn flush(&mut self, _wait: bool) -> Result<(), GpuError> {
            if self.lost {
                return Err(GpuError::Invalid("lost".to_owned()));
            }
            self.submits.push(std::mem::take(&mut self.next_waits));
            Ok(())
        }
        fn read_texture(&mut self, _: TextureId, _: TextureRegion) -> Result<ReadbackId, GpuError> {
            unsupported()
        }
        fn take_readback(&mut self, _: ReadbackId, _: bool) -> Result<Option<Vec<u8>>, GpuError> {
            unsupported()
        }
    }

    #[derive(Default)]
    struct FakeBackend {
        gpu: FakeGpu,
        /// The name of the next image that an exporter allocates.
        next_image: u32,
    }

    impl RendererBackend for FakeBackend {
        type Gpu = FakeGpu;
        type Scope<'a> = &'a mut FakeGpu;

        fn gpu(&mut self) -> &mut FakeGpu {
            &mut self.gpu
        }
    }

    /// The host's images, by name, and whether it gave depth textures,
    /// which are named one past each image's name.
    #[derive(Clone)]
    struct Images {
        names: Vec<u32>,
        size: Size,
        depth: bool,
    }

    fn images(names: &[u32]) -> Images {
        Images { names: names.to_vec(), size: Size::new(4, 4), depth: false }
    }

    impl RingTextures for Images {
        type Image = u32;

        fn images(&self) -> &[u32] {
            &self.names
        }
        fn same_image(a: &u32, b: &u32) -> bool {
            a == b
        }
        fn size(&self) -> Size {
            self.size
        }
        fn has_depth(&self) -> bool {
            self.depth
        }
    }

    /// What the test's rings did apart from their `Gpu`.
    #[derive(Default)]
    struct Record {
        /// How many times they waited for the frames drawn so far.
        frames_waited: usize,
        /// How many times a tear-down deferred the release of their objects.
        torn_down: usize,
        /// The waits that they let go of without a submit.
        forgotten: Vec<u32>,
    }

    thread_local! {
        static RECORD: RefCell<Record> = RefCell::default();
    }

    fn frames_waited() -> usize {
        RECORD.with_borrow(|record| record.frames_waited)
    }

    fn torn_down() -> usize {
        RECORD.with_borrow(|record| record.torn_down)
    }

    fn forgotten() -> Vec<u32> {
        RECORD.with_borrow(|record| record.forgotten.clone())
    }

    /// A ring whose waits are reads by number, and whose frames signal a
    /// counter.
    struct FakeRing {
        value: u64,
    }

    impl RingBackend for FakeRing {
        type Gpu = FakeGpu;
        type Renderer = FakeBackend;
        type Textures = Images;
        type Wait = u32;
        /// The value of the frame's signal.
        type Handed = u64;

        fn new(_: &mut FakeBackend, images: &Images, producer: SyncKind) -> Result<Self, Error> {
            if images.names.is_empty() {
                return Err(Error::invalid_argument("no images"));
            }
            if producer == SyncKind::SyncFd {
                return Err(Error::unsupported("no sync files"));
            }
            Ok(FakeRing { value: 0 })
        }
        fn check_replacement(&self, current: &Images, next: &Images) -> Result<(), Error> {
            if current.depth == next.depth { Ok(()) } else { Err(Error::unsupported("a ring keeps its depth")) }
        }
        fn targets(_: &FakeGpu, _: &Images) -> Result<TargetFormats, Error> {
            Ok(TargetFormats {
                color: Some(TextureFormat::Rgba8Unorm),
                depth_stencil: Some(TextureFormat::Depth24PlusStencil8),
            })
        }
        fn import(gpu: &mut FakeGpu, images: &Images, slot: usize, depth: bool) -> Result<TextureId, GpuError> {
            let name = images.names.get(slot).ok_or(GpuError::UnknownId("slot"))? + u32::from(depth);
            Ok(gpu.textures.insert(Some(name)))
        }
        fn wait_for(&mut self, _: &mut FakeGpu, consumer: ConsumerSync) -> Result<Option<u32>, Error> {
            match consumer.sync.kind {
                SyncKind::QueueOrder => Ok(None),
                SyncKind::VulkanTimelineSemaphore => Ok(Some(consumer.sync.value as u32)),
                kind => Err(Error::unsupported(format!("no {kind:?}"))),
            }
        }
        fn wait_before_using(gpu: &mut FakeGpu, texture: TextureId, wait: &u32) -> Result<(), GpuError> {
            gpu.texture_waits.entry(texture).or_default().push(*wait);
            Ok(())
        }
        fn take_waits_into_next_submit(gpu: &mut FakeGpu, texture: TextureId) {
            gpu.next_waits.extend(gpu.texture_waits.remove(&texture).unwrap_or_default());
        }
        fn forget_waits(gpu: &mut FakeGpu, texture: TextureId) -> bool {
            gpu.texture_waits.remove(&texture).is_some()
        }
        fn waited(gpu: &mut FakeGpu, wait: u32) {
            gpu.waited.push(wait);
        }
        fn signal_after_next_submit(&mut self, gpu: &mut FakeGpu) {
            self.value += 1;
            gpu.signals = self.value;
        }
        fn producer_sync(&mut self, _: &mut FakeGpu) -> Option<(GpuSync, Option<u64>)> {
            let sync = GpuSync { kind: SyncKind::VulkanTimelineSemaphore, handle: 1, value: self.value };
            Some((sync, Some(self.value)))
        }
        fn tear_down(&mut self, _: &mut FakeGpu) {
            RECORD.with_borrow_mut(|record| record.torn_down += 1);
        }
        fn wait_for_frames(&self, _: &str) {
            RECORD.with_borrow_mut(|record| record.frames_waited += 1);
        }
        fn forget(wait: u32) {
            RECORD.with_borrow_mut(|record| record.forgotten.push(wait));
        }
    }

    fn timeline(value: u64) -> ConsumerSync {
        ConsumerSync::of(GpuSync { kind: SyncKind::VulkanTimelineSemaphore, handle: 1, value })
    }

    fn target(backend: &mut FakeBackend, names: &[u32]) -> BorrowedTarget<FakeRing> {
        BorrowedTarget::new(backend, images(names), SyncKind::VulkanTimelineSemaphore).expect("a ring")
    }

    #[test]
    fn a_frame_waits_for_the_hosts_reads_of_its_slot_once() {
        let mut backend = FakeBackend::default();
        let mut ring = target(&mut backend, &[10, 20]);
        let gpu = &mut backend.gpu;
        assert_eq!(gpu.imported(), [10, 20]);
        ring.release(gpu, 1, timeline(7)).expect("a release");
        ring.select(1);
        let image = ring.begin_frame(gpu).expect("a frame");
        // A frame begun again attaches its wait once.
        assert_eq!(ring.begin_frame(gpu), Some(image));
        assert_eq!(gpu.texture_waits[&image.color], [7]);
        assert!(image.depth_stencil.is_some_and(|depth| gpu.textures.get(depth) == Some(&None)));
        assert_eq!(
            ring.end_frame(gpu),
            Handoff::GivenBack {
                slot: 1,
                sync: GpuSync { kind: SyncKind::VulkanTimelineSemaphore, handle: 1, value: 1 }
            }
        );
        assert_eq!(gpu.submits, [vec![7]]);
        assert_eq!(gpu.signals, 1);
        // The submit took the wait: the next frame waits for nothing.
        ring.begin_frame(gpu);
        ring.end_frame(gpu);
        assert_eq!(gpu.submits.last(), Some(&vec![]));
        // A slot that the host gave up waits for nothing, and a slot the ring
        // lacks fails.
        ring.release(gpu, 0, timeline(8)).expect("a release");
        ring.discard(gpu, 0);
        assert!(ring.release(gpu, 2, timeline(9)).is_err());
        ring.tear_down(gpu);
        assert!(gpu.textures.is_empty());
        // A torn-down ring has no slots.
        assert!(ring.release(gpu, 0, timeline(9)).is_err());
    }

    #[test]
    fn a_ring_lets_go_of_each_wait_once() {
        let mut backend = FakeBackend::default();
        let mut ring = target(&mut backend, &[10, 20]);
        let gpu = &mut backend.gpu;
        // The submit that takes a frame's wait has it go after its work.
        ring.release(gpu, 0, timeline(7)).expect("a release");
        ring.begin_frame(gpu);
        ring.end_frame(gpu);
        assert_eq!(gpu.waited, [7]);
        // A release replaces the slot's wait, and one that fails leaves the
        // slot waiting for nothing.
        ring.release(gpu, 1, timeline(8)).expect("a release");
        ring.release(gpu, 1, timeline(9)).expect("a release");
        let fence = ConsumerSync::of(GpuSync { kind: SyncKind::D3d12Fence, handle: 1, value: 10 });
        assert!(ring.release(gpu, 1, fence).is_err());
        assert_eq!(forgotten(), [8, 9]);
        ring.select(1);
        let image = ring.begin_frame(gpu).expect("a frame");
        assert!(!gpu.texture_waits.contains_key(&image.color));
        ring.end_frame(gpu);
        assert_eq!(gpu.submits.last(), Some(&vec![]));
        // A tear-down lets go of the waits that no frame took at once.
        ring.release(gpu, 0, timeline(100)).expect("a release");
        ring.release(gpu, 1, timeline(11)).expect("a release");
        ring.tear_down(gpu);
        assert_eq!(forgotten(), [8, 9, 100, 11]);
        assert_eq!(gpu.waited, [7]);
    }

    fn sync(value: u64) -> GpuSync {
        GpuSync { kind: SyncKind::VulkanTimelineSemaphore, handle: 1, value }
    }

    // The host's reads don't hold up a release: its sync comes after the
    // ring's own work, which waits for them on the GPU.
    #[test]
    fn a_release_hands_over_the_sync_after_its_work_on_the_textures() {
        let mut backend = FakeBackend::default();
        let mut ring = target(&mut backend, &[10, 20]);
        let gpu = &mut backend.gpu;
        ring.release(gpu, 0, timeline(7)).expect("a release");
        ring.release(gpu, 1, timeline(8)).expect("a release");
        let image = ring.begin_frame(gpu).expect("a frame");
        // A pass of the frame took the wait into the work recorded so far.
        FakeRing::take_waits_into_next_submit(gpu, image.color);
        assert_eq!(ring.release_textures(gpu).expect("a release of the textures"), sync(1));
        assert_eq!((gpu.submits.clone(), gpu.signals), (vec![vec![7]], 1));
        assert_eq!((gpu.waited.clone(), forgotten()), (vec![7], vec![8]));
        // It keeps what the sync handed over, and draws no frame.
        assert_eq!(ring.handed, Some(1));
        assert!(gpu.textures.is_empty());
        assert_eq!(ring.begin_frame(gpu), None);
        assert_eq!(ring.end_frame(gpu), Handoff::Dropped);
        assert!(ring.release_textures(gpu).is_err());
        assert!(ring.replace(gpu, images(&[10, 20])).is_err());
        ring.tear_down(gpu);
        assert_eq!((frames_waited(), torn_down()), (0, 1));
    }

    // A submit that fails, as on a lost device, gives no sync to wait for:
    // the release or replacement fails, and the ring keeps its textures.
    #[test]
    fn a_release_or_replacement_whose_submit_fails_keeps_the_textures() {
        let mut backend = FakeBackend::default();
        let mut ring = target(&mut backend, &[10, 20]);
        let gpu = &mut backend.gpu;
        gpu.lost = true;
        assert!(ring.replace(gpu, images(&[20, 30])).is_err());
        assert!(ring.release_textures(gpu).is_err());
        assert_eq!(gpu.imported(), [10, 20]);
        gpu.lost = false;
        assert_eq!(
            ring.replace(gpu, images(&[20, 30])).expect("a replacement").kind,
            SyncKind::VulkanTimelineSemaphore
        );
        assert_eq!(gpu.imported(), [20, 30]);
    }

    // A tear-down or a replacement never blocks: the ring's objects go once
    // the frames are done. Only a target dropped without its tear-down, and
    // so without a `Gpu` to defer to, waits for its frames.
    #[test]
    fn a_ring_waits_for_its_frames_only_when_dropped_without_a_tear_down() {
        let mut backend = FakeBackend::default();
        let mut ring = target(&mut backend, &[10, 20]);
        ring.release(&mut backend.gpu, 0, timeline(7)).expect("a release");
        ring.replace(&mut backend.gpu, images(&[20, 30])).expect("a replacement");
        ring.tear_down(&mut backend.gpu);
        assert_eq!((frames_waited(), torn_down()), (0, 1));
        assert_eq!(forgotten(), [7]);
        drop(ring);
        assert_eq!(frames_waited(), 0);
        // Dropped without a tear-down, it waits as it drops.
        let mut ring = target(&mut backend, &[10]);
        ring.release(&mut backend.gpu, 0, timeline(8)).expect("a release");
        drop(ring);
        assert_eq!((frames_waited(), torn_down()), (1, 1));
        assert_eq!(forgotten(), [7, 8]);
    }

    #[test]
    fn a_frame_left_unended_lets_go_of_a_wait_that_no_pass_took_at_once() {
        let mut backend = FakeBackend::default();
        let mut ring = target(&mut backend, &[10]);
        let gpu = &mut backend.gpu;
        ring.release(gpu, 0, timeline(7)).expect("a release");
        ring.begin_frame(gpu);
        // A tear-down submits the frame, which drew nothing and so waits for
        // no host, and lets go of the wait at once.
        ring.tear_down(gpu);
        assert_eq!(gpu.submits, [Vec::<u32>::new()]);
        assert_eq!((forgotten(), gpu.waited.clone()), (vec![7], vec![]));
        drop(ring);
        // Without a gpu, a drop leaves the wait to the backend.
        let mut ring = target(&mut backend, &[10]);
        ring.release(&mut backend.gpu, 0, timeline(8)).expect("a release");
        ring.begin_frame(&mut backend.gpu);
        drop(ring);
        assert_eq!(forgotten(), [7]);
    }

    #[test]
    fn a_replaced_ring_keeps_the_waits_of_the_images_it_has_again() {
        let mut backend = FakeBackend::default();
        let mut ring = target(&mut backend, &[10, 20]);
        let gpu = &mut backend.gpu;
        ring.release(gpu, 0, timeline(7)).expect("a release");
        ring.release(gpu, 1, timeline(8)).expect("a release");
        // Of another depth, or with depth textures, the ring keeps its own.
        assert!(ring.replace(gpu, images(&[30])).is_err());
        assert!(ring.replace(gpu, Images { depth: true, ..images(&[20, 30]) }).is_err());
        assert_eq!(ring.replace(gpu, images(&[20, 30])).expect("a replacement"), sync(1));
        assert_eq!(gpu.imported(), [20, 30]);
        // The ring lets go of image 10's read at once.
        assert_eq!(forgotten(), [7]);
        // Image 20 keeps its read, at slot 0.
        let image = ring.begin_frame(gpu).expect("a frame");
        assert_eq!(gpu.texture_waits[&image.color], [8]);
        ring.end_frame(gpu);
        assert_eq!(gpu.submits.last(), Some(&vec![8]));
    }

    #[test]
    fn a_ring_that_lets_go_of_a_begun_frame_submits_only_the_waits_that_it_drew_with() {
        let mut backend = FakeBackend::default();
        let mut ring = target(&mut backend, &[10, 20]);
        let gpu = &mut backend.gpu;
        ring.release(gpu, 0, timeline(7)).expect("a release");
        let image = ring.begin_frame(gpu).expect("a frame");
        // A pass of the frame took the wait into the work recorded so far.
        FakeRing::take_waits_into_next_submit(gpu, image.color);
        ring.replace(gpu, images(&[20, 30])).expect("a replacement");
        assert_eq!(gpu.submits, [vec![7]]);
        assert_eq!(gpu.waited, [7]);
        // A frame begun on a texture that the new ring has again, which
        // drew nothing, leaves the wait to the texture's next frame: the
        // replacement's submit waits for no host.
        ring.release(gpu, 0, timeline(8)).expect("a release");
        ring.begin_frame(gpu);
        ring.replace(gpu, images(&[20, 40])).expect("a replacement");
        assert_eq!(gpu.submits, [vec![7], vec![]]);
        let image = ring.begin_frame(gpu).expect("a frame");
        assert_eq!(gpu.texture_waits[&image.color], [8]);
        ring.end_frame(gpu);
        assert_eq!(gpu.submits.last(), Some(&vec![8]));
        assert_eq!(gpu.waited, [7, 8]);
        // A frame that drew nothing waits for no host as the ring goes, and
        // lets go of its wait at once.
        ring.release(gpu, 1, timeline(9)).expect("a release");
        ring.select(1);
        ring.begin_frame(gpu);
        ring.tear_down(gpu);
        assert_eq!(gpu.submits.last(), Some(&vec![]));
        assert_eq!(gpu.waited, [7, 8]);
        assert!(gpu.texture_waits.is_empty());
        assert_eq!(forgotten(), [9]);
    }

    #[test]
    fn a_ring_imports_the_hosts_depth_or_makes_its_own() {
        let mut backend = FakeBackend::default();
        let ring = BorrowedTarget::<FakeRing>::new(
            &mut backend,
            Images { depth: true, ..images(&[10, 20]) },
            SyncKind::QueueOrder,
        )
        .expect("a ring");
        assert_eq!(backend.gpu.imported(), [10, 11, 20, 21]);
        drop(ring);
        // An own depth texture that fails leaves nothing imported.
        let empty = Images { size: Size::new(0, 4), ..images(&[10]) };
        assert!(Imports::new::<FakeRing>(&mut backend.gpu, &empty).is_err());
        assert_eq!(backend.gpu.imported(), [10, 11, 20, 21]);
        assert!(BorrowedTarget::<FakeRing>::new(&mut backend, images(&[10]), SyncKind::SyncFd).is_err());
    }

    /// Images by name, of the backend's counter.
    struct FakeExporter {
        depth: usize,
    }

    impl Exporter for FakeExporter {
        type Ring = FakeRing;
        type Images = ();

        fn new(_: &mut FakeBackend, ring: &ExportedRing, _: ExportKind, _: SyncKind) -> Result<Self, Error> {
            Ok(FakeExporter { depth: ring.depth })
        }
        fn allocate(&self, backend: &mut FakeBackend, size: Size) -> Result<((), Images), Error> {
            let names = (0..self.depth)
                .map(|_| {
                    backend.next_image += 10;
                    backend.next_image
                })
                .collect();
            Ok(((), Images { names, size, depth: false }))
        }
        fn frame(&self, (): &(), images: &Images, slot: usize) -> Option<ExportedFrame> {
            let fd = *images.names.get(slot)? as i32;
            Some(ExportedFrame {
                size: images.size,
                image: ExportedImage::OpaqueFd { fd, allocation_size: 0, dedicated: true, format: 0 },
                texture: NativeTexture::WebGpu,
                depth: None,
            })
        }
    }

    fn fd(frame: Option<ExportedFrame>) -> Option<i32> {
        match frame?.image {
            ExportedImage::OpaqueFd { fd, .. } => Some(fd),
            _ => None,
        }
    }

    #[test]
    fn a_held_frame_keeps_its_generation_until_it_comes_back() {
        let mut backend = FakeBackend::default();
        let ring = ExportedRing {
            depth: 2,
            kinds: crate::export::ExportKinds::NONE,
            dma_buf_formats: Vec::new(),
            android_hardware_buffer_usage: 0,
            keep_depth: false,
        };
        let size = Size::new(4, 4);
        let mut target = ExportedTarget::<FakeExporter>::new(
            &mut backend,
            &ring,
            ExportKind::OpaqueFd,
            size,
            SyncKind::VulkanTimelineSemaphore,
        )
        .expect("a ring");
        target.select(1);
        target.begin_frame(&mut backend.gpu);
        assert!(matches!(target.end_frame(&mut backend.gpu), Handoff::Queued { slot: 1, .. }));
        assert!(!target.resize(&mut backend, size, &[1]).expect("no resize"));
        assert!(target.resize(&mut backend, Size::new(8, 8), &[1]).expect("a resize"));
        assert_eq!(target.size(), Size::new(8, 8));
        // The host holds slot 1 of the first generation.
        assert_eq!((fd(target.image(0)), fd(target.image(1))), (Some(30), Some(20)));
        assert_eq!(backend.gpu.imported(), [10, 20, 30, 40]);
        target.release(&mut backend.gpu, 1, timeline(7)).expect("a release");
        assert_eq!(backend.gpu.imported(), [30, 40]);
        assert_eq!(fd(target.image(1)), Some(40));
        // A frame of an older generation that the host gives up lets go of
        // its generation too.
        target.select(0);
        target.begin_frame(&mut backend.gpu);
        assert!(matches!(target.end_frame(&mut backend.gpu), Handoff::Queued { slot: 0, .. }));
        assert!(target.resize(&mut backend, size, &[0]).expect("a resize"));
        assert_eq!(backend.gpu.imported(), [30, 40, 50, 60]);
        target.discard(&mut backend.gpu, 0);
        assert_eq!(backend.gpu.imported(), [50, 60]);
        target.tear_down(&mut backend.gpu);
        assert!(backend.gpu.textures.is_empty());
    }
}
