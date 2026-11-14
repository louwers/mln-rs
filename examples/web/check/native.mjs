// The native render-test runner, which renders each view and compares its
// render with the page's frame as the test's reference (pixelmatch, as the
// render tests compare).

import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";

import { png } from "./png.mjs";

/**
 * Writes the render test `name` into `out`/tests/render-tests: `style`,
 * and the page's frame, `pixels` (`{ width, height, base64 }` of RGBA), as
 * its expected image.
 */
export async function writeTest(out, name, style, { width, height, base64 }) {
  const dir = path.join(out, "tests/render-tests", name);
  await mkdir(dir, { recursive: true });
  await writeFile(path.join(dir, "style.json"), JSON.stringify(style, null, 1));
  await writeFile(
    path.join(dir, "expected.png"),
    png(width, height, Buffer.from(base64, "base64")),
  );
}

/**
 * Runs `runner` on the tests in `out` with the fixtures of the cache
 * database at `cache`, through the Vulkan driver `icd`; its results go to
 * `out`/results. Returns the runner's exit status and output, and
 * `results`, each test's `{ status, diff, error }` by name, unless the
 * runner wrote no report.
 */
export async function renderNative({ out, runner, cache, codec, icd }) {
  const manifest = path.join(out, "manifest.json");
  await writeFile(
    manifest,
    JSON.stringify({
      base_test_path: path.join(out, "tests"),
      cache_path: cache,
      expectation_paths: [],
      ignore_paths: [],
    }),
  );
  const args = ["-p", manifest, "--output", path.join(out, "results")];
  if (codec) args.push(`--tileWorkerCodec=${codec}`);
  const native = spawnSync(runner, args, {
    env: { ...process.env, VK_DRIVER_FILES: icd },
    encoding: "utf8",
  });
  if (native.error) throw native.error;
  const { status, stdout, stderr } = native;
  const report = path.join(out, "results/manifest.html");
  if (!existsSync(report)) return { status, stdout, stderr };
  return {
    status,
    stdout,
    stderr,
    results: parseReport(await readFile(report, "utf8")),
  };
}

/** Each test's result in the runner's HTML report, by name. */
function parseReport(page) {
  const results = new Map();
  for (const item of page.split('<div class="test ').slice(1)) {
    const id = /<\/span> render-tests\/([^<]+)<\/h2>/.exec(item)?.[1];
    const status = /^(\w+)/.exec(item)?.[1];
    const diff = Number(
      /<strong>Diff:<\/strong> ([0-9.e-]+)/.exec(item)?.[1] ?? 0,
    );
    const error = /<strong>Error:<\/strong> ([^<]*)/.exec(item)?.[1];
    results.set(id, { status, diff, error });
  }
  // The report has an entry only for a failed or errored test. Every other
  // test in its sequence passed, under its view's threshold.
  const sequence =
    /<strong>Test sequence:<\/strong>([^<]*)<\/p>/
      .exec(page)?.[1]
      .trim()
      .split(/\s+/) ?? [];
  for (const id of sequence) {
    const name = id.replace(/^render-tests\//, "");
    if (!results.has(name)) results.set(name, { status: "passed" });
  }
  return results;
}
