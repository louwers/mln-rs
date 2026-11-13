// Loads crates/mln_c's browser package (`just c-web <backend>`) in headless
// Chromium and drives its raw C ABI from a module Web Worker (worker.js), as
// a JavaScript host does: a runtime, a map whose GeoJSON layer the tile
// workers lay out, and sessions one after another, each on a new
// OffscreenCanvas and a device of its own, that render until the canvas
// shows the layer and then end, once with the sessions on a core worker and
// once driven by the worker's own services. Fails when a canvas never shows
// the layer or a session fails, or the library starts no tile worker, or the
// page or a worker logs an error.
//
// Usage: node crates/mln_c/web-check/check.mjs <package dir> <backend>
// [--chromium <path>]. Node 24 with no packages, and Playwright's Chromium
// (`npx playwright@1.61.1 install chromium`), as examples/web/check does.

import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { createServer } from 'node:http';
import { homedir, tmpdir } from 'node:os';
import path from 'node:path';
import { parseArgs } from 'node:util';

const { values: options, positionals } = parseArgs({
  allowPositionals: true,
  options: {
    chromium: {
      type: 'string',
      default: path.join(
        process.env.PLAYWRIGHT_BROWSERS_PATH ?? path.join(homedir(), '.cache/ms-playwright'),
        'chromium-1228',
        'chrome-linux64/chrome',
      ),
    },
  },
});
const [pkg, backend] = positionals;
if (!pkg || !backend) throw new Error('usage: check.mjs <package dir> <backend>');
const here = path.dirname(new URL(import.meta.url).pathname);

const types = { '.js': 'text/javascript', '.wasm': 'application/wasm', '.html': 'text/html' };
const server = createServer(async (request, response) => {
  const url = new URL(request.url, 'http://localhost');
  const file = url.pathname.startsWith('/pkg/')
    ? path.join(pkg, url.pathname.slice('/pkg/'.length))
    : path.join(here, url.pathname.slice(1) || 'index.html');
  try {
    const body = await readFile(file);
    response.writeHead(200, { 'content-type': types[path.extname(file)] ?? 'application/octet-stream' });
    response.end(body);
  } catch {
    response.writeHead(404);
    response.end();
  }
});
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
const origin = `http://127.0.0.1:${server.address().port}`;

const profile = await mkdtemp(path.join(tmpdir(), 'mln-c-web-check-'));
const chromium = spawn(
  options.chromium,
  [
    '--headless=new',
    '--no-sandbox',
    '--no-first-run',
    '--disable-dev-shm-usage',
    '--enable-unsafe-webgpu',
    '--enable-features=Vulkan',
    '--use-angle=vulkan',
    '--use-vulkan=swiftshader',
    '--use-webgpu-adapter=swiftshader',
    '--remote-debugging-port=0',
    `--user-data-dir=${profile}`,
    'about:blank',
  ],
  { stdio: ['ignore', 'ignore', 'pipe'] },
);
const exited = new Promise((resolve) => chromium.on('exit', resolve));
let log = '';
const endpoint = await new Promise((resolve, reject) => {
  chromium.stderr.on('data', (chunk) => {
    log += chunk;
    const match = /DevTools listening on (ws:\/\/\S+)/.exec(log);
    if (match) resolve(match[1]);
  });
  exited.then((code) => reject(new Error(`chromium exited with ${code}\n${log}`)));
});
const socket = new WebSocket(endpoint);
await new Promise((resolve, reject) => {
  socket.onopen = resolve;
  socket.onerror = reject;
});
let next = 1;
const pending = new Map();
const errors = [];
const sessions = new Set();
const send = (method, params = {}, sessionId = undefined) =>
  new Promise((resolve, reject) => {
    const id = next++;
    pending.set(id, { resolve, reject });
    socket.send(JSON.stringify({ id, method, params, sessionId }));
  });
const text = (args) => args.map((arg) => arg.value ?? arg.description).join(' ');
socket.onmessage = ({ data }) => {
  const message = JSON.parse(data);
  if (message.id !== undefined) {
    const entry = pending.get(message.id);
    pending.delete(message.id);
    if (message.error) entry.reject(new Error(JSON.stringify(message.error)));
    else entry.resolve(message.result);
    return;
  }
  if (message.method === 'Target.attachedToTarget') {
    const worker = message.params.sessionId;
    sessions.add(worker);
    send('Target.setAutoAttach', { autoAttach: true, waitForDebuggerOnStart: false, flatten: true }, worker)
      .then(() => send('Runtime.enable', {}, worker))
      .then(() => send('Runtime.runIfWaitingForDebugger', {}, worker))
      .catch((error) => errors.push(`attaching to a worker: ${error.message}`));
  } else if (message.method === 'Runtime.consoleAPICalled') {
    if (process.env.MLN_WEB_CHECK_LOG) console.error(text(message.params.args));
    if (message.params.type === 'error') errors.push(text(message.params.args));
  } else if (message.method === 'Runtime.exceptionThrown') {
    const details = message.params.exceptionDetails;
    errors.push(details.exception?.description ?? details.text);
  }
};

let failed = false;
try {
  for (const driver of ['core', 'caller']) {
    const { targetId } = await send('Target.createTarget', { url: 'about:blank' });
    const { sessionId } = await send('Target.attachToTarget', { targetId, flatten: true });
    await send('Target.setAutoAttach', { autoAttach: true, waitForDebuggerOnStart: false, flatten: true }, sessionId);
    await send('Runtime.enable', {}, sessionId);
    await send('Page.navigate', { url: `${origin}/index.html?pkg=/pkg&backend=${backend}&driver=${driver}` }, sessionId);
    let result;
    for (const deadline = Date.now() + 120000; Date.now() < deadline;) {
      const evaluated = await send(
        'Runtime.evaluate',
        { expression: 'window.result', awaitPromise: true, returnByValue: true },
        sessionId,
      );
      result = evaluated.result.value;
      if (result !== undefined) break;
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    const label = `${backend}, ${driver} driver`;
    console.log(`${label}: ${JSON.stringify(result)}`);
    if (!result?.ok) failed = true;
    else if (result.spawned === 0) {
      console.log(`${label}: the library started no tile worker`);
      failed = true;
    }
    if (errors.length) {
      console.log(`${label}: errors in the page or its workers:\n  ${errors.join('\n  ')}`);
      errors.length = 0;
      failed = true;
    }
    await send('Target.closeTarget', { targetId });
  }
} finally {
  socket.close();
  chromium.kill();
  await exited;
  server.close();
  await rm(profile, { recursive: true, force: true, maxRetries: 5 });
}
process.exit(failed ? 1 : 0);
