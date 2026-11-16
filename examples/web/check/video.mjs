// A video source in the page: the video plays on the page's thread, and its
// frames reach the map's Web Worker as ImageBitmaps, which the map's backend
// copies on the GPU. quarters.webm is 64 × 64 pixels, two seconds at 10
// frames a second, VP8: a red, a green, a blue and a yellow quarter, from
// the top left clockwise but for the yellow bottom right, and 16 × 16 pixels
// of noise in the middle, new each frame. GStreamer made it:
//
//   gst-launch-1.0 -e compositor name=c background=black sink_1::xpos=32 \
//     sink_2::ypos=32 sink_3::xpos=32 sink_3::ypos=32 sink_4::xpos=24 \
//     sink_4::ypos=24 ! video/x-raw,width=64,height=64,framerate=10/1 \
//     ! videoconvert ! vp8enc deadline=1 target-bitrate=400000 ! webmmux \
//     ! filesink location=quarters.webm \
//     videotestsrc num-buffers=20 pattern=solid-color \
//       foreground-color=0xffff0000 ! video/x-raw,width=32,height=32 ! c.sink_0 \
//     ... 0xff00ff00 ... ! c.sink_1  ... 0xff0000ff ... ! c.sink_2 \
//     ... 0xffffff00 ... ! c.sink_3 \
//     videotestsrc num-buffers=20 pattern=snow \
//       ! video/x-raw,width=16,height=16 ! c.sink_4

import { pageUrl } from "./page.mjs";

/** The size of the map, in CSS and device pixels. */
const SIZE = 256;
/** The video's quad: 20° on a side around (0, 0), about 160 pixels at this zoom. */
const ZOOM = 3.5;
/** Where each quarter's color is read: this far from the middle, in pixels. */
const OFFSET = 45;
const QUARTERS = [
  ["top left", -1, -1, [255, 0, 0]],
  ["top right", 1, -1, [0, 255, 0]],
  ["bottom left", -1, 1, [0, 0, 255]],
  ["bottom right", 1, 1, [255, 255, 0]],
];

/** The style of the check, with its video on the server at `origin`. */
export function videoStyle(origin) {
  return {
    version: 8,
    sources: {
      video: {
        type: "video",
        urls: [`${origin}/video/quarters.webm`],
        coordinates: [
          [-10, 10],
          [10, 10],
          [10, -10],
          [-10, -10],
        ],
      },
    },
    layers: [
      {
        id: "background",
        type: "background",
        paint: { "background-color": "black" },
      },
      {
        id: "video",
        type: "raster",
        source: "video",
        paint: { "raster-fade-duration": 0 },
      },
    ],
  };
}

/**
 * Loads the page with the check's video source; returns how the frame
 * failed to keep each quarter's color and place, or the video to move on,
 * if it did.
 */
export async function checkVideo(context) {
  const view = {
    style: { center: [0, 0], zoom: ZOOM, bearing: 0, pitch: 0 },
    width: SIZE,
    height: SIZE,
    ratio: 1,
  };
  const url = new URL(pageUrl(context, view));
  url.searchParams.set("style", `${context.server.origin}/video/style`);
  const session = await context.browser.open(url.href);
  try {
    const frames = await session.evaluate(
      `(async () => {
        const deadline = performance.now() + 60000;
        while (!window.mln) {
          if (performance.now() > deadline) throw new Error('the page did not start');
          await new Promise((resolve) => setTimeout(resolve, 50));
        }
        const within = (promise, what) => Promise.race([
          promise,
          new Promise((_, reject) => setTimeout(() => reject(new Error(what + ' after 60 s')), 60000)),
        ]);
        await within(window.mln.loaded, 'not loaded');
        const frame = async () => Array.from((await within(window.mln.readPixels(), 'no frame')).pixels);
        await new Promise((resolve) => setTimeout(resolve, 1000));
        const first = await frame();
        await new Promise((resolve) => setTimeout(resolve, 700));
        return [first, await frame()];
      })()`,
    );
    const [first, later] = frames;
    const at = (pixels, x, y) => {
      const i = (y * SIZE + x) * 4;
      return [pixels[i], pixels[i + 1], pixels[i + 2]];
    };
    const middle = SIZE / 2;
    for (const [name, dx, dy, expected] of QUARTERS) {
      const color = at(first, middle + dx * OFFSET, middle + dy * OFFSET);
      if (color.some((value, i) => Math.abs(value - expected[i]) > 48))
        return `the video's ${name} quarter is rgb(${color}), not rgb(${expected})`;
    }
    // The noise in the middle changes with each frame of the video.
    let changed = 0;
    for (let y = middle - 10; y < middle + 10; y++) {
      for (let x = middle - 10; x < middle + 10; x++) {
        const [a, b] = [at(first, x, y), at(later, x, y)];
        if (a.some((value, i) => Math.abs(value - b[i]) > 32)) changed++;
      }
    }
    if (changed < 40)
      return `the video didn't move on: ${changed} of its middle's pixels changed`;
    return null;
  } catch (error) {
    return error.message.split("\n").slice(0, 2).join(" ");
  } finally {
    await session.close();
  }
}
