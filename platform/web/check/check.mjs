// Checks the browser build in headless Chromium: loads OpenFreeMap views in
// the page (web/), reads each frame back from the GPU inside the page, and
// compares it with a native render of the same view by the render-test
// runner (pixelmatch, as the render tests compare). See ../README.md.
//
//   node platform/web/check/check.mjs [options]
//
//   --filter <regex>     views of tools/liberty-compare/views.txt to load, by
//                        "style/name" (default: seven Liberty city views, z12 to z15)
//   --workers <n>        Web Workers for the tiles (default 2)
//   --out <dir>          where the views, references and results go
//                        (default: a new temporary directory)
//   --render-test <path> the native runner (default:
//                        $CARGO_TARGET_DIR/debug/render-test)
//   --codec <n>          render natively with --tileWorkerCodec=<n>
//   --chromium <path>    default /usr/bin/chromium-browser
//
// The GPU is lavapipe (VK_DRIVER_FILES, default
// /usr/share/vulkan/icd.d/lvp_icd.x86_64.json): headless Chromium composites
// WebGPU canvases as white, so the page reads its frame back itself.

import { spawn, spawnSync } from 'node:child_process';
import { createReadStream, existsSync } from 'node:fs';
import { mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import { crc32, deflateSync } from 'node:zlib';

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, '../../..');
const { values: options } = parseArgs({
  options: {
    filter: { type: 'string', default: '^liberty/(cairo12|ams13|tokyo13|nyc14|berlin14r2|ams15|sf15p50)$' },
    workers: { type: 'string', default: '2' },
    out: { type: 'string' },
    'render-test': { type: 'string' },
    codec: { type: 'string' },
    chromium: { type: 'string', default: '/usr/bin/chromium-browser' },
  },
});
const workers = Number(options.workers);
const icd = process.env.VK_DRIVER_FILES ?? '/usr/share/vulkan/icd.d/lvp_icd.x86_64.json';

// MARK: - Views

const views = (await readFile(path.join(repo, 'tools/liberty-compare/views.txt'), 'utf8'))
  .split('\n')
  .filter((line) => line.trim() && !line.startsWith('#'))
  .map((line) => {
    const [style, name, lon, lat, zoom, bearing, pitch, ratio, width, height] = line.trim().split(/\s+/);
    return { style, name, lon, lat, zoom, bearing, pitch, ratio, width, height };
  })
  .filter((view) => new RegExp(options.filter).test(`${view.style}/${view.name}`));
if (views.length === 0) throw new Error(`no view matches ${options.filter}`);

const styleUrl = (style) => `https://tiles.openfreemap.org/styles/${style}`;

// MARK: - The page, served from web/

const root = path.join(here, '../web');
if (!existsSync(path.join(root, 'pkg/mln_web_bg.wasm'))) throw new Error('no web/pkg: run `just web` first');
const types = { '.html': 'text/html', '.js': 'text/javascript', '.wasm': 'application/wasm', '.json': 'application/json' };
const server = createServer((request, response) => {
  if (request.url === '/blank') {
    response.writeHead(200, { 'content-type': 'text/html' }).end('<!doctype html>');
    return;
  }
  const file = path.join(root, path.normalize(decodeURIComponent(new URL(request.url, 'http://x').pathname)));
  if (!file.startsWith(root) || !existsSync(file)) {
    response.writeHead(404).end();
    return;
  }
  const target = file.endsWith(path.sep) ? path.join(file, 'index.html') : file;
  response.writeHead(200, { 'content-type': types[path.extname(target)] ?? 'application/octet-stream' });
  createReadStream(target).pipe(response);
});
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
const origin = `http://127.0.0.1:${server.address().port}`;

const pageUrl = (view, count) => {
  const params = new URLSearchParams({
    style: styleUrl(view.style),
    lat: view.lat,
    lon: view.lon,
    zoom: view.zoom,
    bearing: view.bearing,
    pitch: view.pitch,
    width: view.width,
    height: view.height,
    ratio: view.ratio,
    workers: String(count),
  });
  return `${origin}/index.html?${params}`;
};

// MARK: - Chromium over the DevTools protocol

async function launch() {
  const profile = await mkdtemp(path.join(tmpdir(), 'mln-web-check-profile-'));
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
      '--enable-features=Vulkan',
      '--use-angle=vulkan',
      '--remote-debugging-port=0',
      `--user-data-dir=${profile}`,
      'about:blank',
    ],
    { stdio: ['ignore', 'ignore', 'pipe'], env: { ...process.env, VK_DRIVER_FILES: icd } },
  );
  let log = '';
  const url = await new Promise((resolve, reject) => {
    child.stderr.on('data', (chunk) => {
      log += chunk;
      const match = /DevTools listening on (ws:\/\/\S+)/.exec(log);
      if (match) resolve(match[1]);
    });
    child.on('exit', (code) => reject(new Error(`chromium exited with ${code}\n${log}`)));
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
    close() {
      socket.close();
      child.kill();
    },
  };
}

/** Opens `url` in a new tab; returns the session, with the console's errors. */
async function open(browser, url) {
  const { targetId } = await browser.send('Target.createTarget', { url: 'about:blank' });
  const { sessionId } = await browser.send('Target.attachToTarget', { targetId, flatten: true });
  const errors = [];
  browser.listeners.add((message) => {
    if (message.sessionId !== sessionId) return;
    if (process.env.MLN_WEB_CHECK_LOG && message.method === 'Runtime.consoleAPICalled') {
      console.error(`[page] ${message.params.args.map((arg) => arg.value ?? arg.description).join(' ')}`);
    }
    if (message.method === 'Runtime.consoleAPICalled' && message.params.type === 'error') {
      errors.push(message.params.args.map((arg) => arg.value ?? arg.description).join(' '));
    } else if (message.method === 'Runtime.exceptionThrown') {
      errors.push(message.params.exceptionDetails.exception?.description ?? message.params.exceptionDetails.text);
    }
  });
  await browser.send('Runtime.enable', {}, sessionId);
  await browser.send('Page.enable', {}, sessionId);
  const loaded = new Promise((resolve) => {
    const listener = (message) => {
      if (message.sessionId === sessionId && message.method === 'Page.loadEventFired') {
        browser.listeners.delete(listener);
        resolve();
      }
    };
    browser.listeners.add(listener);
  });
  await browser.send('Page.navigate', { url }, sessionId);
  await loaded;
  return { targetId, sessionId, errors };
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

/** Loads `view` with `count` workers; resolves with the page's statistics and its frame. */
async function load(browser, view, count) {
  const session = await open(browser, pageUrl(view, count));
  try {
    const stats = await evaluate(
      browser,
      session,
      `(async () => {
        const deadline = performance.now() + 120000;
        while (!window.mln) {
          if (performance.now() > deadline) throw new Error('the page did not start');
          await new Promise((resolve) => setTimeout(resolve, 50));
        }
        return await Promise.race([
          window.mln.loaded,
          new Promise((_, reject) => setTimeout(() => reject(new Error('not loaded after 120 s')), 120000)),
        ]);
      })()`,
    );
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
    return { stats, pixels, errors: session.errors };
  } finally {
    await browser.send('Target.closeTarget', { targetId: session.targetId });
  }
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
try {
  const gpu = await (async () => {
    const session = await open(browser, `${origin}/blank`);
    try {
      return await evaluate(
        browser,
        session,
        `navigator.gpu?.requestAdapter().then((a) => (a ? \`\${a.info.vendor} \${a.info.architecture} \${a.info.description}\` : 'none'))`,
      );
    } finally {
      await browser.send('Target.closeTarget', { targetId: session.targetId });
    }
  })();
  console.log(`WebGPU adapter: ${gpu}`);

  await compare();
} finally {
  browser.close();
  server.close();
}

async function compare() {
  const out = path.resolve(options.out ?? (await mkdtemp(path.join(tmpdir(), 'mln-web-check-'))));
  const styles = new Map();
  const loaded = new Map();
  for (const view of views) {
    const { stats, pixels, errors } = await load(browser, view, workers);
    loaded.set(view, { stats, errors });
    if (!styles.has(view.style)) styles.set(view.style, await (await fetch(styleUrl(view.style))).json());
    const style = structuredClone(styles.get(view.style));
    style.center = [Number(view.lon), Number(view.lat)];
    style.zoom = Number(view.zoom);
    style.bearing = Number(view.bearing);
    style.pitch = Number(view.pitch);
    style.metadata = {
      test: { width: Number(view.width), height: Number(view.height), pixelRatio: Number(view.ratio) },
    };
    const dir = path.join(out, 'tests/render-tests', view.style, view.name);
    await mkdir(dir, { recursive: true });
    await writeFile(path.join(dir, 'style.json'), JSON.stringify(style, null, 1));
    await writeFile(path.join(dir, 'expected.png'), png(pixels.width, pixels.height, Buffer.from(pixels.base64, 'base64')));
    console.log(`${view.style}/${view.name}: loaded in ${Math.round(stats.loadedMs)} ms, ${stats.replies} worker replies` +
      (errors.length ? `, ${errors.length} console errors:\n  ${errors.join('\n  ')}` : ''));
  }

  // The native renders, compared with the page's frames as references.
  const manifest = path.join(out, 'manifest.json');
  await writeFile(manifest, JSON.stringify({
    base_test_path: path.join(out, 'tests'),
    cache_path: path.join(out, 'cache.db'),
    expectation_paths: [],
    ignore_paths: [],
  }));
  const targetDir = process.env.CARGO_TARGET_DIR ?? path.join(repo, 'target');
  const runner = options['render-test'] ?? path.join(targetDir, 'debug/render-test');
  const args = ['-p', manifest, '--online', '--output', path.join(out, 'results')];
  if (options.codec) args.push(`--tileWorkerCodec=${options.codec}`);
  const native = spawnSync(runner, args, { env: { ...process.env, VK_DRIVER_FILES: icd }, encoding: 'utf8' });
  if (native.error) throw native.error;
  const page = await readFile(path.join(out, 'results/manifest.html'), 'utf8');
  console.log(`\nnative render (${runner}${options.codec ? `, codec with ${options.codec} instances` : ''}) ` +
    `against the page's frame, ${workers} tile workers:`);
  for (const item of page.split('<div class="test ').slice(1)) {
    const id = /<\/span> ([^<]+)<\/h2>/.exec(item)?.[1];
    const diff = Number(/<strong>Diff:<\/strong> ([0-9.e-]+)/.exec(item)?.[1] ?? 0);
    const error = /<strong>Error:<\/strong> ([^<]*)/.exec(item)?.[1];
    console.log(`  ${id}: ${error ? `error: ${error}` : `${(diff * 100).toFixed(3)}% of the pixels differ`}`);
  }
  console.log(`\nviews, frames and diffs: ${out}`);
}
