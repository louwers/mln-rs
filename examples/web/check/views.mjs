// The views the check loads: render tests of the suite whose operations the
// page can follow. The real-world styles (vector tiles with fills, lines,
// icons and labels), raster tiles, raster-dem tiles (hillshade and color
// relief, whose borders the map side backfills from the neighbouring tiles),
// raster with GeoJSON, and labels in font faces (complex scripts and
// right-to-left text, which the map shapes with the shaper module it loads
// for them).

import { readFileSync } from "node:fs";
import path from "node:path";

// THRESHOLD is the share of a view's pixels that may differ (pixelmatch,
// with the render tests' per-pixel threshold), unless the view says
// otherwise. The page renders on SwiftShader and the native runner on
// lavapipe; most views come out identical, and leaving out one layer of
// labels changes 0.1% to 2% of the pixels.
const THRESHOLD = 0.001;
const VIEWS = [
  { id: "real-world/nepal" },
  { id: "real-world/norway" },
  { id: "real-world/uruguay" },
  { id: "real-world/chicago" },
  { id: "real-world/sanfrancisco" },
  { id: "raster-rotation/45" },
  { id: "raster-masking/overlapping-vector" },
  { id: "hillshade-accent-color/default" },
  // SwiftShader and lavapipe antialias the edges of the z1 coastlines
  // differently: 0.5% of the pixels.
  {
    id: "combinations/color-relief--hillshade--vector-fill--color-relief--hillshade",
    allowed: 0.01,
  },
  { id: "text-font/devanagari" },
  { id: "text-font/khmer" },
  { id: "text-font/font-faces-bidi" },
  { id: "is-supported-script/font-faces-layout" },
  // Interactions (interact()): a right-drag that turns the map back and
  // forth, and a pinch that pans and zooms it back and forth, as
  // mln-bench's Frame/Oscillate and Frame/OscillatePanZoom move their
  // cameras. Each starts where some tiles come into view only for moments
  // of each swing, shorter than a tile takes to load.
  {
    id: "real-world/chicago",
    name: "interaction/rotate",
    interaction: "rotate",
    width: 1024,
    height: 768,
    center: [-87.704736328125, 41.86203372809279], // z13 x 2100.23, y 3045.23
  },
  {
    id: "real-world/sanfrancisco",
    name: "interaction/pan-zoom",
    interaction: "pan-zoom",
    width: 1024,
    height: 768,
    center: [-122.4591064453125, 37.77071473849609], // z15 x 5237.5, y 12666
    zoom: 15.9,
  },
];

/**
 * The views whose names match `filter`, each with its render test's style
 * from `tests` (the suite's render-tests directory), with the test's
 * operations followed so that the camera is in the style. The page can't
 * follow other operations or test options, so a view must not have them.
 * `fixtures` gives the styles that a `setStyle` operation names.
 */
export function selectViews(filter, { tests, fixtures }) {
  const pattern = new RegExp(filter);
  const views = VIEWS.filter(({ id, name = id }) => pattern.test(name)).map(
    (entry) => view(entry, { tests, fixtures }),
  );
  if (views.length === 0) throw new Error(`no view matches ${filter}`);
  return views;
}

function view(
  { id, name = id, interaction, allowed = THRESHOLD, ...camera },
  { tests, fixtures },
) {
  const test = JSON.parse(
    readFileSync(path.join(tests, id, "style.json"), "utf8"),
  );
  const metadata = test.metadata?.test ?? {};
  for (const key of Object.keys(metadata)) {
    if (
      ![
        "width",
        "height",
        "pixelRatio",
        "operations",
        "allowed",
        "description",
      ].includes(key)
    ) {
      throw new Error(`${id}: the page can't follow the test's "${key}"`);
    }
  }
  let style = test;
  for (const [operation, ...args] of metadata.operations ?? []) {
    if (operation === "setStyle") style = fixtures.cachedJson(args[0]);
    else if (operation === "setZoom") style.zoom = args[0];
    else if (operation === "setCenter") style.center = args[0];
    else if (operation !== "wait")
      throw new Error(
        `${id}: the page can't follow the operation ${operation}`,
      );
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
