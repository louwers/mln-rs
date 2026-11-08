// The page: it runs no wasm itself. It compiles the module, gives its canvas
// to the map's Web Worker (map-worker.js) as an OffscreenCanvas, and starts
// the tile workers' Web Workers (worker.js), each connected to the map's by
// a MessageChannel, so that their requests and replies never pass through
// this thread. Each Web Worker instantiates the same module with its own
// memory, so nothing needs SharedArrayBuffer or COOP/COEP headers.
//
// URL parameters:
//   style=<url>            the style (default: OpenFreeMap Liberty)
//   lat, lon, zoom, bearing, pitch
//   workers=<n>            Web Workers for the tiles (default: up to 4)
//   width, height, ratio   a fixed canvas size in CSS pixels and pixel ratio
//                          (default: the window and devicePixelRatio)
//
// `window.mln` is for scripts (check/check.mjs): `loaded` resolves with the
// load time and the number of worker replies once the map is fully loaded,
// and `readPixels()` draws the frame again and resolves with its RGBA
// pixels, read back from the GPU.

const params = new URLSearchParams(location.search);
const number = (name, fallback) => (params.has(name) ? Number(params.get(name)) : fallback);
const status = document.getElementById('status');
const canvas = document.getElementById('map');

if (params.has('width') && params.has('height')) {
  canvas.style.width = `${number('width')}px`;
  canvas.style.height = `${number('height')}px`;
}
const ratio = number('ratio', window.devicePixelRatio || 1);
canvas.width = Math.round(canvas.clientWidth * ratio);
canvas.height = Math.round(canvas.clientHeight * ratio);

const module = await WebAssembly.compileStreaming(fetch('./pkg/mln_web_bg.wasm'));

const count = number('workers', Math.max(1, Math.min(4, (navigator.hardwareConcurrency || 2) - 1)));
const ports = Array.from({ length: count }, () => {
  const channel = new MessageChannel();
  const worker = new Worker('./worker.js', { type: 'module' });
  worker.postMessage({ module, port: channel.port2 }, [channel.port2]);
  return channel.port1;
});

const map = new Worker('./map-worker.js', { type: 'module' });
const offscreen = canvas.transferControlToOffscreen();
map.postMessage({
  module,
  canvas: offscreen,
  ports,
  width: canvas.clientWidth,
  height: canvas.clientHeight,
  ratio,
  style: new URL(params.get('style') ?? 'https://tiles.openfreemap.org/styles/liberty', location.href).href,
  camera: [number('lat', 52.37), number('lon', 4.89), number('zoom', 13), number('bearing', 0), number('pitch', 0)],
}, [offscreen, ...ports]);

let markLoaded;
const pixelRequests = [];
window.mln = {
  loaded: new Promise((resolve) => { markLoaded = resolve; }),
  readPixels() {
    return new Promise((resolve) => {
      pixelRequests.push(resolve);
      map.postMessage({ readPixels: true });
    });
  },
};

map.onmessage = ({ data }) => {
  if (data.status !== undefined) status.textContent = data.status;
  if (data.loaded) markLoaded(data.loaded);
  if (data.pixels) pixelRequests.shift()?.(data.pixels);
};
