// A tile workers' Web Worker (tile-worker.ts) on the module that draws with
// WebGL2, so that a browser without WebGPU downloads one module.

import init, { TileWorker, panic_for_check } from './pkg/webgl/mln_web.js';
import { serve } from './tile-worker.ts';

serve({ default: init, TileWorker, panic_for_check });
