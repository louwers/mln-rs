// The page under test (web/), driven through its `window.mln` hooks. Each
// function takes the check's context: `{ browser, server, workers,
// backend }`, the browser that opens the page, the server it loads from,
// how many tile workers it starts and what it draws with.

import { interact } from "./interactions.mjs";

/** The page's URL for `view`. */
export function pageUrl({ server, workers, backend }, view) {
  const params = new URLSearchParams({
    style: server.styleUrl(view),
    lon: view.style.center[0],
    lat: view.style.center[1],
    zoom: view.style.zoom,
    bearing: view.style.bearing,
    pitch: view.style.pitch,
    width: view.width,
    height: view.height,
    ratio: view.ratio,
    workers: String(workers),
    backend,
    // The native runner downloads every glyph, as GL JS's render tests do.
    localIdeographFontFamily: "false",
  });
  return `${server.origin}/index.html?${params}`;
}

/** An error's message, without the rest of its stack. */
export function firstLines(error) {
  return error.split("\n").slice(0, 2).join(" ");
}

/** The name of the page's WebGPU adapter, or `"none"`. */
export async function webgpuAdapter({ browser, server }) {
  const session = await browser.open(`${server.origin}/blank`);
  try {
    return await session.evaluate(
      `navigator.gpu?.requestAdapter().then((a) => (a ? \`\${a.info.vendor} \${a.info.architecture}\` : 'none'))`,
    );
  } finally {
    await session.close();
  }
}

/**
 * Loads `view`; resolves with the page's statistics, its frame, the
 * camera and fetches after the view's interaction, if it has one, the
 * errors the page and its workers reported, and why it couldn't load or
 * read the frame, if it couldn't. An error stops the loading: the tiles of
 * a worker that panicked never load.
 */
export async function load(context, view) {
  const session = await context.browser.open(pageUrl(context, view));
  const failed = session.failed.then(async () => {
    await new Promise((resolve) => setTimeout(resolve, 500)); // the errors that follow
    throw new Error("the page or a worker reported an error");
  });
  const run = async () => {
    const stats = await session.evaluate(
      `(async () => {
        const deadline = performance.now() + 60000;
        while (!window.mln) {
          if (performance.now() > deadline) throw new Error('the page did not start');
          await new Promise((resolve) => setTimeout(resolve, 50));
        }
        return await Promise.race([
          // A panic's message, on one line.
          window.mln.loaded.catch((error) => {
            throw new Error(error.message.replaceAll('\\n', ' '));
          }),
          new Promise((_, reject) => setTimeout(() => reject(new Error('not loaded after 60 s')), 60000)),
        ]);
      })()`,
    );
    const backend = await session.evaluate("window.mln.backend");
    if (backend !== context.backend)
      throw new Error(`the page drew with ${backend}, not ${context.backend}`);
    let interaction;
    if (view.interaction) {
      await interact(session, view.interaction);
      const camera = await session.evaluate(
        `Promise.race([
          window.mln.idle(),
          new Promise((_, reject) => setTimeout(() => reject(new Error('not idle after 60 s')), 60000)),
        ])`,
      );
      interaction = {
        camera,
        fetches: await session.evaluate("window.mln.fetches()"),
      };
    }
    // Let the labels fade in, then draw the frame again and read it back.
    await session.evaluate(
      "new Promise((resolve) => setTimeout(resolve, 1500))",
    );
    const pixels = await session.evaluate(
      `(async () => {
        const { width, height, pixels } = await window.mln.readPixels();
        let binary = '';
        for (let i = 0; i < pixels.length; i += 0x8000) {
          binary += String.fromCharCode(...pixels.subarray(i, i + 0x8000));
        }
        return { width, height, base64: btoa(binary) };
      })()`,
    );
    return { stats, pixels, interaction };
  };
  try {
    return { ...(await Promise.race([run(), failed])), errors: session.errors };
  } catch (error) {
    return { failure: error.message.split("\n")[0], errors: session.errors };
  } finally {
    await session.close();
  }
}

/**
 * Loads `view` with the map's instance (`where` is `"map"`) or the first
 * tile worker's (`"tile"`) panicking at its start (the page's `panic`);
 * returns how the page failed to stop and show the panic, if it did.
 */
export async function checkPanic(context, view, where) {
  const session = await context.browser.open(
    `${pageUrl(context, view)}&panic=${where}`,
  );
  try {
    const page = await session.evaluate(
      `(async () => {
        const deadline = performance.now() + 60000;
        while (!window.mln) {
          if (performance.now() > deadline) throw new Error('the page did not start');
          await new Promise((resolve) => setTimeout(resolve, 50));
        }
        const fatal = await Promise.race([
          window.mln.fatal,
          new Promise((_, reject) => setTimeout(() => reject(new Error('no fatal error after 60 s')), 60000)),
        ]);
        const message = (promise) => promise.then(() => 'resolved', (error) => error.message);
        const banner = document.getElementById('error');
        return {
          fatal: fatal.message,
          loaded: await message(window.mln.loaded),
          readPixels: await message(window.mln.readPixels()),
          banner: banner.hidden ? 'hidden' : banner.textContent,
        };
      })()`,
    );
    const expected = `${where === "map" ? "map" : "tile worker 0"}: panicked at `;
    if (
      !page.fatal.startsWith(expected) ||
      !page.fatal.includes("panic=<map|tile> asked for this panic")
    ) {
      return `the page's fatal error is ${JSON.stringify(page.fatal)}`;
    }
    if (page.loaded !== page.fatal) return `\`loaded\` is ${page.loaded}`;
    if (page.readPixels !== page.fatal)
      return `\`readPixels()\` is ${page.readPixels}`;
    if (!page.banner.includes(page.fatal))
      return `the page shows ${JSON.stringify(page.banner)}`;
    return null;
  } catch (error) {
    return firstLines(error.message);
  } finally {
    await session.close();
  }
}

/**
 * Loads `view` in the whole window with a comparison with GL JS (the page's
 * `compare`); returns how GL JS's map failed to cover the page's map, or to
 * take the input right of the divider, if it did.
 */
export async function checkCompare(context, view) {
  const url = new URL(pageUrl(context, view));
  for (const name of ["width", "height", "ratio"])
    url.searchParams.delete(name);
  url.searchParams.set("compare", "");
  const session = await context.browser.open(url.href);
  try {
    return await session.evaluate(
      `(async () => {
        const deadline = performance.now() + 60000;
        let gl;
        while (!(gl = document.querySelector('.gl-map canvas'))) {
          if (performance.now() > deadline) return "GL JS's map did not open";
          await new Promise((resolve) => setTimeout(resolve, 50));
        }
        const map = document.getElementById('map');
        const box = (element) => {
          const { left, top, width, height } = element.getBoundingClientRect();
          return [left, top, width, height].map(Math.round).join(',');
        };
        if (box(gl) !== box(map)) return \`GL JS's map is at \${box(gl)}, the page's at \${box(map)}\`;
        const { top, height, left, width } = map.getBoundingClientRect();
        const at = (x) => document.elementFromPoint(left + width * x, top + height / 2);
        if (!gl.parentElement.closest('.gl-layer')?.contains(at(0.75))) return "GL JS's map takes no input right of the divider";
        if (at(0.25) !== map) return "the page's map takes no input left of the divider";
        return null;
      })()`,
    );
  } catch (error) {
    return firstLines(error.message);
  } finally {
    await session.close();
  }
}
