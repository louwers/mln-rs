// The headless browser, through Playwright: by default the Chromium that
// playwright-core pins (`pnpm -C examples/web exec playwright-core install
// --no-shell chromium`), whose WebGPU adapter is SwiftShader, which Chromium
// bundles, so its WebGPU frames don't depend on the machine. Playwright's
// Firefox or WebKit (`playwright-core install firefox webkit`), or an
// installed browser through a Playwright channel (`chrome`, `msedge`,
// `moz-firefox`) or its executable, draw on the machine's GPU, as Chromium
// does with `onGpu`.

import { chromium, firefox, webkit } from "playwright-core";

const browsers = { chromium, firefox, webkit };

// Chromium on SwiftShader, for WebGPU and for compositing through Vulkan:
// with headless Chromium's default GL compositing, the copy of the frame
// that the page reads back is never mapped. Its WebGL2 runs on ANGLE's
// Vulkan, on the first device the Vulkan loader finds: lavapipe, or the
// machine's GPU.
const software = [
  "--enable-unsafe-webgpu",
  "--enable-features=Vulkan",
  "--use-angle=vulkan",
  "--use-vulkan=swiftshader",
  "--use-webgpu-adapter=swiftshader",
];
// Chromium on the machine's GPU. Headless Chromium on Linux draws on
// SwiftShader unless it composites and runs ANGLE through Vulkan.
const gpu = [
  "--enable-unsafe-webgpu",
  "--ignore-gpu-blocklist",
  ...(process.platform === "linux"
    ? ["--enable-features=Vulkan", "--use-angle=vulkan"]
    : []),
];
// Offline: a request that escapes the local server (a URL that the server
// doesn't rewrite) fails to resolve, an error in the page or a worker,
// rather than reaching the network.
const offline = "--host-resolver-rules=MAP * ~NOTFOUND , EXCLUDE 127.0.0.1";

/**
 * Launches `browser` (`chromium`, `firefox` or `webkit`) headless and
 * offline: Playwright's build, or the one of `channel` or at `executable`.
 * Chromium draws its WebGPU on SwiftShader unless `onGpu`; the others draw on the
 * machine's GPU, and only Chromium is kept offline. Returns the browser,
 * whose `open(url)` opens a tab in a window as large as the largest view,
 * and `swiftshader`, whether its WebGPU should be SwiftShader. Chromium's
 * also has `crashGpuProcess()`, which crashes its GPU process: every tab
 * loses its WebGPU devices and WebGL contexts, and Chromium starts another.
 */
export async function launch({ browser, channel, executable, onGpu }) {
  const type = browsers[browser];
  if (!type)
    throw new Error(`--browser is chromium, firefox or webkit, not ${browser}`);
  const env = { ...process.env };
  delete env.VK_DRIVER_FILES; // the native runner's driver
  const isChromium = browser === "chromium";
  const instance = await type.launch({
    // Playwright's Chromium is the full Chromium, in its new headless mode,
    // rather than the headless shell.
    channel: channel ?? (isChromium ? "chromium" : undefined),
    executablePath: executable,
    args: isChromium ? [...(onGpu ? gpu : software), offline] : [],
    env,
  });
  const context = await instance.newContext({
    viewport: { width: 1024, height: 768 },
  });
  return {
    name: `${browser} ${instance.version()}${channel ? ` (${channel})` : ""}`,
    swiftshader: isChromium && !onGpu,
    open: (url) => open(context, url),
    close: () => instance.close(),
    crashGpuProcess: isChromium
      ? async () => {
          const cdp = await instance.newBrowserCDPSession();
          try {
            await cdp.send("Browser.crashGpuProcess");
          } finally {
            await cdp.detach();
          }
        }
      : undefined,
  };
}

/**
 * Opens `url` in a new tab, once it has loaded; returns the session, with
 * the errors that its console and its Web Workers' consoles report, and
 * `failed`, which resolves at the first of them. `page` is the tab, and
 * `evaluate` evaluates an expression in it and awaits its promise.
 */
async function open(context, url) {
  const page = await context.newPage();
  const errors = [];
  let fail;
  const failed = new Promise((resolve) => {
    fail = resolve;
  });
  const report = (error) => {
    errors.push(error);
    fail();
  };
  const workers = new Map();
  page.on("worker", (worker) => {
    workers.set(worker, `worker ${workers.size + 1}`);
  });
  page.on("console", (message) => {
    // Only the console's calls: the browser's own messages, such as the
    // 404s of the fixtures the cache lacks, come without arguments.
    if (message.args().length === 0) return;
    const where = workers.get(message.worker()) ?? "page";
    if (process.env.MLN_WEB_CHECK_LOG)
      console.error(`[${where}] ${message.text()}`);
    if (message.type() === "error") report(`${where}: ${message.text()}`);
  });
  // An uncaught error in the page or any of its workers.
  page.on("pageerror", (error) => report(String(error?.stack ?? error)));
  await page.goto(url);
  return {
    page,
    errors,
    failed,
    evaluate: (expression) => page.evaluate(expression),
    close: () => page.close(),
  };
}
