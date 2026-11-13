// The defaults of the browser package (`mise run c-web:build`) for what the
// library loads beside its module. wasm-bindgen copies this file to
// snippets/<crate>/js/, three directories below the package's root.

/** Loads the font-face shaper module that the package ships, initialized. */
export function shaperLoader() {
  return async () => {
    const shaper = await import(
      new URL("../../../mln_web_shaper.js", import.meta.url).href
    );
    await shaper.default();
    return shaper;
  };
}
