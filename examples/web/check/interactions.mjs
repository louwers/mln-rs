// The interaction views' input: drags on the map through the page's input
// path, as the browser delivers a user's.

// How long an interaction drags back and forth, and how often it moves.
const INTERACTION_MS = 2000;
const FRAME_MS = 1000 / 60;

/** From 0 at `ms`: to 1, then between -1 and 1, each swing half a second, back to 0 after each second. */
function swing(ms) {
  const phase = (((ms / 1000 - 0.25) % 1) + 1) % 1;
  return 4 * Math.abs(phase - 0.5) - 1;
}

/**
 * Drags on the canvas of the page in `session` back and forth for
 * INTERACTION_MS through the page's input, as the browser delivers a
 * user's (Playwright's mouse, and the DevTools protocol's touch events),
 * one move a frame, and releases where it started:
 * - `rotate`: a right-drag from the center, which turns the map between
 *   -90° and 90° (mln_gestures' 0.5° a pixel);
 * - `pan-zoom`: a two-finger pinch that pans the map 0.65 of a tile east
 *   and west of where it started while it zooms out by 0.9 at each end.
 * It holds still before the release, longer than the recognizer's 40 ms,
 * so that the release starts no momentum.
 */
export async function interact(session, interaction) {
  const { x, y } = await session.evaluate(
    `(() => {
      const rect = document.getElementById('map').getBoundingClientRect();
      return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
    })()`,
  );
  const { page } = session;
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  let event, release, cdp;
  if (interaction === "rotate") {
    await page.mouse.move(x, y);
    await page.mouse.down({ button: "right" });
    event = (ms) => page.mouse.move(x + 180 * swing(ms), y);
    release = () => page.mouse.up({ button: "right" });
  } else {
    // Playwright's touchscreen only taps: the pinch is the DevTools
    // protocol's.
    cdp = await page.context().newCDPSession(page);
    const send = (method, params) => cdp.send(method, params);
    await send("Emulation.setTouchEmulationEnabled", {
      enabled: true,
      maxTouchPoints: 2,
    });
    // The fingers 200 pixels apart where they start, closer by up to 2^-0.9.
    // The map's center follows the fingers' midpoint, at the zoom's scale:
    // 0.65 of a tile a swing, 333 pixels at the zoom of its ends.
    const fingers = (ms) => {
      const out = Math.abs(swing(ms));
      const at = x + 333 * swing(ms) * 2 ** (0.9 * (1 - out));
      const apart = 100 * 2 ** (-0.9 * out);
      return [
        { x: at - apart, y, id: 0 },
        { x: at + apart, y, id: 1 },
      ];
    };
    await send("Input.dispatchTouchEvent", {
      type: "touchStart",
      touchPoints: fingers(0),
    });
    event = (ms) =>
      send("Input.dispatchTouchEvent", {
        type: "touchMove",
        touchPoints: fingers(ms),
      });
    release = () =>
      send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
  }
  // A move a frame, at 60 Hz deadlines.
  const start = performance.now();
  for (let frame = 0; frame * FRAME_MS < INTERACTION_MS; frame++) {
    await event(performance.now() - start);
    await sleep(
      Math.max(0, start + (frame + 1) * FRAME_MS - performance.now()),
    );
  }
  await event(INTERACTION_MS);
  await sleep(200);
  await release();
  await cdp?.detach();
}

/** The fetches beyond the first of each URL, the URLs fetched most first. */
export function refetches(fetches) {
  const again = Object.entries(fetches)
    .filter(([, count]) => count > 1)
    .sort(([, a], [, b]) => b - a);
  return {
    count: again.reduce((sum, [, count]) => sum + count - 1, 0),
    urls: again,
  };
}
