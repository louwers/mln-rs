// The videos of the map's video sources, on the page's thread, which has the
// `<video>` elements that the map's Web Worker lacks. `pageVideoHost` is the
// VideoHost (mln_video's WebPlayers, in pkg/mln_web.d.ts) of a page: a page
// that runs the map on its own thread gives it to the map as it is. This
// page's map runs in a Web Worker, whose VideoHost asks this thread over the
// worker's messages (map-worker.ts), and `serveVideos` answers them with
// this one, sending each frame back as an ImageBitmap, transferred.
//
// Each video is a `<video>` outside the document, muted, looping and playing
// inline, with a `<source>` per URL in order, as GL JS plays video sources:
// the browser loads the first URL it can play itself, so the map's request
// headers, resource provider and cache don't apply to it. A new frame is
// read when the video shows it (requestVideoFrameCallback, else polled
// each animation frame), and after a seek, even while paused. Unlike GL
// JS, a video that doesn't play reports why (`onError`).

import type { VideoHandle, VideoHost } from "./pkg/mln_web.js";
import type { ToMap, VideoRequest } from "./protocol.ts";

export const pageVideoHost: VideoHost = {
  open: (urls, onFrame, onError) => new PageVideo(urls, onFrame, onError),
};

/**
 * Plays the videos that the map's Web Worker opens, sending their frames
 * and failures to it with `post`; returns what takes its requests. Every
 * video closes with `closeAll`, as when the map stops.
 */
export function serveVideos(
  post: (message: ToMap, transfer?: Transferable[]) => void,
): { handle: (request: VideoRequest) => void; closeAll: () => void } {
  const videos = new Map<number, VideoHandle>();
  const handle = (request: VideoRequest) => {
    const { id } = request;
    if (request.type === "videoOpen") {
      const video = pageVideoHost.open(
        request.urls,
        (frame, width, height, time) =>
          post({ type: "videoFrame", id, frame, width, height, time }, [frame]),
        (message) => post({ type: "videoError", id, message }),
      );
      videos.set(id, video);
    } else if (request.type === "videoSeek") {
      videos.get(id)?.seek(request.seconds);
    } else if (request.control === "close") {
      videos.get(id)?.close();
      videos.delete(id);
    } else {
      videos.get(id)?.[request.control]();
    }
  };
  const closeAll = () => {
    for (const video of videos.values()) video.close();
    videos.clear();
  };
  return { handle, closeAll };
}

// Where the browser has it, a video calls back as it shows each frame.
const frameCallbacks =
  "requestVideoFrameCallback" in HTMLVideoElement.prototype;

type OnFrame = Parameters<VideoHost["open"]>[1];
type OnError = Parameters<VideoHost["open"]>[2];

class PageVideo implements VideoHandle {
  readonly #video = document.createElement("video");
  readonly #onFrame: OnFrame;
  readonly #onError: OnError;
  #closed = false;
  #failed = false;
  /** A frame is being read, and whether another came meanwhile. */
  #reading = false;
  #again = false;
  /** The pending requestVideoFrameCallback or requestAnimationFrame. */
  #callback = 0;

  constructor(urls: string[], onFrame: OnFrame, onError: OnError) {
    this.#onFrame = onFrame;
    this.#onError = onError;
    const video = this.#video;
    video.muted = true;
    video.loop = true;
    video.playsInline = true;
    // The frames of another origin's video can't be read unless its server
    // allows it (CORS), which the request must ask for.
    if (urls.some((url) => new URL(url, location.href).origin !== origin))
      video.crossOrigin = "anonymous";
    for (const url of urls) {
      const source = document.createElement("source");
      source.src = url;
      video.append(source);
    }
    // The last <source> fails once each one before it has.
    video.lastElementChild?.addEventListener("error", () =>
      this.#fail(
        `none of the video's URLs plays in this browser (an unsupported format, an unreachable URL, or another origin's server without CORS): ${urls.join(", ")}`,
      ),
    );
    video.addEventListener("error", () =>
      this.#fail(`the video doesn't play: ${video.error?.message || "error"}`),
    );
    // The first frame, and the frame after a seek, even while paused.
    video.addEventListener("loadeddata", () => this.#read());
    video.addEventListener("seeked", () => this.#read());
    this.#watch();
    // Played rather than `autoplay`: Chromium pauses a muted video that
    // plays by itself while it isn't on screen, as this one never is.
    this.play();
  }

  play() {
    // A muted video may play without a user's gesture. Refused, it stays
    // paused, showing its first frame. The other rejections are of a pause
    // or a load that came first, or of a video that fails (`error`).
    this.#video.play().catch((error: unknown) => {
      if (error instanceof DOMException && error.name === "NotAllowedError")
        console.warn(
          `the browser doesn't play a video source's video: ${error.message}`,
        );
    });
  }

  pause() {
    this.#video.pause();
  }

  seek(seconds: number) {
    this.#video.currentTime = seconds;
  }

  close() {
    if (this.#closed) return;
    this.#closed = true;
    const video = this.#video;
    if (frameCallbacks) video.cancelVideoFrameCallback(this.#callback);
    else cancelAnimationFrame(this.#callback);
    video.pause();
    video.replaceChildren();
    video.load(); // lets go of the media
  }

  /** Reads each frame that the video shows. */
  #watch() {
    const video = this.#video;
    if (frameCallbacks) {
      const shown = () => {
        if (this.#closed) return;
        this.#read();
        this.#callback = video.requestVideoFrameCallback(shown);
      };
      this.#callback = video.requestVideoFrameCallback(shown);
      return;
    }
    let time = -1;
    const poll = () => {
      if (this.#closed) return;
      if (video.currentTime !== time) {
        time = video.currentTime;
        this.#read();
      }
      this.#callback = requestAnimationFrame(poll);
    };
    this.#callback = requestAnimationFrame(poll);
  }

  /** Sends the frame the video shows now, once the frame before was read. */
  #read() {
    const video = this.#video;
    if (
      this.#closed ||
      this.#failed ||
      video.readyState < HTMLMediaElement.HAVE_CURRENT_DATA
    )
      return;
    if (this.#reading) {
      this.#again = true;
      return;
    }
    this.#reading = true;
    const time = video.currentTime;
    createImageBitmap(video).then(
      (frame) => {
        this.#reading = false;
        if (this.#closed) {
          frame.close();
          return;
        }
        this.#onFrame(frame, frame.width, frame.height, time);
        if (this.#again) {
          this.#again = false;
          this.#read();
        }
      },
      (error: unknown) => {
        this.#reading = false;
        this.#fail(`the page can't read the video's frames: ${String(error)}`);
      },
    );
  }

  #fail(message: string) {
    if (this.#closed || this.#failed) return;
    this.#failed = true;
    this.#onError(message);
  }
}
