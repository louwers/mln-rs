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
//   --chromium <path>    another Chromium than playwright-core's (install
//                        it: `pnpm -C examples/web exec playwright-core
//                        install --no-shell chromium`)
//
// The page's WebGPU adapter is SwiftShader, which Chromium bundles, so the
// frames don't depend on the machine; the native runner renders through
// VK_DRIVER_FILES (default /usr/share/vulkan/icd.d/lvp_icd.x86_64.json,
// lavapipe). Headless Chromium composites WebGPU canvases as white, so the
// page reads its frame back itself.

import { existsSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

import { launch } from "./chromium.mjs";
import { openFixtures } from "./fixtures.mjs";
import { refetches } from "./interactions.mjs";
import { renderNative, writeTest } from "./native.mjs";
import {
  checkCompare,
  checkPanic,
  firstLines,
  load,
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

// An interaction view, whose fixtures come as over a network (server.mjs),
// fails when the map fetches more than this many URLs again.
const MAX_REPEATS = 1;

const { values: options } = parseArgs({
  options: {
    filter: { type: "string", default: "" },
    workers: { type: "string", default: "2" },
    out: { type: "string" },
    "render-test": { type: "string" },
    codec: { type: "string" },
    backend: { type: "string", default: "webgpu" },
    chromium: { type: "string" },
  },
});
const workers = Number(options.workers);
if (!["webgpu", "webgl2"].includes(options.backend))
  throw new Error(`--backend is webgpu or webgl2, not ${options.backend}`);
const icd =
  process.env.VK_DRIVER_FILES ?? "/usr/share/vulkan/icd.d/lvp_icd.x86_64.json";
if (!existsSync(path.join(root, "index.html")))
  throw new Error("no dist: run `mise run web:build` first");

const fixtures = openFixtures(cache);
const views = selectViews(options.filter, {
  tests: path.join(metrics, "integration/render-tests"),
  fixtures,
});
const server = await startServer({ root, views, fixtures });
const browser = await launch(options.chromium);
const context = { browser, server, workers, backend: options.backend };
let failures = [];
try {
  const gpu = await webgpuAdapter(context);
  console.log(`WebGPU adapter: ${gpu}`);
  if (!/swiftshader/.test(gpu ?? ""))
    throw new Error(`the page's WebGPU adapter is ${gpu}, not SwiftShader`);

  failures = await compare();
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
          ? `, after the ${view.interaction} ${again.count} fetches again (at most ${MAX_REPEATS})`
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
    if (again && again.count > MAX_REPEATS) {
      const urls = again.urls
        .slice(0, 3)
        .map(
          ([url, count]) => `${url.replace(server.origin, "")} ${count} times`,
        );
      failures.push(
        `${view.name}: ${again.count} fetches again after the ${view.interaction}, of ${urls.join(", ")}`,
      );
    }
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
