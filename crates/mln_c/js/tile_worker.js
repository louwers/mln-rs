// The script of the library's tile worker Web Workers in the browser package
// (`mise run c-web:build`): wasm-bindgen copies it to snippets/<crate>/js/,
// three directories below the package's root, beside the module's bindings. Each
// message is {module, port} from a map's instance: the first instantiates
// the module, and each port gets tile workers of its own.
//
// A panic aborts the instance, and nothing may call into it after that: the
// Web Worker sends {fatal: message} over each port instead.

import init, { mlnWebServeTileWorker } from '../../../maplibre_native_c.js';

const ports = [];
let ready = null;
let dead = false;

function fatal(error) {
  if (dead) return;
  dead = true;
  for (const port of ports) port.postMessage({ fatal: self.mlnPanic ?? String(error) });
}

addEventListener('error', (event) => fatal(event.error ?? event.message));
addEventListener('unhandledrejection', (event) => fatal(event.reason));

onmessage = ({ data }) => {
  ports.push(data.port);
  ready ??= init({ module_or_path: data.module });
  ready.then(() => {
    if (dead) return;
    try {
      mlnWebServeTileWorker(data.port);
    } catch (error) {
      fatal(error);
    }
  }, fatal);
};
