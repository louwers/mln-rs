// Checks the browser build in headless Chromium, offline: loads views of the
// render suite's fixtures in the page (web/), reads each frame back from the
// GPU inside the page, and compares it with the native render-test runner's
// render of the same view (pixelmatch, as the render tests compare). It
// fails when a frame differs by more than its threshold, when the page, one of
// its Web Workers (the map's or a tile worker's) or the native runner reports
// an error, or when a view's tiles got no worker replies. The interaction
// views drag on the map back and forth before their frame, over a slow
// network, and also fail when the map fetches URLs again. It also makes the
// map's wasm instance and a tile worker's panic (the page's `panic`), and
// fails unless the page stops and shows the panic, and opens a comparison
// with GL JS (the page's `compare`), and fails unless GL JS's map covers
// the page's and takes the input right of the divider. CI runs it (the
// `web` job); see ../README.md.
//
//   node examples/web/check/check.mjs [options]
//
//   --filter <regex>     the views of VIEWS to load, by name (default: all)
//   --workers <n>        Web Workers for the tiles (default 2)
//   --out <dir>          where the views, frames and results go
//                        (default: a new temporary directory)
//   --render-test <path> the native runner (default:
//                        $CARGO_TARGET_DIR/debug/render-test)
//   --codec <n>          render natively with --tileWorkerCodec=<n>
//   --backend <name>     what the page draws with: webgpu (default), or
//                        webgl2, the module for browsers without WebGPU
//   --chromium <path>    default: Playwright's Chromium CHROMIUM_BUILD
//                        (`npx playwright@1.61.1 install chromium`)
//
// The page's WebGPU adapter is SwiftShader, which Chromium bundles, so the
// frames don't depend on the machine; the native runner renders through
// VK_DRIVER_FILES (default /usr/share/vulkan/icd.d/lvp_icd.x86_64.json,
// lavapipe). Headless Chromium composites WebGPU canvases as white, so the
// page reads its frame back itself. Its WebGL2 runs on SwiftShader too.

import { spawn, spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { homedir, tmpdir } from 'node:os';
import path from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import { crc32, deflateSync, inflateSync } from 'node:zlib';

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, '../../..');
const metrics = path.join(repo, 'tools/render-test/testdata/metrics');

// The render suite's tests the page loads, with their fixtures from the
// suite's cache database: the real-world styles (vector tiles with fills,
// lines, icons and labels), raster tiles, raster-dem tiles (hillshade and
// color relief, whose borders the map side backfills from the neighbouring
// tiles), raster with GeoJSON, and labels in font faces (complex scripts
// and right-to-left text, which the map shapes with the shaper module it
// loads for them).
//
// THRESHOLD is the share of a view's pixels that may differ (pixelmatch,
// with the render tests' per-pixel threshold), unless the view says
// otherwise. The page renders on SwiftShader and the native runner on
// lavapipe; most views come out identical, and leaving out one layer of
// labels changes 0.1% to 2% of the pixels.
const THRESHOLD = 0.001;
const VIEWS = [
  { id: 'real-world/nepal' },
  { id: 'real-world/norway' },
  { id: 'real-world/uruguay' },
  { id: 'real-world/chicago' },
  { id: 'real-world/sanfrancisco' },
  { id: 'raster-rotation/45' },
  { id: 'raster-masking/overlapping-vector' },
  { id: 'hillshade-accent-color/default' },
  // SwiftShader and lavapipe antialias the edges of the z1 coastlines
  // differently: 0.5% of the pixels.
  { id: 'combinations/color-relief--hillshade--vector-fill--color-relief--hillshade', allowed: 0.01 },
  { id: 'text-font/devanagari' },
  { id: 'text-font/khmer' },
  { id: 'text-font/font-faces-bidi' },
  { id: 'is-supported-script/font-faces-layout' },
  // Interactions (interact()): a right-drag that turns the map back and
  // forth, and a pinch that pans and zooms it back and forth, as
  // mln-bench's Frame/Oscillate and Frame/OscillatePanZoom move their
  // cameras. Each starts where some tiles come into view only for moments
  // of each swing, shorter than a tile takes to load.
  {
    id: 'real-world/chicago',
    name: 'interaction/rotate',
    interaction: 'rotate',
    width: 1024,
    height: 768,
    center: [-87.704736328125, 41.86203372809279], // z13 x 2100.23, y 3045.23
  },
  {
    id: 'real-world/sanfrancisco',
    name: 'interaction/pan-zoom',
    interaction: 'pan-zoom',
    width: 1024,
    height: 768,
    center: [-122.4591064453125, 37.77071473849609], // z15 x 5237.5, y 12666
    zoom: 15.9,
  },
];

// In the interaction views every fixture comes this late, and fresh for an
// hour, as over a network, so the map fetches each URL once unless it drops
// what it fetched. A tile it drops while it loads is fetched again on its
// next swing into view, which the browser answers from the first fetch,
// so it loads then. Such a view fails when the map fetches more than
// MAX_REPEATS URLs again.
const LATENCY_MS = 100;
const MAX_REPEATS = 1;

// Playwright 1.61.1's Chromium (Chrome for Testing 149), as CI installs it.
const CHROMIUM_BUILD = 'chromium-1228';

const { values: options } = parseArgs({
  options: {
    filter: { type: 'string', default: '' },
    workers: { type: 'string', default: '2' },
    out: { type: 'string' },
    'render-test': { type: 'string' },
    codec: { type: 'string' },
    backend: { type: 'string', default: 'webgpu' },
    chromium: {
      type: 'string',
      default: path.join(
        process.env.PLAYWRIGHT_BROWSERS_PATH ?? path.join(homedir(), '.cache/ms-playwright'),
        CHROMIUM_BUILD,
        'chrome-linux64/chrome',
      ),
    },
  },
});
const workers = Number(options.workers);
if (!['webgpu', 'webgl2'].includes(options.backend)) throw new Error(`--backend is webgpu or webgl2, not ${options.backend}`);
const icd = process.env.VK_DRIVER_FILES ?? '/usr/share/vulkan/icd.d/lvp_icd.x86_64.json';
if (!existsSync(options.chromium)) {
  throw new Error(`no Chromium at ${options.chromium}: run \`npx playwright@1.61.1 install chromium\` or pass --chromium`);
}

// MARK: - The fixtures: the render suite's cache database

const database = new DatabaseSync(path.join(metrics, 'cache-style.db'), { readOnly: true });
const resourceQuery = database.prepare('SELECT kind, data, compressed FROM resources WHERE url = ?');
const tileQuery = database.prepare('SELECT data, compressed FROM tiles WHERE id = ?');

/** The cache's URL for a URL of the fixtures, as the runner's file source maps it (tools/render-test/src/file_source.rs). */
function cacheUrl(url) {
  for (const prefix of ['local://', 'http://localhost:2900/']) {
    if (url.startsWith(prefix)) return `http://${url.slice(prefix.length)}`;
  }
  return url;
}

// The cache keys tiles by their URL template and coordinates; the page
// requests them by URL.
const tiles = new Map();
for (const { id, url_template: template, pixel_ratio: ratio, z, x, y } of database
  .prepare('SELECT id, url_template, pixel_ratio, z, x, y FROM tiles')
  .all()) {
  const url = template
    .replaceAll('{z}', z)
    .replaceAll('{x}', x)
    .replaceAll('{y}', y)
    .replaceAll('{ratio}', ratio > 1 ? `@${ratio}x` : '');
  tiles.set(url, id);
}

function unpack(row) {
  return row.compressed ? inflateSync(row.data) : Buffer.from(row.data);
}

/** The cached resource or tile at a URL of the fixtures, if there is one. */
function cached(url) {
  const key = cacheUrl(url);
  const resource = resourceQuery.get(key);
  if (resource) return { kind: resource.kind, data: unpack(resource) };
  const tile = tiles.has(key) ? tileQuery.get(tiles.get(key)) : undefined;
  return tile ? { kind: 'tile', data: unpack(tile) } : undefined;
}

/** A cached JSON document, such as a style. */
function cachedJson(url) {
  const entry = cached(url);
  if (!entry) throw new Error(`${url} is not in the cache`);
  return JSON.parse(entry.data.toString('utf8'));
}

// MARK: - Views

/**
 * A view: the render test's style, with its operations followed, so that
 * the camera is in the style. The page can't follow other operations or
 * test options, so a view must not have them.
 */
function view({ id, name = id, interaction, allowed = THRESHOLD, ...camera }) {
  const test = JSON.parse(readFileSync(path.join(metrics, 'integration/render-tests', id, 'style.json'), 'utf8'));
  const metadata = test.metadata?.test ?? {};
  for (const key of Object.keys(metadata)) {
    if (!['width', 'height', 'pixelRatio', 'operations', 'allowed', 'description'].includes(key)) {
      throw new Error(`${id}: the page can't follow the test's "${key}"`);
    }
  }
  let style = test;
  for (const [operation, ...args] of metadata.operations ?? []) {
    if (operation === 'setStyle') style = cachedJson(args[0]);
    else if (operation === 'setZoom') style.zoom = args[0];
    else if (operation === 'setCenter') style.center = args[0];
    else if (operation !== 'wait') throw new Error(`${id}: the page can't follow the operation ${operation}`);
  }
  style = structuredClone(style);
  if (camera.center) style.center = camera.center;
  if (camera.zoom !== undefined) style.zoom = camera.zoom;
  style.center ??= [0, 0];
  style.zoom ??= 0;
  style.bearing ??= 0;
  style.pitch ??= 0;
  const width = camera.width ?? metadata.width ?? 512;
  const height = camera.height ?? metadata.height ?? 512;
  const ratio = metadata.pixelRatio ?? 1;
  style.metadata = { test: { width, height, pixelRatio: ratio, allowed } };
  return { id, name, interaction, style, width, height, ratio, allowed };
}

const views = VIEWS.filter(({ id, name = id }) => new RegExp(options.filter).test(name)).map(view);
if (views.length === 0) throw new Error(`no view matches ${options.filter}`);

// MARK: - The page, served from web/, and the fixtures

// The page as Vite builds it, as it deploys.
const root = path.join(here, '../dist');
if (!existsSync(path.join(root, 'index.html'))) throw new Error('no dist: run `mise run web:build` first');
const types = {
  '.html': 'text/html',
  '.js': 'text/javascript',
  '.mjs': 'text/javascript',
  '.css': 'text/css',
  '.wasm': 'application/wasm',
  '.json': 'application/json',
};

// Every URL the page loads is on this server: `/fixtures/<scheme>/<rest>`
// serves the cache's `<scheme>://<rest>`.
let origin;
const fixtureUrl = (url) => {
  const match = /^([a-z]+):\/\/(.*)$/.exec(url);
  return match ? `${origin}/fixtures/${match[1]}/${match[2]}` : url;
};

/** The style or TileJSON with each URL in it on this server. */
function served(json) {
  const copy = structuredClone(json);
  if (Array.isArray(copy.tiles)) copy.tiles = copy.tiles.map(fixtureUrl);
  for (const source of Object.values(copy.sources ?? {})) {
    if (typeof source.url === 'string') source.url = fixtureUrl(source.url);
    if (Array.isArray(source.urls)) source.urls = source.urls.map(fixtureUrl);
    if (Array.isArray(source.tiles)) source.tiles = source.tiles.map(fixtureUrl);
    if (typeof source.data === 'string') source.data = fixtureUrl(source.data);
  }
  if (typeof copy.sprite === 'string') copy.sprite = fixtureUrl(copy.sprite);
  if (Array.isArray(copy.sprite)) copy.sprite = copy.sprite.map((sprite) => ({ ...sprite, url: fixtureUrl(sprite.url) }));
  if (typeof copy.glyphs === 'string') copy.glyphs = fixtureUrl(copy.glyphs);
  for (const [name, faces] of Object.entries(copy['font-faces'] ?? {})) {
    copy['font-faces'][name] = typeof faces === 'string'
      ? fixtureUrl(faces)
      : faces.map((face) => ({ ...face, url: fixtureUrl(face.url) }));
  }
  return copy;
}

const SOURCE = 2; // Resource::Kind::Source: a TileJSON
// The fixtures the current view asked for that the cache doesn't have. The
// runner's cache-only file source answers those as not found, and so does
// this server (404, which HTTPFileSource reads as a tile the tileset doesn't
// have, or as a Not Found error for other resources).
let missing = [];
// Whether the current view's fixtures come as over a network (LATENCY_MS).
let slow = false;

function serveFixture(url, response) {
  const entry = cached(url);
  if (!entry) {
    missing.push(url);
    response.writeHead(404).end();
  } else if (entry.kind === SOURCE) {
    response.writeHead(200, { 'content-type': 'application/json' });
    response.end(JSON.stringify(served(JSON.parse(entry.data.toString('utf8')))));
  } else {
    response.writeHead(200, { 'content-type': 'application/octet-stream' }).end(entry.data);
  }
}

const server = createServer((request, response) => {
  const url = new URL(request.url, 'http://x');
  const fixture = /^\/fixtures\/([a-z]+)\/(.*)$/.exec(request.url);
  if (fixture && slow) {
    response.setHeader('cache-control', 'max-age=3600');
    setTimeout(() => serveFixture(`${fixture[1]}://${fixture[2]}`, response), LATENCY_MS);
    return;
  }
  if (fixture) {
    serveFixture(`${fixture[1]}://${fixture[2]}`, response);
    return;
  }
  const style = /^\/style\/(\d+)$/.exec(url.pathname);
  if (style) {
    response.writeHead(200, { 'content-type': 'application/json' });
    response.end(JSON.stringify(served(views[Number(style[1])].style)));
    return;
  }
  if (url.pathname === '/blank') {
    response.writeHead(200, { 'content-type': 'text/html' }).end('<!doctype html>');
    return;
  }
  const file = path.join(root, path.normalize(decodeURIComponent(url.pathname)));
  const target = file.endsWith(path.sep) ? path.join(file, 'index.html') : file;
  if (!target.startsWith(root) || !existsSync(target)) {
    response.writeHead(404).end();
    return;
  }
  response.writeHead(200, { 'content-type': types[path.extname(target)] ?? 'application/octet-stream' });
  response.end(readFileSync(target));
});
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
origin = `http://127.0.0.1:${server.address().port}`;

const pageUrl = (view, count) => {
  const params = new URLSearchParams({
    style: `${origin}/style/${views.indexOf(view)}`,
    lon: view.style.center[0],
    lat: view.style.center[1],
    zoom: view.style.zoom,
    bearing: view.style.bearing,
    pitch: view.style.pitch,
    width: view.width,
    height: view.height,
    ratio: view.ratio,
    workers: String(count),
    backend: options.backend,
    // The native runner downloads every glyph, as GL JS's render tests do.
    localIdeographFontFamily: 'false',
  });
  return `${origin}/index.html?${params}`;
};

// MARK: - Chromium over the DevTools protocol

async function launch() {
  const profile = await mkdtemp(path.join(tmpdir(), 'mln-web-check-profile-'));
  const env = { ...process.env };
  delete env.VK_DRIVER_FILES; // the native runner's driver
  const child = spawn(
    options.chromium,
    [
      '--headless=new',
      '--no-sandbox',
      '--no-first-run',
      '--password-store=basic',
      '--use-mock-keychain',
      '--disable-dev-shm-usage',
      '--enable-unsafe-webgpu',
      // SwiftShader, which Chromium bundles, for WebGPU and for compositing
      // through Vulkan: with headless Chromium's default GL compositing, the
      // copy of the frame that the page reads back is never mapped.
      '--enable-features=Vulkan',
      '--use-angle=vulkan',
      '--use-vulkan=swiftshader',
      '--use-webgpu-adapter=swiftshader',
      // Offline: a request that escapes the local server (a URL that
      // served() doesn't rewrite) fails to resolve, an error in the page
      // or a worker, rather than reaching the network.
      '--host-resolver-rules=MAP * ~NOTFOUND , EXCLUDE 127.0.0.1',
      '--remote-debugging-port=0',
      `--user-data-dir=${profile}`,
      'about:blank',
    ],
    { stdio: ['ignore', 'ignore', 'pipe'], env },
  );
  const exited = new Promise((resolve) => child.on('exit', resolve));
  let log = '';
  const url = await new Promise((resolve, reject) => {
    child.stderr.on('data', (chunk) => {
      log += chunk;
      const match = /DevTools listening on (ws:\/\/\S+)/.exec(log);
      if (match) resolve(match[1]);
    });
    exited.then((code) => reject(new Error(`chromium exited with ${code}\n${log}`)));
  });
  const socket = new WebSocket(url);
  await new Promise((resolve, reject) => {
    socket.onopen = resolve;
    socket.onerror = reject;
  });
  let next = 1;
  const pending = new Map();
  const listeners = new Set();
  socket.onmessage = (event) => {
    const message = JSON.parse(event.data);
    if (message.id === undefined) {
      for (const listener of listeners) listener(message);
      return;
    }
    const entry = pending.get(message.id);
    pending.delete(message.id);
    if (message.error) entry.reject(new Error(`${entry.method}: ${JSON.stringify(message.error)}`));
    else entry.resolve(message.result);
  };
  const send = (method, params = {}, sessionId = undefined) =>
    new Promise((resolve, reject) => {
      const id = next++;
      pending.set(id, { resolve, reject, method });
      socket.send(JSON.stringify({ id, method, params, sessionId }));
    });
  return {
    send,
    listeners,
    async close() {
      socket.close();
      child.kill();
      await exited;
      await rm(profile, { recursive: true, force: true, maxRetries: 5 });
    },
  };
}

const text = (args) => args.map((arg) => arg.value ?? arg.description).join(' ');

/**
 * Opens `url` in a new tab; returns the session, with the errors that its
 * console and its Web Workers' consoles report, and `failed`, which
 * resolves at the first of them.
 */
async function open(browser, url) {
  const { targetId } = await browser.send('Target.createTarget', { url: 'about:blank' });
  const { sessionId } = await browser.send('Target.attachToTarget', { targetId, flatten: true });
  const errors = [];
  let fail;
  const failed = new Promise((resolve) => {
    fail = resolve;
  });
  const report = (error) => {
    errors.push(error);
    fail();
  };
  const sessions = new Map([[sessionId, 'page']]);
  const listener = (message) => {
    // The Web Workers: each starts paused until its console is attached.
    if (message.method === 'Target.attachedToTarget' && sessions.has(message.sessionId)) {
      const worker = message.params.sessionId;
      sessions.set(worker, `worker ${sessions.size}`);
      browser
        .send('Runtime.enable', {}, worker)
        .then(() => browser.send('Runtime.runIfWaitingForDebugger', {}, worker))
        .catch((error) => report(`attaching to a worker: ${error.message}`));
      return;
    }
    const where = sessions.get(message.sessionId);
    if (where === undefined) return;
    if (process.env.MLN_WEB_CHECK_LOG && message.method === 'Runtime.consoleAPICalled') {
      console.error(`[${where}] ${text(message.params.args)}`);
    }
    if (message.method === 'Runtime.consoleAPICalled' && message.params.type === 'error') {
      report(`${where}: ${text(message.params.args)}`);
    } else if (message.method === 'Runtime.exceptionThrown') {
      const details = message.params.exceptionDetails;
      report(`${where}: ${details.exception?.description ?? details.text}`);
    }
  };
  browser.listeners.add(listener);
  await browser.send('Target.setAutoAttach', { autoAttach: true, waitForDebuggerOnStart: true, flatten: true }, sessionId);
  await browser.send('Runtime.enable', {}, sessionId);
  await browser.send('Page.enable', {}, sessionId);
  const loaded = new Promise((resolve) => {
    const onLoad = (message) => {
      if (message.sessionId === sessionId && message.method === 'Page.loadEventFired') {
        browser.listeners.delete(onLoad);
        resolve();
      }
    };
    browser.listeners.add(onLoad);
  });
  await browser.send('Page.navigate', { url }, sessionId);
  await loaded;
  return {
    targetId,
    sessionId,
    errors,
    failed,
    async close() {
      await browser.send('Target.closeTarget', { targetId });
      browser.listeners.delete(listener);
    },
  };
}

async function evaluate(browser, session, expression) {
  const result = await browser.send(
    'Runtime.evaluate',
    { expression, awaitPromise: true, returnByValue: true },
    session.sessionId,
  );
  if (result.exceptionDetails) {
    throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
  }
  return result.result.value;
}

/**
 * Loads `view` with `count` workers; resolves with the page's statistics,
 * its frame, the errors the page and its workers reported, and why it
 * couldn't load or read the frame, if it couldn't. An error stops the
 * loading: the tiles of a worker that panicked never load.
 */
async function load(browser, view, count) {
  const session = await open(browser, pageUrl(view, count));
  const failed = session.failed.then(async () => {
    await new Promise((resolve) => setTimeout(resolve, 500)); // the errors that follow
    throw new Error('the page or a worker reported an error');
  });
  const run = async () => {
    const stats = await evaluate(
      browser,
      session,
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
    const backend = await evaluate(browser, session, 'window.mln.backend');
    if (backend !== options.backend) throw new Error(`the page drew with ${backend}, not ${options.backend}`);
    let interaction;
    if (view.interaction) {
      await interact(browser, session, view.interaction);
      const camera = await evaluate(
        browser,
        session,
        `Promise.race([
          window.mln.idle(),
          new Promise((_, reject) => setTimeout(() => reject(new Error('not idle after 60 s')), 60000)),
        ])`,
      );
      interaction = { camera, fetches: await evaluate(browser, session, 'window.mln.fetches()') };
    }
    // Let the labels fade in, then draw the frame again and read it back.
    await evaluate(browser, session, 'new Promise((resolve) => setTimeout(resolve, 1500))');
    const pixels = await evaluate(
      browser,
      session,
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
    return { failure: error.message.split('\n')[0], errors: session.errors };
  } finally {
    await session.close();
  }
}

// MARK: - Interactions

// How long an interaction drags back and forth, and how often it moves.
const INTERACTION_MS = 2000;
const FRAME_MS = 1000 / 60;

/** From 0 at `ms`: to 1, then between -1 and 1, each swing half a second, back to 0 after each second. */
function swing(ms) {
  const phase = (((ms / 1000 - 0.25) % 1) + 1) % 1;
  return 4 * Math.abs(phase - 0.5) - 1;
}

/**
 * Drags on the canvas back and forth for INTERACTION_MS through the page's
 * input, as the browser delivers a user's (the DevTools protocol's mouse
 * and touch events), one move a frame, and releases where it started:
 * - `rotate`: a right-drag from the center, which turns the map between
 *   -90° and 90° (mln_gestures' 0.5° a pixel);
 * - `pan-zoom`: a two-finger pinch that pans the map 0.65 of a tile east
 *   and west of where it started while it zooms out by 0.9 at each end.
 * It holds still before the release, longer than the recognizer's 40 ms,
 * so that the release starts no momentum.
 */
async function interact(browser, session, interaction) {
  const { x, y } = await evaluate(
    browser,
    session,
    `(() => {
      const rect = document.getElementById('map').getBoundingClientRect();
      return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
    })()`,
  );
  const send = (method, params) => browser.send(method, params, session.sessionId);
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  let event, release;
  if (interaction === 'rotate') {
    const mouse = (type, at) =>
      send('Input.dispatchMouseEvent', {
        type,
        x: at,
        y,
        button: 'right',
        buttons: type === 'mouseReleased' ? 0 : 2,
        clickCount: type === 'mouseMoved' ? 0 : 1,
      });
    await mouse('mousePressed', x);
    event = (ms) => mouse('mouseMoved', x + 180 * swing(ms));
    release = () => mouse('mouseReleased', x);
  } else {
    await send('Emulation.setTouchEmulationEnabled', { enabled: true, maxTouchPoints: 2 });
    // The fingers 200 pixels apart where they start, closer by up to 2^-0.9.
    // The map's center follows the fingers' midpoint, at the zoom's scale:
    // 0.65 of a tile a swing, 333 pixels at the zoom of its ends.
    const fingers = (ms) => {
      const out = Math.abs(swing(ms));
      const at = x + 333 * swing(ms) * 2 ** (0.9 * (1 - out));
      const apart = 100 * 2 ** (-0.9 * out);
      return [{ x: at - apart, y, id: 0 }, { x: at + apart, y, id: 1 }];
    };
    await send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: fingers(0) });
    event = (ms) => send('Input.dispatchTouchEvent', { type: 'touchMove', touchPoints: fingers(ms) });
    release = () => send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
  }
  // A move a frame, at 60 Hz deadlines.
  const start = performance.now();
  for (let frame = 0; frame * FRAME_MS < INTERACTION_MS; frame++) {
    await event(performance.now() - start);
    await sleep(Math.max(0, start + (frame + 1) * FRAME_MS - performance.now()));
  }
  await event(INTERACTION_MS);
  await sleep(200);
  await release();
}

/** The fetches beyond the first of each URL, the URLs fetched most first. */
function refetches(fetches) {
  const again = Object.entries(fetches).filter(([, count]) => count > 1).sort(([, a], [, b]) => b - a);
  return { count: again.reduce((sum, [, count]) => sum + count - 1, 0), urls: again };
}

// MARK: - PNG

function png(width, height, rgba) {
  const chunk = (type, data) => {
    const length = Buffer.alloc(4);
    length.writeUInt32BE(data.length);
    const body = Buffer.concat([Buffer.from(type), data]);
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(crc32(body));
    return Buffer.concat([length, body, crc]);
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header.set([8, 6, 0, 0, 0], 8); // 8-bit RGBA
  const rows = Buffer.alloc((width * 4 + 1) * height);
  for (let y = 0; y < height; y++) {
    rgba.copy(rows, y * (width * 4 + 1) + 1, y * width * 4, (y + 1) * width * 4);
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', header),
    chunk('IDAT', deflateSync(rows)),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

// MARK: - Main

const browser = await launch();
let failures = [];
try {
  const gpu = await (async () => {
    const session = await open(browser, `${origin}/blank`);
    try {
      return await evaluate(
        browser,
        session,
        `navigator.gpu?.requestAdapter().then((a) => (a ? \`\${a.info.vendor} \${a.info.architecture}\` : 'none'))`,
      );
    } finally {
      await session.close();
    }
  })();
  console.log(`WebGPU adapter: ${gpu}`);
  if (!/swiftshader/.test(gpu ?? '')) throw new Error(`the page's WebGPU adapter is ${gpu}, not SwiftShader`);

  failures = await compare();
  for (const [where, instance] of [['map', "the map's instance"], ['tile', "a tile worker's instance"]]) {
    const failure = await checkPanic(browser, views[0], where);
    console.log(`a panic in ${instance}: ${failure ?? 'the page stopped and showed it'}`);
    if (failure) failures.push(`a panic in ${instance}: ${failure}`);
  }
  const comparing = await checkCompare(browser, views[0]);
  console.log(`a comparison with GL JS: ${comparing ?? "GL JS's map covers the page's, right of the divider"}`);
  if (comparing) failures.push(`a comparison with GL JS: ${comparing}`);
} finally {
  await browser.close();
  server.close();
  database.close();
}
if (failures.length > 0) {
  console.log(`\nfailed:\n  ${failures.join('\n  ')}`);
  process.exitCode = 1;
}

/**
 * Loads `view` in the whole window with a comparison with GL JS (the page's
 * `compare`); returns how GL JS's map failed to cover the page's map, or to
 * take the input right of the divider, if it did.
 */
async function checkCompare(browser, view) {
  const url = new URL(pageUrl(view, workers));
  for (const name of ['width', 'height', 'ratio']) url.searchParams.delete(name);
  url.searchParams.set('compare', '');
  const session = await open(browser, url.href);
  try {
    return await evaluate(
      browser,
      session,
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

/**
 * Loads `view` with the map's instance or the first tile worker's panicking
 * at its start (the page's `panic`); returns how the page failed to stop
 * and show the panic, if it did.
 */
async function checkPanic(browser, view, where) {
  const session = await open(browser, `${pageUrl(view, workers)}&panic=${where}`);
  try {
    const page = await evaluate(
      browser,
      session,
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
    const expected = `${where === 'map' ? 'map' : 'tile worker 0'}: panicked at `;
    if (!page.fatal.startsWith(expected) || !page.fatal.includes('panic=<map|tile> asked for this panic')) {
      return `the page's fatal error is ${JSON.stringify(page.fatal)}`;
    }
    if (page.loaded !== page.fatal) return `\`loaded\` is ${page.loaded}`;
    if (page.readPixels !== page.fatal) return `\`readPixels()\` is ${page.readPixels}`;
    if (!page.banner.includes(page.fatal)) return `the page shows ${JSON.stringify(page.banner)}`;
    return null;
  } catch (error) {
    return firstLines(error.message);
  } finally {
    await session.close();
  }
}

/** Loads and compares every view; returns what failed. */
async function compare() {
  const out = path.resolve(options.out ?? (await mkdtemp(path.join(tmpdir(), 'mln-web-check-'))));
  // A kept --out holds an earlier run's views and results; without this a
  // runner that writes no results would leave the old ones to be read.
  for (const dir of ['tests', 'results']) await rm(path.join(out, dir), { recursive: true, force: true });
  const failures = [];
  for (const view of views) {
    missing = [];
    slow = view.interaction !== undefined;
    const { stats, pixels, interaction, errors, failure } = await load(browser, view, workers);
    slow = false;
    const again = interaction && refetches(interaction.fetches);
    if (process.env.MLN_WEB_CHECK_LOG && again) console.error(`  fetched again: ${JSON.stringify(again.urls)}`);
    console.log(`${view.name}: ` +
      (failure ? failure : `loaded in ${Math.round(stats.loadedMs)} ms, ${stats.replies} worker replies`) +
      (again ? `, after the ${view.interaction} ${again.count} fetches again (at most ${MAX_REPEATS})` : '') +
      (missing.length ? `, ${missing.length} tiles or resources not in the cache (not found)` : '') +
      (errors.length ? `, ${errors.length} errors:\n  ${errors.map(firstLines).join('\n  ')}` : ''));
    if (process.env.MLN_WEB_CHECK_LOG && missing.length) console.error(`  not in the cache: ${missing.join(', ')}`);
    if (failure) {
      failures.push(`${view.name}: ${failure}`);
      continue;
    }
    if (errors.length) failures.push(`${view.name}: ${errors.length} errors in the page or its workers`);
    // Every view has tiles, and only their workers reply: the raster and
    // raster-dem tiles' too, in the views that have nothing else.
    if (stats.replies === 0) failures.push(`${view.name}: no worker replies`);
    if (again && again.count > MAX_REPEATS) {
      const urls = again.urls.slice(0, 3).map(([url, count]) => `${url.replace(origin, '')} ${count} times`);
      failures.push(`${view.name}: ${again.count} fetches again after the ${view.interaction}, of ${urls.join(', ')}`);
    }
    // An interaction's frame is of the camera it left the map at.
    let style = view.style;
    if (interaction) {
      const [lat, lon, zoom, bearing, pitch] = interaction.camera;
      style = { ...style, center: [lon, lat], zoom, bearing, pitch };
    }
    const dir = path.join(out, 'tests/render-tests', view.name);
    await mkdir(dir, { recursive: true });
    await writeFile(path.join(dir, 'style.json'), JSON.stringify(style, null, 1));
    await writeFile(path.join(dir, 'expected.png'), png(pixels.width, pixels.height, Buffer.from(pixels.base64, 'base64')));
  }
  if (!existsSync(path.join(out, 'tests'))) return failures;

  // The native renders, compared with the page's frames as references.
  const manifest = path.join(out, 'manifest.json');
  await writeFile(manifest, JSON.stringify({
    base_test_path: path.join(out, 'tests'),
    cache_path: path.join(metrics, 'cache-style.db'),
    expectation_paths: [],
    ignore_paths: [],
  }));
  const targetDir = process.env.CARGO_TARGET_DIR ?? path.join(repo, 'target');
  const runner = options['render-test'] ?? path.join(targetDir, 'debug/render-test');
  const args = ['-p', manifest, '--output', path.join(out, 'results')];
  if (options.codec) args.push(`--tileWorkerCodec=${options.codec}`);
  const native = spawnSync(runner, args, { env: { ...process.env, VK_DRIVER_FILES: icd }, encoding: 'utf8' });
  if (native.error) throw native.error;
  if (!existsSync(path.join(out, 'results/manifest.html'))) {
    console.log(native.stdout, native.stderr);
    return [...failures, `the native runner exited with ${native.status} and no results`];
  }
  const page = await readFile(path.join(out, 'results/manifest.html'), 'utf8');
  console.log(`\nnative render (${runner}${options.codec ? `, codec with ${options.codec} instances` : ''}) ` +
    `against the page's frame, ${workers} tile workers:`);
  const results = new Map();
  for (const item of page.split('<div class="test ').slice(1)) {
    const id = /<\/span> render-tests\/([^<]+)<\/h2>/.exec(item)?.[1];
    const status = /^(\w+)/.exec(item)?.[1];
    const diff = Number(/<strong>Diff:<\/strong> ([0-9.e-]+)/.exec(item)?.[1] ?? 0);
    const error = /<strong>Error:<\/strong> ([^<]*)/.exec(item)?.[1];
    results.set(id, { status, diff, error });
  }
  // The report has an entry only for a failed or errored test. Every other
  // test in its sequence passed, under its view's threshold.
  const sequence = /<strong>Test sequence:<\/strong>([^<]*)<\/p>/.exec(page)?.[1].trim().split(/\s+/) ?? [];
  for (const id of sequence) {
    const name = id.replace(/^render-tests\//, '');
    if (!results.has(name)) results.set(name, { status: 'passed' });
  }
  for (const view of views) {
    if (!existsSync(path.join(out, 'tests/render-tests', view.name))) continue;
    const result = results.get(view.name);
    if (!result) {
      console.log(`  ${view.name}: not rendered`);
      failures.push(`${view.name}: the native runner didn't render it`);
    } else if (result.error) {
      console.log(`  ${view.name}: error: ${result.error}`);
      failures.push(`${view.name}: the native runner: ${result.error}`);
    } else if (result.diff === undefined) {
      console.log(`  ${view.name}: passed (at most ${(view.allowed * 100).toFixed(1)}% of the pixels differ)`);
    } else {
      console.log(`  ${view.name}: ${(result.diff * 100).toFixed(3)}% of the pixels differ ` +
        `(at most ${(view.allowed * 100).toFixed(1)}%)`);
      if (result.status !== 'passed' || result.diff > view.allowed) {
        failures.push(`${view.name}: ${(result.diff * 100).toFixed(3)}% of the pixels differ (${result.status})`);
      }
    }
  }
  console.log(`\nviews, frames and diffs: ${out}`);
  return failures;
}

/** An error's message, without the rest of its stack. */
function firstLines(error) {
  return error.split('\n').slice(0, 2).join(' ');
}
