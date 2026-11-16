//! Video sources on Windows, played by Media Foundation's media engine.
//!
//! Each player runs a thread that owns an `IMFMediaEngine` in frame-server
//! mode, on a D3D11 device of its own made for video. The engine reads the
//! video through an `IMFByteStream` over a [`ByteSource`], decodes it (in
//! hardware where the device can) and converts each frame to BGRA in a D3D11
//! texture. The player shares that texture with the renderer through an NT
//! handle ([`GpuFrame::DxgiSharedHandle`]), or copies it to memory once a
//! renderer refused GPU frames.

#![expect(unsafe_code, reason = "Media Foundation and D3D11 through the windows crate are the FFI here")]

use std::os::windows::io::{FromRawHandle as _, OwnedHandle};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use mln_gfx::video::{CpuPlane, FramePixels, GpuFrame, PlaneLayout, VideoFrame, YuvColor};
use mln_storage::FileSource;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;
use parking_lot::Mutex;
use windows::Win32::Foundation::{HMODULE, RECT, S_OK};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_FLAG, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
    D3D11_QUERY_DESC, D3D11_QUERY_EVENT, D3D11_RESOURCE_MISC_FLAG, D3D11_RESOURCE_MISC_SHARED,
    D3D11_RESOURCE_MISC_SHARED_NTHANDLE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE, D3D11_USAGE_DEFAULT,
    D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread, ID3D11Query,
    ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{DXGI_SHARED_RESOURCE_READ, DXGI_SHARED_RESOURCE_WRITE, IDXGIResource1};
use windows::Win32::Media::MediaFoundation::{
    CLSID_MFMediaEngineClassFactory, IMFAttributes, IMFByteStream, IMFDXGIDeviceManager, IMFMediaEngine,
    IMFMediaEngineClassFactory, IMFMediaEngineEx, IMFMediaEngineNotify, MF_MEDIA_ENGINE_CALLBACK,
    MF_MEDIA_ENGINE_DXGI_MANAGER, MF_MEDIA_ENGINE_EVENT_ERROR, MF_MEDIA_ENGINE_EVENT_FIRSTFRAMEREADY,
    MF_MEDIA_ENGINE_EVENT_SEEKED, MF_MEDIA_ENGINE_READY_HAVE_METADATA, MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT, MF_VERSION,
    MFARGB, MFCreateAttributes, MFCreateDXGIDeviceManager, MFSTARTUP_LITE, MFShutdown, MFStartup,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::core::{BOOL, BSTR, Interface as _};

use crate::bytes::ByteSource;
use crate::player::{OpenRequest, VideoPlayer, VideoPlayers};
use crate::slot::FrameSink;

/// How often a player's thread asks the engine whether a new frame is due,
/// and takes commands: more often than any video's frame rate.
const TICK: Duration = Duration::from_millis(4);

/// Media Foundation's players.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct MediaFoundationPlayers;

impl VideoPlayers for MediaFoundationPlayers {
    fn open(&self, request: OpenRequest) -> Result<Box<dyn VideoPlayer>, String> {
        let OpenRequest { urls, file_source, frames, .. } = request;
        let file_source = file_source.ok_or("the map has no storage to read the video through")?;
        if urls.is_empty() {
            return Err("the video source has no URLs".to_owned());
        }
        let (commands, receiver) = mpsc::channel();
        let reading = Arc::new(Mutex::new(Reading::default()));
        let thread_reading = Arc::clone(&reading);
        let thread = std::thread::Builder::new()
            .name("mln-video".to_owned())
            .spawn(move || run(&urls, &file_source, &frames, &receiver, &thread_reading))
            .map_err(|error| format!("the video's thread didn't start: {error}"))?;
        Ok(Box::new(MediaFoundationPlayer { commands, reading, thread: Some(thread) }))
    }
}

#[derive(Clone, Copy)]
enum Command {
    Play,
    Pause,
    Seek(f64),
}

/// The bytes that a player's engine reads, which dropping the player
/// cancels.
#[derive(Default)]
struct Reading {
    closed: bool,
    bytes: Option<Arc<ByteSource>>,
}

impl Reading {
    /// Makes `bytes` the ones the engine reads, cancelling the ones before,
    /// and `bytes` too once the player is closed.
    fn replace(&mut self, bytes: &Arc<ByteSource>) {
        if let Some(before) = self.bytes.replace(Arc::clone(bytes)) {
            before.cancel();
        }
        if self.closed {
            bytes.cancel();
        }
    }
}

struct MediaFoundationPlayer {
    commands: Sender<Command>,
    reading: Arc<Mutex<Reading>>,
    thread: Option<JoinHandle<()>>,
}

impl VideoPlayer for MediaFoundationPlayer {
    fn play(&mut self) {
        let _ = self.commands.send(Command::Play);
    }

    fn pause(&mut self) {
        let _ = self.commands.send(Command::Pause);
    }

    fn seek(&mut self, seconds: f64) {
        let _ = self.commands.send(Command::Seek(seconds));
    }
}

impl Drop for MediaFoundationPlayer {
    fn drop(&mut self) {
        // Reads that block the engine end first, so that its thread sees the
        // closed channel within a tick and shuts the engine down.
        {
            let mut reading = self.reading.lock();
            reading.closed = true;
            if let Some(bytes) = &reading.bytes {
                bytes.cancel();
            }
        }
        let (closed, _) = mpsc::channel();
        drop(std::mem::replace(&mut self.commands, closed));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A player's thread: starts COM and Media Foundation, plays until the
/// player drops, then stops them.
fn run(
    urls: &[String],
    file_source: &Arc<dyn FileSource>,
    frames: &FrameSink,
    commands: &Receiver<Command>,
    reading: &Mutex<Reading>,
) {
    // SAFETY: the thread joins the multithreaded apartment, which it leaves
    // at its end, after everything of COM it made has dropped.
    if let Err(error) = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok() {
        frames.fail(format!("COM didn't start for the video: {error}"));
        return;
    }
    // SAFETY: a plain call, which the thread balances with MFShutdown.
    match unsafe { MFStartup(MF_VERSION, MFSTARTUP_LITE) } {
        Ok(()) => {
            match Player::new(urls, file_source, frames, reading) {
                Ok(mut player) => {
                    player.play(commands);
                    player.shutdown();
                }
                Err(message) => frames.fail(message),
            }
            // SAFETY: as for MFStartup.
            let _ = unsafe { MFShutdown() };
        }
        Err(error) => frames.fail(format!("Media Foundation didn't start: {error}")),
    }
    // SAFETY: as for CoInitializeEx.
    unsafe { CoUninitialize() };
}

/// The media engine of a player, and what it sends frames with.
struct Player<'a> {
    urls: &'a [String],
    file_source: &'a Arc<dyn FileSource>,
    frames: &'a FrameSink,
    reading: &'a Mutex<Reading>,
    /// The index of the URL the engine plays.
    url: usize,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    engine: IMFMediaEngine,
    events: Receiver<EngineEvent>,
    /// What frames in memory go through, made for the size of the last: the
    /// texture the engine writes and the one the CPU reads.
    cpu_textures: Option<(Size, ID3D11Texture2D, ID3D11Texture2D)>,
    /// The query that tells when the device's work is done.
    query: ID3D11Query,
    /// Whether the engine has a frame to send even if the stream's clock
    /// says no new one is due: after a seek, or a refusal of GPU frames.
    due: bool,
    /// Whether a seek hasn't finished: until it does, the engine's frame is
    /// the one before it, though its time is the seek's.
    seeking: bool,
    /// Whether the last frame sent was on the GPU, and any frame was.
    sent_gpu: bool,
    sent_any: bool,
    failed: bool,
}

/// An event of the engine, with its parameters.
type EngineEvent = (u32, usize, u32);

impl<'a> Player<'a> {
    fn new(
        urls: &'a [String],
        file_source: &'a Arc<dyn FileSource>,
        frames: &'a FrameSink,
        reading: &'a Mutex<Reading>,
    ) -> Result<Player<'a>, String> {
        let failed = |what: &'static str| move |error: windows::core::Error| format!("{what}: {error}");
        let (mut device, mut context) = (None, None);
        // SAFETY: the call fills the device and its context.
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_VIDEO_SUPPORT | D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&raw mut device),
                None,
                Some(&raw mut context),
            )
        }
        .map_err(failed("D3D11 made no device for video"))?;
        let (Some(device), Some(context)) = (device, context) else {
            return Err("D3D11 made no device for video".to_owned());
        };
        let (events, receiver) = mpsc::channel();
        let notify: IMFMediaEngineNotify = Notify(Mutex::new(events)).into();
        let mut attributes: Option<IMFAttributes> = None;
        let mut token = 0;
        let mut manager: Option<IMFDXGIDeviceManager> = None;
        // SAFETY: plain calls of live objects, whose out-pointers outlive
        // them. The engine's threads share the device's immediate context
        // with this one, which the multithread protection serializes.
        let engine = unsafe {
            let _ =
                context.cast::<ID3D11Multithread>().map_err(failed("ID3D11Multithread"))?.SetMultithreadProtected(true);
            MFCreateDXGIDeviceManager(&raw mut token, &raw mut manager).map_err(failed("MFCreateDXGIDeviceManager"))?;
            let manager = manager.ok_or("Media Foundation made no DXGI device manager")?;
            manager.ResetDevice(&device, token).map_err(failed("IMFDXGIDeviceManager::ResetDevice"))?;
            MFCreateAttributes(&raw mut attributes, 3).map_err(failed("MFCreateAttributes"))?;
            let attributes = attributes.ok_or("Media Foundation made no attributes")?;
            attributes.SetUnknown(&MF_MEDIA_ENGINE_DXGI_MANAGER, &manager).map_err(failed("the DXGI manager"))?;
            attributes.SetUnknown(&MF_MEDIA_ENGINE_CALLBACK, &notify).map_err(failed("the engine's callback"))?;
            // An output format puts the engine in frame-server mode.
            attributes
                .SetUINT32(&MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM.0 as u32)
                .map_err(failed("the engine's output format"))?;
            let factory: IMFMediaEngineClassFactory =
                CoCreateInstance(&CLSID_MFMediaEngineClassFactory, None, CLSCTX_INPROC_SERVER)
                    .map_err(failed("Media Foundation has no media engine"))?;
            let engine = factory.CreateInstance(0, &attributes).map_err(failed("the media engine didn't start"))?;
            engine.SetMuted(true).map_err(failed("muting the video"))?;
            engine.SetLoop(true).map_err(failed("looping the video"))?;
            engine.SetAutoPlay(true).map_err(failed("playing the video"))?;
            engine
        };
        let mut query = None;
        let desc = D3D11_QUERY_DESC { Query: D3D11_QUERY_EVENT, MiscFlags: 0 };
        // SAFETY: the description outlives the call, which fills `query`.
        unsafe { device.CreateQuery(&raw const desc, Some(&raw mut query)) }.map_err(failed("a D3D11 query"))?;
        let query = query.ok_or("D3D11 made no query")?;
        let mut player = Player {
            urls,
            file_source,
            frames,
            reading,
            url: 0,
            device,
            context,
            engine,
            events: receiver,
            cpu_textures: None,
            query,
            due: false,
            seeking: false,
            sent_gpu: false,
            sent_any: false,
            failed: false,
        };
        player.open_from(0)?;
        Ok(player)
    }

    /// Has the engine play the first of `urls[first..]` that opens; fails
    /// with the last URL's error if none does.
    fn open_from(&mut self, first: usize) -> Result<(), String> {
        let mut result = Err("no URL left to play".to_owned());
        for index in first..self.urls.len() {
            result = self.open(index);
            if result.is_ok() {
                break;
            }
        }
        result
    }

    /// Has the engine play `urls[index]`.
    fn open(&mut self, index: usize) -> Result<(), String> {
        let url = &self.urls[index];
        self.url = index;
        let bytes = Arc::new(ByteSource::new(Arc::clone(self.file_source), url.clone()));
        self.reading.lock().replace(&bytes);
        let stream: IMFByteStream = stream::ByteStream::new(bytes).into();
        // SAFETY: plain calls of the live engine; the URL tells the engine
        // the container by its extension. The engine plays the source as
        // it opens, as it autoplays.
        unsafe {
            let engine: IMFMediaEngineEx = self.engine.cast().map_err(|error| format!("IMFMediaEngineEx: {error}"))?;
            engine
                .SetSourceFromByteStream(&stream, &BSTR::from(url.as_str()))
                .map_err(|error| format!("{url} can't be opened: {error}"))?;
        }
        Ok(())
    }

    /// Plays until the player drops.
    fn play(&mut self, commands: &Receiver<Command>) {
        loop {
            match commands.recv_timeout(TICK) {
                Ok(command) => self.command(command),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            while let Ok(event) = self.events.try_recv() {
                self.event(event);
            }
            if !self.failed
                && let Err(error) = self.tick()
            {
                Log::warning(Event::General, format!("A frame of {} was lost: {error}", self.urls[self.url]));
            }
        }
    }

    fn command(&mut self, command: Command) {
        // SAFETY: plain calls of the live engine.
        let done = unsafe {
            match command {
                Command::Play => self.engine.Play(),
                Command::Pause => self.engine.Pause(),
                Command::Seek(seconds) => {
                    // Before the video's metadata, a seek only sets where it
                    // starts, and no SEEKED event follows.
                    self.seeking = self.engine.GetReadyState() >= MF_MEDIA_ENGINE_READY_HAVE_METADATA.0 as u16;
                    self.engine.SetCurrentTime(seconds)
                }
            }
        };
        if let Err(error) = done {
            Log::warning(Event::General, format!("The video at {} didn't follow: {error}", self.urls[self.url]));
        }
    }

    fn event(&mut self, (event, code, status): EngineEvent) {
        match event as i32 {
            event if event == MF_MEDIA_ENGINE_EVENT_ERROR.0 => {
                // A URL that never played gives way to the next.
                if !self.sent_any && self.url + 1 < self.urls.len() {
                    if let Err(message) = self.open_from(self.url + 1) {
                        self.fail(message);
                    }
                    return;
                }
                let status = windows::core::HRESULT(status as i32);
                let reason = match code {
                    1 => "it was aborted",
                    2 => "its bytes didn't load",
                    3 => "it doesn't decode",
                    4 => "its format isn't supported",
                    5 => "it's encrypted",
                    _ => "the media engine failed",
                };
                self.fail(format!("{} doesn't play: {reason} ({})", self.urls[self.url], status.message()));
            }
            event if event == MF_MEDIA_ENGINE_EVENT_SEEKED.0 => {
                self.seeking = false;
                self.due = true;
            }
            event if event == MF_MEDIA_ENGINE_EVENT_FIRSTFRAMEREADY.0 => self.due = true,
            _ => {}
        }
    }

    fn fail(&mut self, message: String) {
        self.failed = true;
        self.frames.fail(message);
    }

    /// Sends the engine's frame, if a new one is due.
    fn tick(&mut self) -> Result<(), String> {
        let refused = self.frames.gpu_frames_refused();
        let mut pts = 0;
        // SAFETY: a plain call of the live engine, through its vtable: the
        // wrapper reads S_FALSE (no new frame) as success too.
        let status = unsafe {
            (windows::core::Interface::vtable(&self.engine).OnVideoStreamTick)(self.engine.as_raw(), &raw mut pts)
        };
        let new = status == S_OK;
        if self.seeking || !(new || self.due || (self.sent_gpu && refused)) {
            return Ok(());
        }
        let Some(size) = self.video_size() else {
            return Ok(());
        };
        self.due = false;
        // SAFETY: a plain query.
        let time = if new { pts as f64 / 1e7 } else { unsafe { self.engine.GetCurrentTime() } };
        let pixels = if refused { self.cpu_frame(size)? } else { self.gpu_frame(size)? };
        self.sent_gpu = !refused;
        self.sent_any = true;
        self.frames.push(VideoFrame { size, pixels, time: Some(time) });
        Ok(())
    }

    fn video_size(&self) -> Option<Size> {
        let (mut width, mut height) = (0, 0);
        // SAFETY: a plain query.
        unsafe { self.engine.GetNativeVideoSize(Some(&raw mut width), Some(&raw mut height)) }.ok()?;
        (width > 0 && height > 0).then(|| Size::new(width, height))
    }

    /// The frame in a texture of its own, shared through a new NT handle.
    /// The renderer may read a frame's texture until it drops the frame,
    /// which tells the player nothing, so each frame has a texture of its
    /// own rather than one of a ring that the player would write again.
    fn gpu_frame(&self, size: Size) -> Result<FramePixels, String> {
        let texture = self.texture(
            size,
            D3D11_USAGE_DEFAULT,
            D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE,
            D3D11_RESOURCE_MISC_SHARED | D3D11_RESOURCE_MISC_SHARED_NTHANDLE,
        )?;
        self.transfer(&texture, size)?;
        // The frame's contract: its texture is written before it's sent.
        self.finish()?;
        // SAFETY: a plain call of the live texture; the frame owns the
        // handle from here.
        let handle = unsafe {
            let resource: IDXGIResource1 = texture.cast().map_err(|error| format!("IDXGIResource1: {error}"))?;
            let handle = resource
                .CreateSharedHandle(None, (DXGI_SHARED_RESOURCE_READ | DXGI_SHARED_RESOURCE_WRITE).0, None)
                .map_err(|error| format!("sharing the frame's texture: {error}"))?;
            OwnedHandle::from_raw_handle(handle.0)
        };
        Ok(FramePixels::Gpu(GpuFrame::DxgiSharedHandle {
            handle,
            layout: PlaneLayout::Rgba,
            color: YuvColor::default(),
        }))
    }

    /// The frame's pixels in memory, as RGBA.
    fn cpu_frame(&mut self, size: Size) -> Result<FramePixels, String> {
        let (target, staging) = match &self.cpu_textures {
            Some((made, target, staging)) if *made == size => (target.clone(), staging.clone()),
            _ => {
                let target = self.texture(
                    size,
                    D3D11_USAGE_DEFAULT,
                    D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE,
                    D3D11_RESOURCE_MISC_FLAG(0),
                )?;
                let staging =
                    self.texture(size, D3D11_USAGE_STAGING, D3D11_BIND_FLAG(0), D3D11_RESOURCE_MISC_FLAG(0))?;
                self.cpu_textures = Some((size, target.clone(), staging.clone()));
                (target, staging)
            }
        };
        self.transfer(&target, size)?;
        let (width, height) = (size.width as usize, size.height as usize);
        let mut data = vec![0; width * height * 4];
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: plain calls of live textures; the mapping holds `height`
        // rows `RowPitch` bytes apart, of `width` BGRA pixels, until the
        // unmap.
        unsafe {
            self.context.CopyResource(&staging, &target);
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&raw mut mapped))
                .map_err(|error| format!("reading the frame: {error}"))?;
            for (row, out) in data.chunks_exact_mut(width * 4).enumerate() {
                let bgra = std::slice::from_raw_parts(
                    mapped.pData.cast::<u8>().add(row * mapped.RowPitch as usize),
                    width * 4,
                );
                for (pixel, bgra) in out.as_chunks_mut::<4>().0.iter_mut().zip(bgra.as_chunks::<4>().0) {
                    // Video is opaque, so its straight RGBA is premultiplied.
                    pixel.copy_from_slice(&[bgra[2], bgra[1], bgra[0], 255]);
                }
            }
            self.context.Unmap(&staging, 0);
        }
        Ok(FramePixels::Rgba(CpuPlane { data, stride: width * 4 }))
    }

    /// A BGRA texture of `size`.
    fn texture(
        &self,
        size: Size,
        usage: D3D11_USAGE,
        bind: D3D11_BIND_FLAG,
        misc: D3D11_RESOURCE_MISC_FLAG,
    ) -> Result<ID3D11Texture2D, String> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: size.width,
            Height: size.height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: usage,
            BindFlags: bind.0 as u32,
            CPUAccessFlags: if usage == D3D11_USAGE_STAGING { D3D11_CPU_ACCESS_READ.0 as u32 } else { 0 },
            MiscFlags: misc.0 as u32,
        };
        let mut texture = None;
        // SAFETY: the description outlives the call, which fills `texture`.
        unsafe { self.device.CreateTexture2D(&raw const desc, None, Some(&raw mut texture)) }
            .map_err(|error| format!("a texture for the frame: {error}"))?;
        texture.ok_or_else(|| "D3D11 made no texture for the frame".to_owned())
    }

    /// Has the engine write its frame into `texture`, of `size`.
    fn transfer(&self, texture: &ID3D11Texture2D, size: Size) -> Result<(), String> {
        let rect = RECT { left: 0, top: 0, right: size.width as i32, bottom: size.height as i32 };
        let border = MFARGB { rgbBlue: 0, rgbGreen: 0, rgbRed: 0, rgbAlpha: 255 };
        // SAFETY: a plain call of the live engine and texture.
        unsafe { self.engine.TransferVideoFrame(texture, None, &raw const rect, Some(&raw const border)) }
            .map_err(|error| format!("the engine didn't write the frame: {error}"))
    }

    /// Blocks until the device has done the work sent to it.
    fn finish(&self) -> Result<(), String> {
        // SAFETY: plain calls of the live context and query; `done` outlives
        // each read of the query, which writes it once the work is done.
        unsafe {
            self.context.End(&self.query);
            self.context.Flush();
            loop {
                let mut done = BOOL(0);
                self.context
                    .GetData(&self.query, Some((&raw mut done).cast()), size_of::<BOOL>() as u32, 0)
                    .map_err(|error| format!("waiting for the frame: {error}"))?;
                if done.as_bool() {
                    return Ok(());
                }
                std::thread::yield_now();
            }
        }
    }

    fn shutdown(&self) {
        // SAFETY: a plain call of the live engine, which no call uses after.
        let _ = unsafe { self.engine.Shutdown() };
    }
}

/// The COM objects that Media Foundation calls, in a module of their own for
/// the lints of the glue that `#[implement]` generates.
#[expect(trivial_casts, clippy::ref_as_ptr, clippy::inline_always, reason = "the code that #[implement] generates")]
mod notify {
    use std::sync::mpsc::Sender;

    use parking_lot::Mutex;
    use windows::Win32::Media::MediaFoundation::{IMFMediaEngineNotify, IMFMediaEngineNotify_Impl};
    use windows::core::{Result, implement};

    use super::EngineEvent;

    /// Sends the engine's events to the player's thread.
    #[implement(IMFMediaEngineNotify)]
    pub(super) struct Notify(pub(super) Mutex<Sender<EngineEvent>>);

    impl IMFMediaEngineNotify_Impl for Notify_Impl {
        fn EventNotify(&self, event: u32, param1: usize, param2: u32) -> Result<()> {
            let _ = self.0.lock().send((event, param1, param2));
            Ok(())
        }
    }
}

use notify::Notify;

/// The engine's byte stream over a [`ByteSource`].
#[expect(trivial_casts, clippy::ref_as_ptr, clippy::inline_always, reason = "the code that #[implement] generates")]
mod stream {
    use std::io;
    use std::sync::Arc;
    use std::sync::mpsc::{self, Sender};
    use std::thread::JoinHandle;

    use mln_util::hash::HashMap;
    use parking_lot::Mutex;
    use windows::Win32::Foundation::{E_ABORT, E_FAIL, E_INVALIDARG, E_NOTIMPL};
    use windows::Win32::Media::MediaFoundation::{
        IMFAsyncCallback, IMFAsyncResult, IMFByteStream, IMFByteStream_Impl, MFBYTESTREAM_IS_READABLE,
        MFBYTESTREAM_IS_SEEKABLE, MFBYTESTREAM_SEEK_ORIGIN, MFCreateAsyncResult, MFInvokeCallback, msoBegin,
        msoCurrent,
    };
    use windows::core::{BOOL, Error, IUnknown, Interface as _, Ref, Result, implement};

    use crate::bytes::ByteSource;

    /// What the stream's reads move.
    #[derive(Default)]
    struct Cursor {
        position: u64,
        /// Whether a read found the end of a resource of unknown size.
        ended: bool,
    }

    /// A read that the stream's thread does: `len` bytes at `offset` into
    /// `buffer`, then it invokes `result`'s callback.
    struct Job {
        buffer: *mut u8,
        len: usize,
        offset: u64,
        result: IMFAsyncResult,
    }

    // SAFETY: Media Foundation's async results may be used from any thread,
    // and the buffer is the caller's to keep until EndRead.
    unsafe impl Send for Job {}

    /// Reads a [`ByteSource`]: synchronously on the engine's threads, or on a
    /// thread of the stream's own for its async reads, which block until the
    /// bytes arrive or the source is cancelled.
    #[implement(IMFByteStream)]
    pub(super) struct ByteStream {
        bytes: Arc<ByteSource>,
        cursor: Arc<Mutex<Cursor>>,
        /// The counts of the async reads done, by their results' pointers.
        done: Arc<Mutex<HashMap<usize, u32>>>,
        jobs: Mutex<Option<Sender<Job>>>,
        thread: Mutex<Option<JoinHandle<()>>>,
    }

    fn failed(error: &io::Error) -> Error {
        let code = if error.kind() == io::ErrorKind::Interrupted { E_ABORT } else { E_FAIL };
        Error::new(code, error.to_string())
    }

    impl ByteStream {
        pub(super) fn new(bytes: Arc<ByteSource>) -> ByteStream {
            ByteStream {
                bytes,
                cursor: Arc::default(),
                done: Arc::default(),
                jobs: Mutex::new(None),
                thread: Mutex::new(None),
            }
        }

        /// The thread's queue of jobs, which starts it with the first.
        fn jobs(&self) -> Result<Sender<Job>> {
            let mut jobs = self.jobs.lock();
            if let Some(sender) = &*jobs {
                return Ok(sender.clone());
            }
            let (sender, receiver) = mpsc::channel::<Job>();
            let (bytes, cursor, done) = (Arc::clone(&self.bytes), Arc::clone(&self.cursor), Arc::clone(&self.done));
            let thread = std::thread::Builder::new()
                .name("mln-video-read".to_owned())
                .spawn(move || {
                    for job in receiver {
                        // SAFETY: the caller keeps the buffer until EndRead,
                        // which comes after the callback below.
                        let buffer = unsafe { std::slice::from_raw_parts_mut(job.buffer, job.len) };
                        match bytes.read_full(job.offset, buffer) {
                            Ok(count) => {
                                finish_read(&cursor, job.offset, job.len, count);
                                done.lock().insert(job.result.as_raw() as usize, count as u32);
                            }
                            Err(error) => {
                                // SAFETY: a plain call of the live result.
                                let _ = unsafe { job.result.SetStatus(failed(&error).code()) };
                            }
                        }
                        // SAFETY: as above.
                        let _ = unsafe { MFInvokeCallback(&job.result) };
                    }
                })
                .map_err(|error| Error::new(E_FAIL, error.to_string()))?;
            *self.thread.lock() = Some(thread);
            *jobs = Some(sender.clone());
            Ok(sender)
        }
    }

    /// Moves the cursor past `count` bytes read of `len` asked for at
    /// `offset`, unless something moved it since.
    fn finish_read(cursor: &Mutex<Cursor>, offset: u64, len: usize, count: usize) {
        let mut cursor = cursor.lock();
        if count < len && cursor.position == offset + len as u64 {
            cursor.position = offset + count as u64;
        }
        if count == 0 && len > 0 {
            cursor.ended = true;
        }
    }

    impl Drop for ByteStream {
        fn drop(&mut self) {
            drop(self.jobs.lock().take());
            if let Some(thread) = self.thread.lock().take()
                && thread.thread().id() != std::thread::current().id()
            {
                let _ = thread.join();
            }
        }
    }

    impl IMFByteStream_Impl for ByteStream_Impl {
        fn GetCapabilities(&self) -> Result<u32> {
            Ok(MFBYTESTREAM_IS_READABLE | MFBYTESTREAM_IS_SEEKABLE)
        }

        fn GetLength(&self) -> Result<u64> {
            // Media Foundation reads all ones as an unknown length.
            Ok(self.bytes.size().map_err(|error| failed(&error))?.unwrap_or(u64::MAX))
        }

        fn SetLength(&self, _length: u64) -> Result<()> {
            Err(E_NOTIMPL.into())
        }

        fn GetCurrentPosition(&self) -> Result<u64> {
            Ok(self.cursor.lock().position)
        }

        fn SetCurrentPosition(&self, position: u64) -> Result<()> {
            *self.cursor.lock() = Cursor { position, ended: false };
            Ok(())
        }

        fn IsEndOfStream(&self) -> Result<BOOL> {
            let cursor = self.cursor.lock();
            let ended = match self.bytes.size() {
                Ok(Some(size)) => cursor.position >= size,
                _ => cursor.ended,
            };
            Ok(ended.into())
        }

        fn Read(&self, buffer: *mut u8, len: u32, read: *mut u32) -> Result<()> {
            if buffer.is_null() || read.is_null() {
                return Err(E_INVALIDARG.into());
            }
            let offset = {
                let mut cursor = self.cursor.lock();
                let offset = cursor.position;
                cursor.position += u64::from(len);
                offset
            };
            // SAFETY: the caller passes a buffer of `len` bytes.
            let slice = unsafe { std::slice::from_raw_parts_mut(buffer, len as usize) };
            let count = self.bytes.read_full(offset, slice).map_err(|error| failed(&error))?;
            finish_read(&self.cursor, offset, len as usize, count);
            // SAFETY: the caller passes a place for the count.
            unsafe { *read = count as u32 };
            Ok(())
        }

        fn BeginRead(
            &self,
            buffer: *mut u8,
            len: u32,
            callback: Ref<IMFAsyncCallback>,
            state: Ref<IUnknown>,
        ) -> Result<()> {
            if buffer.is_null() {
                return Err(E_INVALIDARG.into());
            }
            // SAFETY: a plain call; the result holds the callback and state.
            let result = unsafe { MFCreateAsyncResult(None, callback.as_ref(), state.as_ref()) }?;
            // The cursor moves past the read at once, so that reads that
            // follow before it's done read on; a short read moves it back.
            let offset = {
                let mut cursor = self.cursor.lock();
                let offset = cursor.position;
                cursor.position += u64::from(len);
                offset
            };
            self.jobs()?
                .send(Job { buffer, len: len as usize, offset, result })
                .map_err(|_| Error::new(E_FAIL, "the video's reads ended"))
        }

        fn EndRead(&self, result: Ref<IMFAsyncResult>) -> Result<u32> {
            let result = result.ok()?;
            // SAFETY: a plain query of the live result.
            unsafe { result.GetStatus() }?;
            Ok(self.done.lock().remove(&(result.as_raw() as usize)).unwrap_or(0))
        }

        fn Write(&self, _buffer: *const u8, _len: u32) -> Result<u32> {
            Err(E_NOTIMPL.into())
        }

        fn BeginWrite(
            &self,
            _buffer: *const u8,
            _len: u32,
            _callback: Ref<IMFAsyncCallback>,
            _state: Ref<IUnknown>,
        ) -> Result<()> {
            Err(E_NOTIMPL.into())
        }

        fn EndWrite(&self, _result: Ref<IMFAsyncResult>) -> Result<u32> {
            Err(E_NOTIMPL.into())
        }

        fn Seek(&self, origin: MFBYTESTREAM_SEEK_ORIGIN, offset: i64, _flags: u32) -> Result<u64> {
            let mut cursor = self.cursor.lock();
            let base = if origin == msoBegin {
                0
            } else if origin == msoCurrent {
                cursor.position
            } else {
                return Err(E_INVALIDARG.into());
            };
            let position = base.checked_add_signed(offset).ok_or_else(|| Error::from(E_INVALIDARG))?;
            *cursor = Cursor { position, ended: false };
            Ok(position)
        }

        fn Flush(&self) -> Result<()> {
            Ok(())
        }

        fn Close(&self) -> Result<()> {
            Ok(())
        }
    }
}
