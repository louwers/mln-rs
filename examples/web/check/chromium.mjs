// Headless Chromium through Playwright: the build that playwright-core pins
// (`pnpm -C examples/web exec playwright-core install --no-shell chromium`).
// Its WebGPU adapter is SwiftShader, which Chromium bundles, so the frames
// don't depend on the machine. Its WebGL2 runs on SwiftShader too.

import { chromium } from "playwright-core";

const args = [
  "--enable-unsafe-webgpu",
  // SwiftShader, which Chromium bundles, for WebGPU and for compositing
  // through Vulkan: with headless Chromium's default GL compositing, the
  // copy of the frame that the page reads back is never mapped.
  "--enable-features=Vulkan",
  "--use-angle=vulkan",
  "--use-vulkan=swiftshader",
  "--use-webgpu-adapter=swiftshader",
  // Offline: a request that escapes the local server (a URL that the server
  // doesn't rewrite) fails to resolve, an error in the page or a worker,
  // rather than reaching the network.
  "--host-resolver-rules=MAP * ~NOTFOUND , EXCLUDE 127.0.0.1",
];

/**
 * Launches headless Chromium, offline: playwright-core's, or the one at
 * `executable`. Returns the browser, whose `open(url)` opens a tab in a
 * window as large as the largest view.
 */
export async function launch(executable) {
  const env = { ...process.env };
  delete env.VK_DRIVER_FILES; // the native runner's driver
  const browser = await chromium.launch({
    // The full Chromium, in its new headless mode, rather than the
    // headless shell.
    channel: "chromium",
    executablePath: executable,
    args,
    env,
  });
  const context = await browser.newContext({
    viewport: { width: 1024, height: 768 },
  });
  return {
    open: (url) => open(context, url),
    close: () => browser.close(),
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
