// Checks the browser build in a headless browser, offline: loads views of the
// render suite's fixtures in the page (web/), reads each frame back from the
// GPU inside the page, and compares it with the native render-test runner's
// render of the same view (pixelmatch, as the render tests compare). It
// fails when a frame differs by more than its threshold, when the page, one of
// its Web Workers (the map's or a tile worker's) or the native runner reports
// an error, or when a view's tiles got no worker replies. The interaction
// views drag on the map back and forth before their frame, over a slow
// network, and log the URLs the map fetched again. It also runs
// mln_renderer's conformance cases of writes from JS arrays, and of writes
// while the instance's memory grows, on a GPU of the map's
// backend (the page's `checkGpu`), makes the map's wasm instance and a tile worker's
// panic (the page's `panic`), and fails unless the page stops and shows the
// panic, and opens a comparison with GL JS (the page's `compare`), and
// fails unless GL JS's map covers the page's and takes the input right of
// the divider. Last, in Chromium with WebGPU, it crashes the browser's GPU
// process under a loaded map, and fails unless the page loads again by
// itself and draws the same frame with WebGPU, then loads again with WebGL2
// after a second crash. CI runs it (the
// `web` job); see ../README.md.
//
//   node examples/web/check/check.mjs [options]
//
//   --filter <regex>     the views of VIEWS (views.mjs) to load, by name
//                        (default: all)
//   --workers <n>        Web Workers for the tiles (default 2)
//   --out <dir>          where the views, frames and results go
//                        (default: a new temporary directory)
//   --render-test <path> the native runner (default:
//                        $CARGO_TARGET_DIR/debug/render-test)
//   --codec <n>          render natively with --tileWorkerCodec=<n>
//   --backend <name>     what the page draws with: webgpu (default), or
//                        webgl2, the module for browsers without WebGPU
//   --browser <name>     chromium (default), firefox or webkit: Playwright's
//                        build (`pnpm -C examples/web exec playwright-core
//                        install --no-shell chromium`, or `install firefox
//                        webkit`)
//   --channel <name>     an installed browser instead, through Playwright's
//                        channel: chrome or msedge (Chromium), moz-firefox
//                        (Firefox)
//   --executable <path>  the browser's executable, instead of Playwright's or
//                        the channel's
//   --gpu                Chromium draws on the machine's GPU, not SwiftShader
//
// Chromium's WebGPU adapter is SwiftShader, which it bundles, so the
// frames don't depend on the machine (browser.mjs); the native runner
// renders through VK_DRIVER_FILES, which defaults to lavapipe
// (/usr/share/vulkan/icd.d/lvp_icd.x86_64.json) where that is installed and
// otherwise stays unset, so the runner draws on the machine's default device.
// Headless Chromium composites WebGPU canvases as white, so the page reads its frame back
// itself.

import { existsSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

import { launch } from "./browser.mjs";
import { openFixtures } from "./fixtures.mjs";
import { refetches } from "./interactions.mjs";
import { renderNative, writeTest } from "./native.mjs";
import {
  checkCompare,
  checkGpu,
  checkLostDevice,
  checkPanic,
  firstLines,
  load,
  webgl2Renderer,
  webgpuAdapter,
} from "./page.mjs";
import { startServer } from "./server.mjs";
import { selectViews } from "./views.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, "../../..");
const metrics = path.join(repo, "tools/render-test/testdata/metrics");
const cache = path.join(metrics, "cache-style.db");
// The page as Vite builds it, as it deploys.
const root = path.join(here, "../dist");

const { values: options } = parseArgs({
  options: {
    filter: { type: "string", default: "" },
    workers: { type: "string", default: "2" },
    out: { type: "string" },
    "render-test": { type: "string" },
    codec: { type: "string" },
    backend: { type: "string", default: "webgpu" },
    browser: { type: "string", default: "chromium" },
    channel: { type: "string" },
    executable: { type: "string" },
    gpu: { type: "boolean", default: false },
  },
});
const workers = Number(options.workers);
if (!["webgpu", "webgl2"].includes(options.backend))
  throw new Error(`--backend is webgpu or webgl2, not ${options.backend}`);
const lavapipe = "/usr/share/vulkan/icd.d/lvp_icd.x86_64.json";
const icd =
  process.env.VK_DRIVER_FILES ?? (existsSync(lavapipe) ? lavapipe : undefined);
if (!existsSync(path.join(root, "index.html")))
  throw new Error("no dist: run `mise run web:build` first");

const fixtures = openFixtures(cache);
const views = selectViews(options.filter, {
  tests: path.join(metrics, "integration/render-tests"),
  fixtures,
}).filter((view) => {
  // The pinch is the DevTools protocol's (interactions.mjs), which only
  // Chromium speaks.
  const runs =
    options.browser === "chromium" || view.interaction !== "pan-zoom";
  if (!runs) console.log(`${view.name}: skipped, its pinch needs Chromium`);
  return runs;
});
const server = await startServer({ root, views, fixtures });
const browser = await launch({
  browser: options.browser,
  channel: options.channel,
  executable: options.executable,
  onGpu: options.gpu,
});
const context = { browser, server, workers, backend: options.backend };
let failures = [];
try {
  const gpu = await webgpuAdapter(context);
  console.log(`${browser.name}`);
  console.log(`WebGPU adapter: ${gpu}`);
  console.log(`WebGL2 renderer: ${await webgl2Renderer(context)}`);
  console.log(
    `native device: ${icd ? `Vulkan driver ${icd}` : "the machine's default"}`,
  );
  if (options.backend === "webgpu" && gpu === "none")
    throw new Error(`${browser.name} has no WebGPU: try --backend webgl2`);
  if (browser.swiftshader && !/swiftshader/.test(gpu ?? ""))
    throw new Error(`the page's WebGPU adapter is ${gpu}, not SwiftShader`);

  failures = await compare();
  const gpuCheck = await checkGpu(context, views[0]);
  console.log(`a check of the map's GPU: ${gpuCheck ?? "passed"}`);
  if (gpuCheck) failures.push(`a check of the map's GPU: ${gpuCheck}`);
  for (const [where, instance] of [
    ["map", "the map's instance"],
    ["tile", "a tile worker's instance"],
  ]) {
    const failure = await checkPanic(context, views[0], where);
    console.log(
      `a panic in ${instance}: ${failure ?? "the page stopped and showed it"}`,
    );
    if (failure) failures.push(`a panic in ${instance}: ${failure}`);
  }
  const comparing = await checkCompare(context, views[0]);
  console.log(
    `a comparison with GL JS: ${comparing ?? "GL JS's map covers the page's, right of the divider"}`,
  );
  if (comparing) failures.push(`a comparison with GL JS: ${comparing}`);
  // Last, since it leaves the browser on a new GPU process.
  if (browser.crashGpuProcess && options.backend === "webgpu") {
    const losing = await checkLostDevice(context, views[0]);
    console.log(
      `a lost WebGPU device: ${losing ?? "the page loaded again and drew the same frame, and with WebGL2 after a second loss"}`,
    );
    if (losing) failures.push(`a lost WebGPU device: ${losing}`);
  } else {
    console.log("a lost WebGPU device: skipped, it needs Chromium and WebGPU");
  }
} finally {
  await browser.close();
  server.close();
  fixtures.close();
}
if (failures.length > 0) {
  console.log(`\nfailed:\n  ${failures.join("\n  ")}`);
  process.exitCode = 1;
}

/** Loads every view, then compares the frames with native renders; returns what failed. */
async function compare() {
  const out = path.resolve(
    options.out ?? (await mkdtemp(path.join(tmpdir(), "mln-web-check-"))),
  );
  // A kept --out holds an earlier run's views and results; without this a
  // runner that writes no results would leave the old ones to be read.
  for (const dir of ["tests", "results"])
    await rm(path.join(out, dir), { recursive: true, force: true });
  const failures = [];
  const written = new Set();
  for (const view of views) {
    const missing = server.beginView({ slow: view.interaction !== undefined });
    const { stats, pixels, interaction, errors, failure } = await load(
      context,
      view,
    );
    server.endView();
    const again = interaction && refetches(interaction.fetches);
    if (process.env.MLN_WEB_CHECK_LOG && again)
      console.error(`  fetched again: ${JSON.stringify(again.urls)}`);
    console.log(
      `${view.name}: ` +
        (failure
          ? failure
          : `loaded in ${Math.round(stats.loadedMs)} ms, ${stats.replies} worker replies`) +
        (again
          ? `, after the ${view.interaction} ${again.count} fetches again`
          : "") +
        (missing.length
          ? `, ${missing.length} tiles or resources not in the cache (not found)`
          : "") +
        (errors.length
          ? `, ${errors.length} errors:\n  ${errors.map(firstLines).join("\n  ")}`
          : ""),
    );
    if (process.env.MLN_WEB_CHECK_LOG && missing.length)
      console.error(`  not in the cache: ${missing.join(", ")}`);
    if (failure) {
      failures.push(`${view.name}: ${failure}`);
      continue;
    }
    if (errors.length)
      failures.push(
        `${view.name}: ${errors.length} errors in the page or its workers`,
      );
    // Every view has tiles, and only their workers reply: the raster and
    // raster-dem tiles' too, in the views that have nothing else.
    if (stats.replies === 0) failures.push(`${view.name}: no worker replies`);
    // An interaction's frame is of the camera it left the map at.
    let style = view.style;
    if (interaction) {
      const [lat, lon, zoom, bearing, pitch] = interaction.camera;
      style = { ...style, center: [lon, lat], zoom, bearing, pitch };
    }
    await writeTest(out, view.name, style, pixels);
    written.add(view);
  }
  if (written.size === 0) return failures;

  // The native renders, compared with the page's frames as references.
  const targetDir = process.env.CARGO_TARGET_DIR ?? path.join(repo, "target");
  const runner =
    options["render-test"] ?? path.join(targetDir, "debug/render-test");
  const native = await renderNative({
    out,
    runner,
    cache,
    codec: options.codec,
    icd,
  });
  if (!native.results) {
    console.log(native.stdout, native.stderr);
    return [
      ...failures,
      `the native runner exited with ${native.status} and no results`,
    ];
  }
  console.log(
    `\nnative render (${runner}${options.codec ? `, codec with ${options.codec} instances` : ""}) ` +
      `against the page's frame, ${workers} tile workers:`,
  );
  for (const view of written) {
    const result = native.results.get(view.name);
    if (!result) {
      console.log(`  ${view.name}: not rendered`);
      failures.push(`${view.name}: the native runner didn't render it`);
    } else if (result.error) {
      console.log(`  ${view.name}: error: ${result.error}`);
      failures.push(`${view.name}: the native runner: ${result.error}`);
    } else if (result.diff === undefined) {
      console.log(
        `  ${view.name}: passed (at most ${(view.allowed * 100).toFixed(1)}% of the pixels differ)`,
      );
    } else {
      console.log(
        `  ${view.name}: ${(result.diff * 100).toFixed(3)}% of the pixels differ ` +
          `(at most ${(view.allowed * 100).toFixed(1)}%)`,
      );
      if (result.status !== "passed" || result.diff > view.allowed) {
        failures.push(
          `${view.name}: ${(result.diff * 100).toFixed(3)}% of the pixels differ (${result.status})`,
        );
      }
    }
  }
  console.log(`\nviews, frames and diffs: ${out}`);
  return failures;
}
