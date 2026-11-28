// The browser package's entry (`mise run c-web:build`): the module's
// bindings (maplibre_native_c.js), whose initialization also points the
// library at the files that the package ships beside them, the tile
// workers' script (tile_worker.js) and the font-face shaper module
// (mln_web_shaper.js). Load the package through this file, not the bindings.

import initBindings, {
  initSync as initBindingsSync,
  mlnWebConfigureTileWorkers,
  mlnWebSetShaperLoader,
} from "./maplibre_native_c.js";

export * from "./maplibre_native_c.js";

// Whether `configure` has run: a later `init` returns the same instance and
// leaves whatever the host has configured since.
let configured = false;

function configure() {
  if (configured) return;
  configured = true;
  mlnWebConfigureTileWorkers(
    undefined,
    () =>
      new Worker(new URL("./tile_worker.js", import.meta.url), {
        type: "module",
      }),
  );
  mlnWebSetShaperLoader(async () => {
    const shaper = await import(
      new URL("./mln_web_shaper.js", import.meta.url).href
    );
    await shaper.default();
    return shaper;
  });
}

/** Instantiates the module, as the bindings' own default export does. */
export default async function init(options) {
  const exports = await initBindings(options);
  configure();
  return exports;
}

/** Instantiates the module synchronously, as the bindings' `initSync`. */
export function initSync(options) {
  const exports = initBindingsSync(options);
  configure();
  return exports;
}
